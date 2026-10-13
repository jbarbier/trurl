//! The course models: one click downloads exactly the pinned file (repo, commit,
//! name, size, SHA-256 from pins.json), so a learner cannot end up with a
//! look-alike from a Hugging Face search.

use axum::Router;
use axum::http::Uri;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use trurl_core::app::{App, AppConfig};
use trurl_core::paths::Paths;
use trurl_core::pins::course_models;

struct Started {
    app: App,
    paths: Paths,
    asked: Arc<Mutex<Vec<String>>>,
    _home: tempfile::TempDir,
}

/// A Trurl whose "Hugging Face" is a local server that writes down every path
/// it is asked for and answers 404.
async fn start() -> Started {
    let asked: Arc<Mutex<Vec<String>>> = Arc::default();
    let a2 = asked.clone();
    let router = Router::new().fallback(move |uri: Uri| {
        let a = a2.clone();
        async move {
            a.lock().unwrap().push(uri.path().to_string());
            axum::http::StatusCode::NOT_FOUND
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

    let home = tempfile::tempdir().unwrap();
    let paths = Paths::at(home.path());
    std::fs::create_dir_all(paths.models()).unwrap();
    let app = App::start(
        AppConfig {
            paths: paths.clone(),
            llama_root: None,
            hf_base: format!("http://{addr}"),
            user_agent: "trurl-test".into(),
        },
        Arc::new(|_| {}),
    );
    Started {
        app,
        paths,
        asked,
        _home: home,
    }
}

#[tokio::test]
async fn the_snapshot_lists_the_course_models_in_course_order() {
    let s = start().await;
    let snap = s.app.snapshot();
    let got: Vec<(&str, &str, u64)> = snap
        .course_models
        .iter()
        .map(|m| (m.id.as_str(), m.file.as_str(), m.size))
        .collect();
    let want: Vec<(&str, &str, u64)> = course_models()
        .map(|m| (m.id.as_str(), m.file.as_str(), m.size))
        .collect();
    assert_eq!(got, want);
    assert_eq!(
        got.iter().map(|g| g.0).collect::<Vec<_>>(),
        ["gemma-3-1b-it", "gemma-3-1b-pt", "qwen3-0.6b"]
    );
    assert!(snap.course_models.iter().all(|m| !m.note.is_empty()));
    // Gemma 3 4B is pinned (USB sticks carry it) but the course does not use it.
    assert!(!got.iter().any(|g| g.0 == "gemma-3-4b-it"));
    s.app.shutdown().await;
}

#[tokio::test]
async fn one_click_asks_for_the_pinned_commit_and_file_and_checks_the_pinned_hash() {
    for pin in course_models() {
        let s = start().await;
        let id = s.app.download_course_model(&pin.id).unwrap();
        let listed = s.app.snapshot().course_models;
        let c = listed.iter().find(|c| c.id == pin.id).unwrap();
        assert_eq!(c.download_id, id, "the snapshot names the pinned download");
        let want = format!("/{}/resolve/{}/{}", pin.repo, pin.revision, pin.file);
        let mut seen = Vec::new();
        for _ in 0..100 {
            seen = s.asked.lock().unwrap().clone();
            if !seen.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
        assert_eq!(seen.first(), Some(&want), "{}", pin.id);
        let d = s
            .app
            .downloads_list()
            .into_iter()
            .find(|d| d.id == id)
            .unwrap();
        assert_eq!(d.repo, pin.repo);
        assert_eq!(d.revision, pin.revision);
        assert_eq!(d.dest_file, pin.file);
        assert_eq!(d.size, pin.size);
        assert_eq!(d.sha256.as_deref(), Some(pin.sha256.as_str()));
        s.app.download_cancel(&id);
        s.app.shutdown().await;
    }
}

#[tokio::test]
async fn an_unknown_or_non_course_id_downloads_nothing() {
    let s = start().await;
    assert!(s.app.download_course_model("someone/else").is_err());
    // Pinned, but not a course model.
    assert!(s.app.download_course_model("gemma-3-4b-it").is_err());
    assert!(s.app.downloads_list().is_empty());
    s.app.shutdown().await;
}

#[tokio::test]
async fn a_course_model_already_here_is_not_downloaded_again() {
    let s = start().await;
    let pin = course_models().find(|m| m.id == "qwen3-0.6b").unwrap();
    // Same name and size as the pin (sparse: no 484 MB written).
    let f = std::fs::File::create(s.paths.models().join(&pin.file)).unwrap();
    f.set_len(pin.size).unwrap();
    drop(f);
    let e = s.app.download_course_model(&pin.id).unwrap_err();
    assert_eq!(e.title, "This model is already on this computer.");
    assert!(s.asked.lock().unwrap().is_empty());
    s.app.shutdown().await;
}
