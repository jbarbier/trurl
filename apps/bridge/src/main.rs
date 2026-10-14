//! Dev bridge: serves the desktop UI to a normal browser, backed by the real
//! trurl-core. Used to test and screenshot the UI on any OS, and by the e2e
//! suite. Not shipped to learners.
//!
//!   cargo run -p trurl-bridge -- [--port 7861] [--ui apps/desktop/ui]
//!   then open http://127.0.0.1:7861/
//!
//! Commands: POST /api/<command> with a JSON body (contracts/events.md).
//! Events:   GET /events (Server-Sent Events), a snapshot first, then live.

use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde_json::Value;
use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::{Stream, StreamExt};
use trurl_core::app::{App, AppConfig};

#[derive(Clone)]
struct Ctx {
    app: App,
    tx: broadcast::Sender<String>,
}

/// Only this page may drive the bridge. Without this, any web site open in the
/// same browser could send it commands, and DNS rebinding (a hostile name that
/// resolves to 127.0.0.1) could read the answers too.
fn same_origin(headers: &axum::http::HeaderMap, port: u16) -> bool {
    let ok_host = |h: &str| h == format!("127.0.0.1:{port}") || h == format!("localhost:{port}");
    let header = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
    let host_ok = header("host").is_some_and(ok_host);
    let origin_ok = match header("origin") {
        None => true,
        Some(o) => o.strip_prefix("http://").is_some_and(ok_host),
    };
    host_ok && origin_ok
}

async fn same_origin_only(
    State(port): State<u16>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let ok = same_origin(req.headers(), port);
    if ok {
        next.run(req).await
    } else {
        (StatusCode::FORBIDDEN, "not from the Trurl page").into_response()
    }
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let flag = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let port: u16 = flag("--port").and_then(|p| p.parse().ok()).unwrap_or(7861);
    let ui: PathBuf = flag("--ui")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../desktop/ui"));

    let (tx, _) = broadcast::channel::<String>(4096);
    let tx2 = tx.clone();
    let app = App::start(
        AppConfig::from_env("bridge"),
        Arc::new(move |e| {
            if let Ok(s) = serde_json::to_string(&e) {
                // Engine changes on stderr: a test that waits for "Ready" can be
                // diagnosed from the log alone.
                if matches!(e, trurl_core::events::Event::Engine { .. }) {
                    let t = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs_f64())
                        .unwrap_or(0.0);
                    eprintln!("{t:.1} {s}");
                }
                let _ = tx2.send(s);
            }
        }),
    );
    let ctx = Ctx {
        app: app.clone(),
        tx,
    };

    let router = Router::new()
        .route("/api/{cmd}", post(command))
        .route("/events", get(events))
        .fallback_service(tower_http::services::ServeDir::new(&ui))
        .with_state(ctx)
        .layer(axum::middleware::from_fn_with_state(port, same_origin_only));

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .expect("bind");
    eprintln!(
        "Trurl bridge on http://127.0.0.1:{port}/  (UI from {})",
        ui.display()
    );
    let shutdown_app = app.clone();
    axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            let _ = tokio::signal::ctrl_c().await;
            shutdown_app.shutdown().await;
        })
        .await
        .expect("serve");
}

async fn command(
    State(ctx): State<Ctx>,
    Path(cmd): Path<String>,
    body: Option<axum::Json<Value>>,
) -> Response {
    let args = body.map(|b| b.0).unwrap_or(Value::Null);
    let result = match cmd.as_str() {
        // Desktop-only commands: the bridge has no updater and no file manager.
        "update_check" | "update_status" => Ok(Value::Null),
        "update_install" | "open_models_folder" => Ok(Value::Null),
        _ => trurl_core::api::dispatch(&ctx.app, &cmd, args).await,
    };
    match result {
        Ok(v) => axum::Json(v).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, axum::Json(e)).into_response(),
    }
}

async fn events(State(ctx): State<Ctx>) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    // Subscribe before taking the snapshot so no event falls in between.
    let rx = ctx.tx.subscribe();
    let first =
        serde_json::json!({ "type": "snapshot", "snapshot": ctx.app.snapshot() }).to_string();
    let live = BroadcastStream::new(rx)
        .filter_map(|r| r.ok())
        .map(|s| Ok(SseEvent::default().data(s)));
    Sse::new(tokio_stream::once(Ok(SseEvent::default().data(first))).chain(live))
        .keep_alive(KeepAlive::default())
}
