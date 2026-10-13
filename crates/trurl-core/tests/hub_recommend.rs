//! The Hugging Face screen recommends the file that is fastest with the
//! llama-server build that actually runs on this computer, not the one the
//! processor was detected for.

use axum::Router;
use axum::routing::get;
use std::sync::Arc;
use trurl_core::app::{App, AppConfig};
use trurl_core::hardware::CpuVariant;
use trurl_core::paths::Paths;
use trurl_core::settings::Settings;

const REPO: &str = r#"{"id":"someone/tiny-GGUF","sha":"0123456789abcdef0123456789abcdef01234567",
  "siblings":[
    {"rfilename":"tiny-Q8_0.gguf","size":2000,"lfs":{"sha256":"aa","size":2000}},
    {"rfilename":"tiny-Q4_K_M.gguf","size":1200,"lfs":{"sha256":"bb","size":1200}},
    {"rfilename":"tiny-Q4_0.gguf","size":1100,"lfs":{"sha256":"cc","size":1100}}
  ]}"#;

async fn recommended_with(max_variant: Option<CpuVariant>) -> String {
    let app_router = Router::new().route(
        "/api/models/{owner}/{repo}",
        get(|| async { ([("content-type", "application/json")], REPO) }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app_router).await.unwrap() });

    let home = tempfile::tempdir().unwrap();
    let paths = Paths::at(home.path());
    std::fs::create_dir_all(paths.models()).unwrap();
    let settings = Settings {
        max_variant,
        ..Settings::default()
    };
    settings.save(&paths.settings()).unwrap();
    let app = App::start(
        AppConfig {
            paths,
            llama_root: None,
            hf_base: format!("http://{addr}"),
            user_agent: "trurl-test".into(),
        },
        Arc::new(|_| {}),
    );
    let d = app.hub_details("someone/tiny-GGUF").await.unwrap();
    app.shutdown().await;
    let rec: Vec<&str> = d
        .files
        .iter()
        .filter(|f| f.recommended)
        .map(|f| f.path.as_str())
        .collect();
    assert_eq!(rec.len(), 1, "exactly one recommended file: {rec:?}");
    assert_eq!(
        d.files[0].path, rec[0],
        "the recommended file is listed first"
    );
    rec[0].to_string()
}

#[tokio::test]
async fn the_build_that_runs_decides_not_the_processor() {
    // A processor whose faster builds crashed runs a slower one (the cap is
    // saved in the settings): the recommendation follows that build.
    assert_eq!(
        recommended_with(Some(CpuVariant::Ssse3)).await,
        "tiny-Q4_0.gguf"
    );
    assert_eq!(
        recommended_with(Some(CpuVariant::Compat)).await,
        "tiny-Q4_0.gguf"
    );
    let detected = trurl_core::hardware::CpuFeatures::detect().variant();
    if detected >= CpuVariant::Avx {
        assert_eq!(
            recommended_with(Some(CpuVariant::Avx)).await,
            "tiny-Q4_K_M.gguf"
        );
    }
}

#[tokio::test]
async fn without_a_cap_the_processor_decides() {
    let want = if trurl_core::hardware::CpuFeatures::detect().variant() >= CpuVariant::Avx {
        "tiny-Q4_K_M.gguf"
    } else {
        "tiny-Q4_0.gguf"
    };
    assert_eq!(recommended_with(None).await, want);
}
