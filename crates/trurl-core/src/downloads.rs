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
    cancel: Option<CancellationToken>,
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
    fn restore(&self) {
        let Ok(rd) = std::fs::read_dir(&self.inner.dir) else {
            return;
        };
        let mut map = self.inner.entries.lock().unwrap();
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("json") {
                continue;
            }
            let Some(mut info) = std::fs::read_to_string(&p)
                .ok()
                .and_then(|t| serde_json::from_str::<DownloadInfo>(&t).ok())
            else {
                continue;
            };
            info.done = std::fs::metadata(self.part_path(&info.id))
                .map(|m| m.len())
                .unwrap_or(0);
            info.rate = 0;
            if info.status != DownloadStatus::Failed {
                info.status = DownloadStatus::Paused;
            }
            map.insert(info.id.clone(), Entry { info, cancel: None });
        }
    }

    pub fn list(&self) -> Vec<DownloadInfo> {
        let map = self.inner.entries.lock().unwrap();
        let mut v: Vec<DownloadInfo> = map.values().map(|e| e.info.clone()).collect();
        v.sort_by(|a, b| a.dest_file.cmp(&b.dest_file));
        v
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
        let snapshot = {
            let mut map = self.inner.entries.lock().unwrap();
            let Some(e) = map.get_mut(id) else { return };
            f(&mut e.info);
            e.info.clone()
        };
        if snapshot.status != DownloadStatus::Done {
            let _ = std::fs::write(
                self.job_path(id),
                serde_json::to_vec(&snapshot).unwrap_or_default(),
            );
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
            };
            map.insert(id.clone(), Entry { info, cancel: None });
        }
        self.resume(&id)?;
        Ok(id)
    }

    pub fn resume(&self, id: &str) -> Result<(), UserError> {
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
            e.info.clone()
        };
        let remaining = info.size.saturating_sub(info.done);
        if let Some(free) = crate::hardware::free_space(&self.inner.dir)
            && free < remaining + remaining / 20 + 64 * 1024 * 1024
        {
            self.finish_with_error(id, UserError::disk_full());
            return Err(UserError::disk_full());
        }
        let _ = std::fs::write(
            self.job_path(id),
            serde_json::to_vec(&info).unwrap_or_default(),
        );
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

    /// Pause everything (app exit). The bytes stay for next time.
    pub fn pause_all(&self) {
        let ids: Vec<String> = self.inner.entries.lock().unwrap().keys().cloned().collect();
        for id in ids {
            let running = self
                .inner
                .entries
                .lock()
                .unwrap()
                .get(&id)
                .map(|e| e.cancel.is_some())
                .unwrap_or(false);
            if running {
                self.pause(&id);
            }
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
        let mut attempt = 0usize;
        loop {
            let r = self.fetch(&id, &cancel).await;
            if cancel.is_cancelled() {
                return; // paused or cancelled; state already updated
            }
            match r {
                Ok(()) => break,
                Err(e)
                    if e.code == ErrorCode::DownloadFailed
                        && attempt < self.inner.retry_delays.len() =>
                {
                    let delay = self.inner.retry_delays[attempt];
                    attempt += 1;
                    self.update(&id, |i| {
                        i.status = DownloadStatus::Retrying;
                        i.rate = 0;
                    });
                    self.emit();
                    tokio::select! {
                        _ = tokio::time::sleep(delay) => {}
                        _ = cancel.cancelled() => return,
                    }
                    self.update(&id, |i| i.status = DownloadStatus::Running);
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
        let verified =
            tokio::task::spawn_blocking(move || verify_and_install(&part, &info, &models_dir))
                .await;
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
            std::fs::write(&part, b"")
                .map_err(|e| UserError::from_io_write(&e, UserError::download_failed()))?;
            have = 0;
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
        let resp = resp.map_err(|e| UserError::download_failed().with_detail(e.to_string()))?;
        let status = resp.status().as_u16();
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
                    // Later than asked, or unreadable: we would leave a hole. Start over.
                    _ => {
                        std::fs::write(&part, b"").map_err(|e| {
                            UserError::from_io_write(&e, UserError::download_failed())
                        })?;
                        return Err(UserError::download_failed()
                            .with_detail(format!("unexpected Content-Range {cr:?}")));
                    }
                }
            }
            200 => 0,
            416 => {
                if info.size > 0 && have >= info.size {
                    return Ok(());
                }
                std::fs::write(&part, b"")
                    .map_err(|e| UserError::from_io_write(&e, UserError::download_failed()))?;
                return Err(UserError::download_failed().with_detail("416 on resume; restarting"));
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
            .map_err(|e| UserError::from_io_write(&e, UserError::download_failed()))?;
        f.set_len(start)
            .map_err(|e| UserError::from_io_write(&e, UserError::download_failed()))?;
        f.seek(SeekFrom::Start(start))
            .map_err(|e| UserError::from_io_write(&e, UserError::download_failed()))?;
        let mut done = start;
        self.update(id, |i| i.done = done);
        let mut stream = resp.bytes_stream();
        let mut last_emit = Instant::now();
        let mut window_start = Instant::now();
        let mut window_bytes = 0u64;
        loop {
            let next = tokio::select! {
                n = stream.next() => n,
                _ = cancel.cancelled() => {
                    let _ = f.flush();
                    return Ok(());
                }
            };
            match next {
                Some(Ok(chunk)) => {
                    f.write_all(&chunk)
                        .map_err(|e| UserError::from_io_write(&e, UserError::download_failed()))?;
                    done += chunk.len() as u64;
                    window_bytes += chunk.len() as u64;
                    if last_emit.elapsed() >= Duration::from_millis(300) {
                        let secs = window_start.elapsed().as_secs_f64().max(0.001);
                        let rate = (window_bytes as f64 / secs) as u64;
                        if window_start.elapsed() > Duration::from_secs(3) {
                            window_start = Instant::now();
                            window_bytes = 0;
                        }
                        self.update(id, |i| {
                            i.done = done;
                            i.rate = rate;
                        });
                        self.emit();
                        last_emit = Instant::now();
                    }
                }
                Some(Err(e)) => {
                    let _ = f.flush();
                    self.update(id, |i| i.done = done);
                    return Err(UserError::download_failed().with_detail(e.to_string()));
                }
                None => break,
            }
        }
        f.sync_all()
            .map_err(|e| UserError::from_io_write(&e, UserError::download_failed()))?;
        self.update(id, |i| {
            i.done = done;
            if i.size == 0 {
                i.size = done;
            }
        });
        if info.size > 0 && done < info.size {
            return Err(UserError::download_failed()
                .with_detail(format!("connection closed at {done} of {}", info.size)));
        }
        Ok(())
    }
}

/// `bytes 100-199/200` -> Some(100)
pub fn content_range_start(v: &str) -> Option<u64> {
    let rest = v.trim().strip_prefix("bytes")?.trim_start();
    let (range, _) = rest.split_once('/')?;
    let (s, _) = range.split_once('-')?;
    s.trim().parse().ok()
}

fn verify_and_install(
    part: &Path,
    info: &DownloadInfo,
    models_dir: &Path,
) -> Result<String, UserError> {
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
    std::fs::create_dir_all(models_dir)
        .map_err(|e| UserError::from_io_write(&e, UserError::download_failed()))?;
    let dest = crate::models::unique_dest(models_dir, &info.dest_file);
    if std::fs::rename(part, &dest).is_err() {
        // Different volume (TRURL_HOME split): copy then remove.
        std::fs::copy(part, &dest)
            .map_err(|e| UserError::from_io_write(&e, UserError::download_failed()))?;
        let _ = std::fs::remove_file(part);
    }
    Ok(dest.file_name().unwrap().to_string_lossy().into_owned())
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
