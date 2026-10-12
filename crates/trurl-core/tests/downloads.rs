//! Resumable downloads against a local server that misbehaves the way real
//! connections do: drops mid-file, ignores Range, answers the wrong range,
//! asks for a token, serves a corrupted file.

use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::Response;
use axum::routing::get;
use bytes::Bytes;
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use trurl_core::downloads::{DownloadInfo, DownloadRequest, DownloadStatus, Downloads};
use trurl_core::errors::ErrorCode;

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Good,
    /// Close the connection after this many bytes, on the first N requests.
    DropAfter(usize, usize),
    /// Ignore Range and send the whole file with 200.
    IgnoreRange,
    /// Drop the first request, then answer the resume with a 206 that starts
    /// earlier than asked (allowed by HTTP: the client must honour Content-Range).
    WrongRange,
    NeedToken,
    /// Serve bytes that do not match the hash.
    Corrupt,
}

struct Srv {
    data: Vec<u8>,
    mode: Mode,
    requests: AtomicUsize,
    ranges: Mutex<Vec<Option<String>>>,
}

async fn serve(State(s): State<Arc<Srv>>, headers: HeaderMap) -> Response {
    let n = s.requests.fetch_add(1, Ordering::SeqCst);
    let range = headers.get(header::RANGE).and_then(|v| v.to_str().ok()).map(str::to_string);
    s.ranges.lock().unwrap().push(range.clone());
    if s.mode == Mode::NeedToken && headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()) != Some("Bearer hf_secret") {
        return Response::builder().status(401).body(Body::empty()).unwrap();
    }
    let total = s.data.len();
    let mut start = range
        .as_deref()
        .and_then(|r| r.strip_prefix("bytes="))
        .and_then(|r| r.trim_end_matches('-').parse::<usize>().ok())
        .unwrap_or(0);
    let mut data = s.data.clone();
    if s.mode == Mode::Corrupt {
        data[total / 2] ^= 0xFF;
    }
    let mut status = if start > 0 { StatusCode::PARTIAL_CONTENT } else { StatusCode::OK };
    if s.mode == Mode::IgnoreRange {
        start = 0;
        status = StatusCode::OK;
    }
    if start >= total && start > 0 {
        return Response::builder().status(416).body(Body::empty()).unwrap();
    }
    let mut range_start = start;
    if s.mode == Mode::WrongRange && start > 0 && n == 1 {
        range_start = start / 2;
    }
    let body_bytes = data[range_start..].to_vec();
    let mut b = Response::builder().status(status).header(header::CONTENT_LENGTH, body_bytes.len());
    if status == StatusCode::PARTIAL_CONTENT {
        b = b.header(header::CONTENT_RANGE, format!("bytes {}-{}/{}", range_start, total - 1, total));
    }
    let drop_rule = match s.mode {
        Mode::DropAfter(cut, times) => Some((cut, times)),
        Mode::WrongRange => Some((700_000, 1)),
        _ => None,
    };
    if let Some((cut, times)) = drop_rule {
        if n < times {
            let first = Bytes::from(body_bytes[..cut.min(body_bytes.len())].to_vec());
            let stream = futures_util::stream::iter(vec![
                Ok::<Bytes, std::io::Error>(first),
                Err(std::io::Error::new(std::io::ErrorKind::ConnectionReset, "wifi dropped")),
            ]);
            return b.body(Body::from_stream(stream)).unwrap();
        }
    }
    b.body(Body::from(body_bytes)).unwrap()
}

struct Setup {
    downloads: Downloads,
    srv: Arc<Srv>,
    models: tempfile::TempDir,
    dl_dir: tempfile::TempDir,
    seen: Arc<Mutex<Vec<DownloadInfo>>>,
    done: Arc<Mutex<Vec<String>>>,
    req: DownloadRequest,
}

fn model_bytes() -> Vec<u8> {
    let mut b = trurl_core::gguf::testfile::chat_model("Test");
    // Pad to ~3 MB so there are several chunks to drop in the middle of.
    b.extend(std::iter::repeat_n(7u8, 3 * 1024 * 1024));
    b
}

