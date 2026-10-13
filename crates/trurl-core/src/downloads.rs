//! Resumable model downloads from Hugging Face.
//!
//! Built for connections that drop every few minutes:
//! - bytes go to `downloads/<id>.part`, the job to `downloads/<id>.json`, so a
//!   download survives app restarts and resumes where it stopped;
//! - every (re)start asks `huggingface.co/<repo>/resolve/<commit>/<file>` again
//!   with `Range: bytes=<have>-`; the signed CDN URL it redirects to is never
//!   stored, because it expires;
//! - a server that ignores the range (200), a range that does not start where
//!   we are, or a 416 are all handled;
//! - drops are retried automatically with backoff before asking the learner;
//! - the finished file must match Hugging Face's SHA-256 and parse as GGUF
//!   before it is moved into the models folder.

use crate::errors::{ErrorCode, UserError};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DownloadStatus {
    Running,
    /// Waiting before an automatic retry after a drop.
    Retrying,
    Paused,
    Verifying,
    Failed,
    Done,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadInfo {
    pub id: String,
    pub repo: String,
    /// Path of the file inside the repo.
    pub file: String,
    /// Commit SHA the download is pinned to.
    pub revision: String,
    /// Name it will get in the models folder.
    pub dest_file: String,
    pub size: u64,
    pub sha256: Option<String>,
    pub done: u64,
    pub status: DownloadStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<UserError>,
    /// Bytes per second over the last few seconds (0 when not running).
    #[serde(default)]
    pub rate: u64,
    /// The learner pressed Pause. Downloads cut by closing Trurl (or by a crash)
    /// carry on by themselves at the next start; paused ones wait for Resume.
    #[serde(default)]
    pub user_paused: bool,
    /// Bytes of the part file known to be on the disk (synced). After a crash or
    /// a power cut, Trurl resumes from here.
    #[serde(default)]
    pub synced: u64,
}

#[derive(Debug, Clone)]
pub struct DownloadRequest {
    pub repo: String,
    pub file: String,
    pub revision: String,
    pub size: u64,
    pub sha256: Option<String>,
}

pub fn download_id(repo: &str, file: &str, revision: &str) -> String {
    let h = Sha256::digest(format!("{repo}/{file}@{revision}").as_bytes());
    hex::encode(&h[..8])
}

/// `https://huggingface.co/<repo>/resolve/<rev>/<file>`, each path segment percent-encoded.
pub fn resolve_url(base: &str, repo: &str, revision: &str, file: &str) -> String {
    let enc = |s: &str| -> String {
        s.split('/')
            .map(|seg| {
                seg.bytes()
                    .map(|b| match b {
                        b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                            (b as char).to_string()
                        }
                        _ => format!("%{b:02X}"),
                    })
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("/")
    };
    format!(
        "{}/{}/resolve/{}/{}",
        base.trim_end_matches('/'),
        enc(repo),
        enc(revision),
        enc(file)
    )
}

type Notify = Arc<dyn Fn(Vec<DownloadInfo>) + Send + Sync>;
type OnDone = Arc<dyn Fn(String) + Send + Sync>;

struct Entry {
    info: DownloadInfo,
    /// What the job file on disk says (status, synced, paused, failed): it is
    /// rewritten only when one of these changes, not on every progress tick.
    saved: Option<(DownloadStatus, u64, bool, bool)>,
    cancel: Option<CancellationToken>,
}

impl Entry {
    fn new(info: DownloadInfo) -> Self {
        Entry {
            info,
            saved: None,
            cancel: None,
        }
    }
}

#[derive(Clone)]
pub struct Downloads {
    inner: Arc<Inner>,
}

struct Inner {
    dir: PathBuf,
    models_dir: PathBuf,
    base_url: String,
    http: reqwest::Client,
    token: Mutex<String>,
    entries: Mutex<HashMap<String, Entry>>,
    /// One per download id, kept even after Cancel: the task that writes a file
    /// holds it, so Pause then Resume, or Cancel then Download again, can never
    /// have two tasks writing the same file.
    run_locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// Job files are written by the download task and by Pause, Resume and
    /// closing Trurl: one at a time, or two writers could mix one file.
    job_io: Mutex<()>,
    notify: Notify,
    on_done: OnDone,
    retry_delays: Vec<Duration>,
}

impl Downloads {
    pub fn new(
        dir: PathBuf,
        models_dir: PathBuf,
        base_url: String,
        http: reqwest::Client,
        notify: Notify,
        on_done: OnDone,
    ) -> Self {
        let d = Downloads {
            inner: Arc::new(Inner {
                dir,
                models_dir,
                base_url,
                http,
                token: Mutex::new(String::new()),
                entries: Mutex::new(HashMap::new()),
                run_locks: Mutex::new(HashMap::new()),
                job_io: Mutex::new(()),
                notify,
                on_done,
                retry_delays: vec![
                    Duration::from_secs(2),
                    Duration::from_secs(5),
                    Duration::from_secs(10),
                    Duration::from_secs(20),
                    Duration::from_secs(40),
                ],
            }),
        };
        d.restore();
        d
    }

    #[doc(hidden)]
    pub fn with_retry_delays(self, delays: Vec<Duration>) -> Self {
        let inner = Arc::into_inner(self.inner).expect("set retry delays before sharing");
        Downloads {
            inner: Arc::new(Inner {
                retry_delays: delays,
                ..inner
            }),
        }
    }

    pub fn set_token(&self, token: &str) {
        *self.inner.token.lock().unwrap() = token.trim().to_string();
    }

    /// Jobs left from a previous run come back paused, with their bytes.
    /// `resume_interrupted` then restarts the ones the learner did not pause.
    fn restore(&self) {
        let Ok(rd) = std::fs::read_dir(&self.inner.dir) else {
            return;
        };
        let mut map = self.inner.entries.lock().unwrap();
        let mut parts = Vec::new();
        for e in rd.flatten() {
            let p = e.path();
            match p.extension().and_then(|x| x.to_str()) {
                Some("part") => {
                    parts.push(p);
                    continue;
                }
                Some("json") => {}
                _ => continue,
            }
            let Some(mut info) = read_job(&p).or_else(|| read_job(&p.with_extension("json.tmp")))
            else {
                continue;
            };
            // The job file is data on disk, not proof: rebuild what it may name.
            let base = info.file.rsplit('/').next().unwrap_or(&info.file);
            match crate::paths::safe_model_file_name(base) {
                Some(n) => info.dest_file = n,
                None => continue,
            }
            if info.id != download_id(&info.repo, &info.file, &info.revision)
                || p.file_stem().and_then(|s| s.to_str()) != Some(info.id.as_str())
            {
                continue;
            }
            // After a power cut the end of the part file may hold bytes that never
            // reached the disk. Keep only what was synced.
            let part = self.part_path(&info.id);
            let len = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
            if len > info.synced
                && let Ok(f) = std::fs::OpenOptions::new().write(true).open(&part)
            {
                let _ = f.set_len(info.synced);
            }
            info.done = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
            info.synced = info.done;
            info.rate = 0;
            if info.status != DownloadStatus::Failed {
                info.status = DownloadStatus::Paused;
            }
            map.insert(info.id.clone(), Entry::new(info));
        }
        // A part file with no job file at all can never be resumed or seen: free
        // the disk. One whose job file exists but cannot be read is left alone:
        // deleting gigabytes on a guess is worse than keeping them.
        for p in parts {
            let id = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            let job = self.job_path(id);
            if !map.contains_key(id) && !job.exists() && !job.with_extension("json.tmp").exists() {
                let _ = std::fs::remove_file(&p);
            }
        }
    }

    /// Carry on with downloads that were running when Trurl last closed. Must be
    /// called inside a tokio runtime. Returns how many were restarted.
    pub fn resume_interrupted(&self) -> usize {
        let ids: Vec<String> = self
            .inner
            .entries
            .lock()
            .unwrap()
            .values()
            .filter(|e| e.info.status == DownloadStatus::Paused && !e.info.user_paused)
            .map(|e| e.info.id.clone())
            .collect();
        ids.iter()
            .filter(|id| self.resume_inner(id, false).is_ok())
            .count()
    }

    pub fn list(&self) -> Vec<DownloadInfo> {
        let map = self.inner.entries.lock().unwrap();
        let mut v: Vec<DownloadInfo> = map.values().map(|e| e.info.clone()).collect();
        v.sort_by(|a, b| a.dest_file.cmp(&b.dest_file));
        v
    }

    fn write_job(&self, info: &DownloadInfo) {
        let _one = self.inner.job_io.lock().unwrap_or_else(|p| p.into_inner());
        write_job(&self.job_path(&info.id), info);
    }

    fn part_path(&self, id: &str) -> PathBuf {
        self.inner.dir.join(format!("{id}.part"))
    }
    fn job_path(&self, id: &str) -> PathBuf {
        self.inner.dir.join(format!("{id}.json"))
    }

    fn emit(&self) {
        (self.inner.notify)(self.list());
    }

    fn update(&self, id: &str, f: impl FnOnce(&mut DownloadInfo)) {
        let to_save = {
            let mut map = self.inner.entries.lock().unwrap();
            let Some(e) = map.get_mut(id) else { return };
            f(&mut e.info);
            let key = (
                e.info.status,
                e.info.synced,
                e.info.user_paused,
                e.info.error.is_some(),
            );
            if e.info.status == DownloadStatus::Done || e.saved == Some(key) {
                None
            } else {
                e.saved = Some(key);
                Some(e.info.clone())
            }
        };
        if let Some(info) = to_save {
            self.write_job(&info);
        }
    }

    /// Start (or resume) a download. Returns its id.
    pub fn start(&self, req: DownloadRequest) -> Result<String, UserError> {
        let base_name = req.file.rsplit('/').next().unwrap_or(&req.file);
        let dest_file = crate::paths::safe_model_file_name(base_name)
            .ok_or_else(|| UserError::not_gguf(base_name))?;
        let id = download_id(&req.repo, &req.file, &req.revision);
        std::fs::create_dir_all(&self.inner.dir)
            .map_err(|e| UserError::from_io_write(&e, UserError::download_failed()))?;
        {
            let mut map = self.inner.entries.lock().unwrap();
            if let Some(e) = map.get(&id)
                && e.cancel.is_some()
            {
                return Ok(id); // already running
            }
            let done = std::fs::metadata(self.part_path(&id))
                .map(|m| m.len())
                .unwrap_or(0);
            // Bytes left by an earlier job of this session were never proven synced.
            let synced = map.get(&id).map_or(0, |e| e.info.synced.min(done));
            let info = DownloadInfo {
                id: id.clone(),
                repo: req.repo,
                file: req.file,
                revision: req.revision,
                dest_file,
                size: req.size,
                sha256: req.sha256.map(|s| s.to_ascii_lowercase()),
                done,
                status: DownloadStatus::Running,
                error: None,
                rate: 0,
                user_paused: false,
                synced,
            };
            map.insert(id.clone(), Entry::new(info));
        }
        self.resume(&id)?;
        Ok(id)
    }

    /// The learner pressed Resume (or Try again).
    pub fn resume(&self, id: &str) -> Result<(), UserError> {
        self.resume_inner(id, true)
    }

    fn resume_inner(&self, id: &str, by_learner: bool) -> Result<(), UserError> {
        let token = CancellationToken::new();
        let info = {
            let mut map = self.inner.entries.lock().unwrap();
            let Some(e) = map.get_mut(id) else {
                return Ok(());
            };
            if e.cancel.is_some() {
                return Ok(());
            }
            e.cancel = Some(token.clone());
            e.info.status = DownloadStatus::Running;
            e.info.error = None;
            if by_learner {
                e.info.user_paused = false;
            }
            e.info.clone()
        };
        let remaining = info.size.saturating_sub(info.done);
        if let Some(free) = crate::hardware::free_space(&self.inner.dir)
            && free < remaining + remaining / 20 + 64 * 1024 * 1024
        {
            self.finish_with_error(id, UserError::disk_full());
            return Err(UserError::disk_full());
        }
        self.write_job(&info);
        self.emit();
        let me = self.clone();
        let id = id.to_string();
        tokio::spawn(async move { me.run(id, token).await });
        Ok(())
    }

    pub fn pause(&self, id: &str) {
        let token = {
            let mut map = self.inner.entries.lock().unwrap();
            let Some(e) = map.get_mut(id) else { return };
            e.info.status = DownloadStatus::Paused;
            e.info.rate = 0;
            e.info.user_paused = true;
            e.cancel.take()
        };
        if let Some(t) = token {
            t.cancel();
        }
        self.update(id, |_| {});
        self.emit();
    }

    pub fn cancel(&self, id: &str) {
        let token = {
            let mut map = self.inner.entries.lock().unwrap();
            map.remove(id).and_then(|e| e.cancel)
        };
        if let Some(t) = token {
            t.cancel();
        }
        let _ = std::fs::remove_file(self.part_path(id));
        let _ = std::fs::remove_file(self.job_path(id));
        self.emit();
    }

    /// Stop everything at app exit. The bytes stay, and these downloads carry
    /// on by themselves at the next start (the learner did not pause them).
    pub fn pause_all(&self) {
        let tokens: Vec<(String, CancellationToken)> = {
            let mut map = self.inner.entries.lock().unwrap();
            map.values_mut()
                .filter_map(|e| {
                    let t = e.cancel.take()?;
                    e.info.status = DownloadStatus::Paused;
                    e.info.rate = 0;
                    Some((e.info.id.clone(), t))
                })
                .collect()
        };
        for (id, t) in tokens {
            t.cancel();
            self.update(&id, |_| {});
        }
    }

    fn finish_with_error(&self, id: &str, err: UserError) {
        {
            let mut map = self.inner.entries.lock().unwrap();
            if let Some(e) = map.get_mut(id) {
                e.cancel = None;
                e.info.rate = 0;
            }
        }
        self.update(id, |i| {
            i.status = DownloadStatus::Failed;
            i.error = Some(err);
        });
        self.emit();
    }

    fn info(&self, id: &str) -> Option<DownloadInfo> {
        self.inner
            .entries
            .lock()
            .unwrap()
            .get(id)
            .map(|e| e.info.clone())
    }

    async fn run(&self, id: String, cancel: CancellationToken) {
        let lock = self
            .inner
            .run_locks
            .lock()
            .unwrap()
            .entry(id.clone())
            .or_default()
            .clone();
        // Wait for a previous task on this download (paused a moment ago) to let go of the file.
        let _running = lock.lock().await;
        if cancel.is_cancelled() {
            return;
        }
        let mut attempt = 0usize;
        let mut blocked = 0usize;
        loop {
            let before = std::fs::metadata(self.part_path(&id))
                .map(|m| m.len())
                .unwrap_or(0);
            let r = self.fetch(&id, &cancel).await;
            if cancel.is_cancelled() {
                return; // paused or cancelled; state already updated
            }
            // Any progress means the connection works: give the next drop a full set of retries.
            let after = std::fs::metadata(self.part_path(&id))
                .map(|m| m.len())
                .unwrap_or(0);
            if after > before {
                attempt = 0;
                blocked = 0;
            }
            match r {
                Ok(()) => break,
                // A dropped connection is the normal case here: keep trying, slower
                // and slower, for as long as Trurl is open. Pause and Cancel still work.
                // "Not the file" (a filter page, a broken proxy): a few tries, then say so.
                Err(e)
                    if (e.code == ErrorCode::DownloadFailed
                        || (e.code == ErrorCode::DownloadBlocked
                            && blocked < self.inner.retry_delays.len()))
                        && !self.inner.retry_delays.is_empty() =>
                {
                    let delays = &self.inner.retry_delays;
                    let delay = delays[attempt.min(delays.len() - 1)];
                    attempt += 1;
                    let why = (e.code == ErrorCode::DownloadBlocked).then(|| {
                        blocked += 1;
                        e.clone()
                    });
                    self.update(&id, |i| {
                        i.status = DownloadStatus::Retrying;
                        i.rate = 0;
                        i.error = why;
                    });
                    self.emit();
                    tokio::select! {
                        _ = tokio::time::sleep(delay) => {}
                        _ = cancel.cancelled() => return,
                    }
                    self.update(&id, |i| {
                        i.status = DownloadStatus::Running;
                        i.error = None;
                    });
                    self.emit();
                }
                Err(e) => {
                    self.finish_with_error(&id, e);
                    return;
                }
            }
        }
        // Complete: verify, then move into the models folder.
        self.update(&id, |i| {
            i.status = DownloadStatus::Verifying;
            i.rate = 0;
        });
        self.emit();
        let Some(info) = self.info(&id) else { return };
        let part = self.part_path(&id);
        let models_dir = self.inner.models_dir.clone();
        let (check_part, check_info) = (part.clone(), info.clone());
        let checked = tokio::task::spawn_blocking(move || verify(&check_part, &check_info)).await;
        if cancel.is_cancelled() {
            // Cancelled or paused while the file was being checked: do not install it.
            if self.info(&id).is_none() {
                let _ = std::fs::remove_file(&part);
            }
            return;
        }
        let verified = match checked {
            Ok(Ok(())) => Ok(install(&part, &info, &models_dir)),
            Ok(Err(e)) => Ok(Err(e)),
            Err(e) => Err(e),
        };
        match verified {
            Ok(Ok(name)) => {
                let _ = std::fs::remove_file(self.job_path(&id));
                {
                    let mut map = self.inner.entries.lock().unwrap();
                    map.remove(&id);
                }
                self.emit();
                (self.inner.on_done)(name);
            }
            Ok(Err(e)) => {
                if e.code == ErrorCode::DownloadCorrupt {
                    let _ = std::fs::remove_file(self.part_path(&id));
                    self.update(&id, |i| i.done = 0);
                }
                self.finish_with_error(&id, e);
            }
            Err(e) => self.finish_with_error(&id, UserError::internal(e.to_string())),
        }
    }

    /// One HTTP attempt: append to the part file until complete or the connection drops.
    async fn fetch(&self, id: &str, cancel: &CancellationToken) -> Result<(), UserError> {
        let Some(info) = self.info(id) else {
            return Ok(());
        };
        let part = self.part_path(id);
        let mut have = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
        if info.size > 0 && have > info.size {
            // Longer than the file can be: the tail is junk, the start may be fine
            // (the check at the end decides).
            std::fs::OpenOptions::new()
                .write(true)
                .open(&part)
                .and_then(|f| f.set_len(info.size))
                .map_err(save_error)?;
            have = info.size;
        }
        if info.size > 0 && have == info.size {
            return Ok(());
        }
        let url = resolve_url(&self.inner.base_url, &info.repo, &info.revision, &info.file);
        let mut rb = self.inner.http.get(&url);
        if have > 0 {
            rb = rb.header(reqwest::header::RANGE, format!("bytes={have}-"));
        }
        let token = self.inner.token.lock().unwrap().clone();
        if !token.is_empty() {
            // reqwest drops this header when a redirect leaves huggingface.co.
            rb = rb.bearer_auth(token);
        }
        let resp = tokio::select! {
            r = rb.send() => r,
            _ = cancel.cancelled() => return Ok(()),
        };
        let resp = resp.map_err(request_error)?;
        let status = resp.status().as_u16();
        // Nothing on disk is touched until the reply is known to be the right file:
        // a school proxy that ignores Range, or a web filter page sent with status
        // 200, must never cost the learner the part already downloaded.
        let length = resp.content_length();
        let start: u64 = match status {
            206 => {
                let cr = resp
                    .headers()
                    .get(reqwest::header::CONTENT_RANGE)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("");
                match content_range_start(cr) {
                    // The server may start earlier than asked: write from where it says.
                    Some(s) if s <= have => s,
                    // Later than asked, or unreadable: appending would leave a hole.
                    _ => {
                        return Err(UserError::download_blocked()
                            .with_detail(format!("unexpected Content-Range {cr:?}")));
                    }
                }
            }
            // The whole file from the start (the server ignored Range). Accept it
            // only if it has exactly the expected size, then write it over the
            // part from byte 0: matching bytes stay as they are.
            200 => {
                let wrong = if info.size > 0 {
                    length != Some(info.size)
                } else {
                    // Size unknown: at least never shorter than what we have.
                    have > 0 && length.is_none_or(|l| l < have)
                };
                if wrong {
                    return Err(UserError::download_blocked().with_detail(format!(
                        "HTTP 200 with {length:?} bytes, expected {}",
                        info.size
                    )));
                }
                0
            }
            416 => {
                // "Nothing left to send": done if the size is unknown or reached;
                // the check after the download decides if the bytes are right.
                if info.size == 0 || have >= info.size {
                    return Ok(());
                }
                return Err(UserError::download_blocked().with_detail("416 on resume"));
            }
            401 | 403 => {
                return Err(UserError::hub_gated(&info.repo).with_detail(format!("HTTP {status}")));
            }
            404 => return Err(UserError::hub_failed().with_detail(format!("HTTP 404 for {url}"))),
            _ => return Err(UserError::download_failed().with_detail(format!("HTTP {status}"))),
        };
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&part)
            .map_err(save_error)?;
        f.seek(SeekFrom::Start(start)).map_err(save_error)?;
        let mut done = start;
        // Bytes known to be on the disk (synced): what a restart after a power cut
        // may trust. Everything past it is checked again or downloaded again.
        // Writing again from an earlier point rewrites the same bytes (the commit is
        // pinned): what was synced before stays synced. It only ever grows.
        let mut synced = info.synced.min(have);
        let mut last_sync = Instant::now();
        self.update(id, |i| {
            i.done = done.max(have);
            i.synced = synced;
        });
        let mut stream = resp.bytes_stream();
        let mut last_emit = Instant::now();
        let mut window_start = Instant::now();
        let mut window_bytes = 0u64;
        loop {
            let next = tokio::select! {
                n = stream.next() => n,
                _ = cancel.cancelled() => {
                    if f.sync_data().is_ok() {
                        synced = synced.max(done);
                    }
                    self.update(id, |i| i.synced = synced);
                    return Ok(());
                }
            };
            match next {
                Some(Ok(chunk)) => {
                    f.write_all(&chunk).map_err(save_error)?;
                    done += chunk.len() as u64;
                    if info.size > 0 && done > info.size {
                        let _ = f.sync_data();
                        return Err(UserError::download_blocked()
                            .with_detail(format!("more than {} bytes", info.size)));
                    }
                    window_bytes += chunk.len() as u64;
                    if last_sync.elapsed() >= Duration::from_secs(5) {
                        f.sync_data().map_err(save_error)?;
                        synced = synced.max(done);
                        last_sync = Instant::now();
                        self.update(id, |i| i.synced = synced);
                    }
                    if last_emit.elapsed() >= Duration::from_millis(300) {
                        let secs = window_start.elapsed().as_secs_f64().max(0.001);
                        let rate = (window_bytes as f64 / secs) as u64;
                        if window_start.elapsed() > Duration::from_secs(3) {
                            window_start = Instant::now();
                            window_bytes = 0;
                        }
                        self.update(id, |i| {
                            i.done = done.max(have);
                            i.rate = rate;
                        });
                        self.emit();
                        last_emit = Instant::now();
                    }
                }
                Some(Err(e)) => {
                    if f.sync_data().is_ok() {
                        synced = synced.max(done);
                    }
                    self.update(id, |i| {
                        i.done = done.max(have);
                        i.synced = synced;
                    });
                    // A server that ignores Range on a link that drops before it gets
                    // back to where we were would loop for ever: say so instead.
                    if start == 0 && have > 0 && done < have {
                        return Err(restarted_short(done, have));
                    }
                    return Err(UserError::download_failed().with_detail(crate::errors::chain(&e)));
                }
                None => break,
            }
        }
        if done < have && info.size > 0 && done >= info.size {
            // A whole file sent over a longer stale part: drop the stale tail.
            f.set_len(done).map_err(save_error)?;
        }
        f.sync_all().map_err(save_error)?;
        let synced = synced.max(done);
        self.update(id, |i| {
            i.done = done
                .max(have)
                .min(if i.size > 0 { i.size } else { u64::MAX });
            i.synced = synced;
            if i.size == 0 {
                i.size = done;
            }
        });
        if info.size > 0 && done < info.size {
            if start == 0 && have > 0 && done < have {
                return Err(restarted_short(done, have));
            }
            return Err(UserError::download_failed()
                .with_detail(format!("connection closed at {done} of {}", info.size)));
        }
        Ok(())
    }
}

/// A request that never got a reply. No internet: keep trying. A secure
/// connection that fails (a wrong clock, an intercepting network): say what to
/// check, no retry can fix it. A proxy that wants a login: a few tries, then say
/// a network may block downloads. Anything else: a drop, keep trying.
fn request_error(e: reqwest::Error) -> UserError {
    let detail = crate::errors::chain(&e);
    match crate::net::classify(&e) {
        crate::net::NetFailure::Offline => UserError::download_failed().with_detail(detail),
        crate::net::NetFailure::Tls => UserError::hub_tls().with_detail(detail),
        // A proxy that refuses to pass the request on (it wants a login): no
        // retry fixes that. Every other failure here is a dropped connection.
        crate::net::NetFailure::Other if crate::net::is_proxy_refusal(&e) => {
            UserError::download_blocked().with_detail(detail)
        }
        crate::net::NetFailure::Other => UserError::download_failed().with_detail(detail),
    }
}

/// The server ignored Range, sent the file from byte 0, and the connection
/// dropped before it got back to where the download was: on a link that drops
/// this would never finish.
fn restarted_short(done: u64, have: u64) -> UserError {
    UserError::download_blocked().with_detail(format!(
        "the server restarts from the beginning and the connection dropped at {done} of the {have} bytes we had"
    ))
}

fn save_error(e: std::io::Error) -> UserError {
    UserError::from_io_write(&e, UserError::download_save_failed())
}

/// Write the job file so that a power cut leaves the old one or the new one,
/// never a half-written or empty one: temporary file, synced, then renamed.
fn write_job(path: &Path, info: &DownloadInfo) {
    let tmp = path.with_extension("json.tmp");
    let ok = std::fs::File::create(&tmp)
        .and_then(|mut f| {
            f.write_all(&serde_json::to_vec(info).unwrap_or_default())?;
            f.sync_all()
        })
        .is_ok();
    if ok {
        let _ = std::fs::rename(&tmp, path);
    }
}

/// A job file from disk. Files from before `synced` existed trust the part file.
fn read_job(path: &Path) -> Option<DownloadInfo> {
    let mut v: serde_json::Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    let had_synced = v.get("synced").is_some();
    let mut info: DownloadInfo = match serde_json::from_value(v.clone()) {
        Ok(i) => i,
        // An error this version does not know (written by a newer Trurl): drop it.
        Err(_) => {
            v.as_object_mut()?.remove("error");
            serde_json::from_value(v).ok()?
        }
    };
    if !had_synced {
        info.synced = u64::MAX;
    }
    Some(info)
}

/// `bytes 100-199/200` -> Some(100)
pub fn content_range_start(v: &str) -> Option<u64> {
    let rest = v.trim().strip_prefix("bytes")?.trim_start();
    let (range, _) = rest.split_once('/')?;
    let (s, _) = range.split_once('-')?;
    s.trim().parse().ok()
}

fn verify(part: &Path, info: &DownloadInfo) -> Result<(), UserError> {
    if let Some(expected) = &info.sha256 {
        let got = crate::models::sha256_file(part, |_| {})
            .map_err(|e| UserError::download_failed().with_detail(e.to_string()))?;
        if &got != expected {
            return Err(UserError::download_corrupt(&info.dest_file)
                .with_detail(format!("sha256 {got} != {expected}")));
        }
    }
    if let Err(e) = crate::gguf::read(part) {
        return Err(UserError::download_corrupt(&info.dest_file).with_detail(e.to_string()));
    }
    Ok(())
}

/// Move a verified file into the models folder under a name nobody else holds.
/// The name is claimed by creating it (create_new is atomic), then replaced by
/// the download, so two downloads with the same name never overwrite each other.
fn install(part: &Path, info: &DownloadInfo, models_dir: &Path) -> Result<String, UserError> {
    std::fs::create_dir_all(models_dir)
        .map_err(|e| UserError::from_io_write(&e, UserError::download_failed()))?;
    let dest = claim_dest(models_dir, &info.dest_file)
        .map_err(|e| UserError::from_io_write(&e, UserError::download_failed()))?;
    if std::fs::rename(part, &dest).is_err() {
        // Different volume (TRURL_HOME split): copy then remove.
        if let Err(e) = std::fs::copy(part, &dest) {
            let _ = std::fs::remove_file(&dest);
            return Err(UserError::from_io_write(&e, UserError::download_failed()));
        }
        let _ = std::fs::remove_file(part);
    }
    Ok(dest.file_name().unwrap().to_string_lossy().into_owned())
}

pub fn claim_dest(dir: &Path, file: &str) -> std::io::Result<PathBuf> {
    let stem = file.trim_end_matches(".gguf");
    for i in 1..10_000 {
        let name = if i == 1 {
            file.to_string()
        } else {
            format!("{stem}-{i}.gguf")
        };
        let p = dir.join(name);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&p)
        {
            Ok(_) => return Ok(p),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::other("no free file name"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_are_encoded_per_segment() {
        assert_eq!(
            resolve_url(
                "https://huggingface.co/",
                "ggml-org/gemma-3-1b-it-GGUF",
                "abc123",
                "sub dir/m+1.gguf"
            ),
            "https://huggingface.co/ggml-org/gemma-3-1b-it-GGUF/resolve/abc123/sub%20dir/m%2B1.gguf"
        );
    }

    #[test]
    fn content_range_parsing() {
        assert_eq!(content_range_start("bytes 100-199/200"), Some(100));
        assert_eq!(content_range_start("bytes 0-0/1"), Some(0));
        assert_eq!(content_range_start("bytes */200"), None);
        assert_eq!(content_range_start("garbage"), None);
    }

    #[test]
    fn claiming_a_name_never_reuses_one() {
        let d = tempfile::tempdir().unwrap();
        let a = claim_dest(d.path(), "m.gguf").unwrap();
        let b = claim_dest(d.path(), "m.gguf").unwrap();
        assert_ne!(a, b);
        assert_eq!(b.file_name().unwrap(), "m-2.gguf");
    }

    #[test]
    fn ids_are_stable_and_distinct() {
        assert_eq!(
            download_id("a/b", "f.gguf", "r"),
            download_id("a/b", "f.gguf", "r")
        );
        assert_ne!(
            download_id("a/b", "f.gguf", "r"),
            download_id("a/b", "f.gguf", "s")
        );
    }
}