async fn setup(mode: Mode) -> Setup {
    let data = model_bytes();
    let sha = hex::encode(Sha256::digest(&data));
    let srv = Arc::new(Srv { data: data.clone(), mode, requests: AtomicUsize::new(0), ranges: Mutex::new(vec![]) });
    let app = Router::new().route("/{owner}/{repo}/resolve/{rev}/{*file}", get(serve)).with_state(srv.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let models = tempfile::tempdir().unwrap();
    let dl_dir = tempfile::tempdir().unwrap();
    let seen: Arc<Mutex<Vec<DownloadInfo>>> = Arc::default();
    let done: Arc<Mutex<Vec<String>>> = Arc::default();
    let (s2, d2) = (seen.clone(), done.clone());
    let downloads = Downloads::new(
        dl_dir.path().into(),
        models.path().into(),
        format!("http://{addr}"),
        reqwest::Client::builder().read_timeout(Duration::from_secs(5)).build().unwrap(),
        Arc::new(move |l| s2.lock().unwrap().extend(l)),
        Arc::new(move |f| d2.lock().unwrap().push(f)),
    )
    .with_retry_delays(vec![Duration::from_millis(50); 5]);
    let req = DownloadRequest {
        repo: "someone/tiny-GGUF".into(),
        file: "tiny-Q4_K_M.gguf".into(),
        revision: "0123456789abcdef0123456789abcdef01234567".into(),
        size: data.len() as u64,
        sha256: Some(sha),
    };
    Setup { downloads, srv, models, dl_dir, seen, done, req }
}

async fn wait_done(s: &Setup) -> Result<String, DownloadInfo> {
    for _ in 0..200 {
        if let Some(f) = s.done.lock().unwrap().first().cloned() {
            return Ok(f);
        }
        if let Some(fail) = s.downloads.list().into_iter().find(|d| d.status == DownloadStatus::Failed) {
            return Err(fail);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("download neither finished nor failed; list: {:?}", s.downloads.list());
}

fn assert_installed(s: &Setup, name: &str) {
    let got = std::fs::read(s.models.path().join(name)).unwrap();
    assert_eq!(got, s.srv.data, "installed file differs");
    let leftovers: Vec<_> = std::fs::read_dir(s.dl_dir.path()).unwrap().flatten().map(|e| e.file_name()).collect();
    assert!(leftovers.is_empty(), "download folder should be empty, has {leftovers:?}");
}

#[tokio::test]
async fn clean_download_is_verified_and_installed() {
    let s = setup(Mode::Good).await;
    s.downloads.start(s.req.clone()).unwrap();
    let name = wait_done(&s).await.unwrap();
    assert_eq!(name, "tiny-Q4_K_M.gguf");
    assert_installed(&s, &name);
    assert!(s.seen.lock().unwrap().iter().any(|d| d.status == DownloadStatus::Verifying));
}

#[tokio::test]
async fn dropped_connections_resume_with_range() {
    let s = setup(Mode::DropAfter(700_000, 3)).await;
    s.downloads.start(s.req.clone()).unwrap();
    let name = wait_done(&s).await.unwrap();
    assert_installed(&s, &name);
    let ranges = s.srv.ranges.lock().unwrap().clone();
    assert_eq!(ranges[0], None);
    assert_eq!(ranges[1].as_deref(), Some("bytes=700000-"));
    assert_eq!(ranges[2].as_deref(), Some("bytes=1400000-"));
    assert!(s.seen.lock().unwrap().iter().any(|d| d.status == DownloadStatus::Retrying));
}

#[tokio::test]
async fn server_ignoring_range_restarts_cleanly() {
    let s = setup(Mode::IgnoreRange).await;
    std::fs::write(s.dl_dir.path().join(format!("{}.part", trurl_core::downloads::download_id(&s.req.repo, &s.req.file, &s.req.revision))), b"GGUFpartial-garbage").unwrap();
    s.downloads.start(s.req.clone()).unwrap();
    let name = wait_done(&s).await.unwrap();
    assert_installed(&s, &name);
}

#[tokio::test]
async fn wrong_content_range_is_not_appended() {
    let s = setup(Mode::WrongRange).await;
    s.downloads.start(s.req.clone()).unwrap();
    let name = wait_done(&s).await.unwrap();
    assert_installed(&s, &name);
    assert_eq!(s.srv.ranges.lock().unwrap()[1].as_deref(), Some("bytes=700000-"));
}

#[tokio::test]
async fn pause_keeps_bytes_and_restart_restores_paused() {
    let s = setup(Mode::DropAfter(1_000_000, 100)).await; // never completes on its own
    let id = s.downloads.start(s.req.clone()).unwrap();
    for _ in 0..100 {
        if s.downloads.list().iter().any(|d| d.done >= 1_000_000) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    s.downloads.pause(&id);
    let part = s.dl_dir.path().join(format!("{id}.part"));
    let have = std::fs::metadata(&part).unwrap().len();
    assert!(have >= 1_000_000, "kept {have} bytes");
    // "Restart the app": a new manager over the same folder sees the job, paused, with its bytes.
    let again = Downloads::new(
        s.dl_dir.path().into(),
        s.models.path().into(),
        "http://127.0.0.1:9".into(),
        reqwest::Client::new(),
        Arc::new(|_| {}),
        Arc::new(|_| {}),
    );
    let l = again.list();
    assert_eq!(l.len(), 1);
    assert_eq!(l[0].status, DownloadStatus::Paused);
    assert!(l[0].done >= 1_000_000);
    assert_eq!(l[0].dest_file, "tiny-Q4_K_M.gguf");
}

#[tokio::test]
async fn cancel_deletes_everything() {
    let s = setup(Mode::DropAfter(500_000, 100)).await;
    let id = s.downloads.start(s.req.clone()).unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    s.downloads.cancel(&id);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(s.downloads.list().is_empty());
    assert_eq!(std::fs::read_dir(s.dl_dir.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn corrupted_file_is_rejected_and_removed() {
    let s = setup(Mode::Corrupt).await;
    s.downloads.start(s.req.clone()).unwrap();
    let fail = wait_done(&s).await.unwrap_err();
    assert_eq!(fail.error.unwrap().code, ErrorCode::DownloadCorrupt);
    assert_eq!(std::fs::read_dir(s.models.path()).unwrap().count(), 0, "nothing reaches the models folder");
}

#[tokio::test]
async fn gated_repo_asks_for_a_token_then_works_with_it() {
    let s = setup(Mode::NeedToken).await;
    s.downloads.start(s.req.clone()).unwrap();
    let fail = wait_done(&s).await.unwrap_err();
    assert_eq!(fail.error.unwrap().code, ErrorCode::HubGated);
    s.downloads.set_token("hf_secret");
    s.downloads.resume(&fail.id).unwrap();
    let name = wait_done(&s).await.unwrap();
    assert_installed(&s, &name);
}

#[tokio::test]
async fn unsafe_remote_names_are_flattened() {
    let mut s = setup(Mode::Good).await;
    s.req.file = "sub dir/../Modèle-Q4_K_M.gguf".into();
    s.downloads.start(s.req.clone()).unwrap();
    let name = wait_done(&s).await.unwrap();
    assert_eq!(name, "Mod_le-Q4_K_M.gguf");
    assert!(s.models.path().join(&name).exists());
}
