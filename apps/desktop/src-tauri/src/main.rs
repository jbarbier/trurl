//! Trurl desktop: a thin Tauri shell around trurl-core.
//!
//! All app logic lives in the core. This file only:
//! - forwards UI commands to `trurl_core::api::dispatch` (one `call` command),
//! - forwards core events to the window as `trurl` events,
//! - adds what only a desktop app has: the self-updater, the single-instance
//!   guard, opening the models folder, and stopping llama-server on exit.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, RunEvent};
use tauri_plugin_updater::{Update, UpdaterExt};
use trurl_core::app::{App, AppConfig};
use trurl_core::errors::UserError;

struct Desktop {
    core: App,
    pending: Mutex<Option<Update>>,
    installing: Mutex<bool>,
}

const UPDATE_EVERY: Duration = Duration::from_secs(24 * 60 * 60);
/// Let the first model load before touching the network at all.
const FIRST_CHECK_AFTER: Duration = Duration::from_secs(15);

#[tauri::command]
async fn call(
    app: AppHandle,
    state: tauri::State<'_, Arc<Desktop>>,
    cmd: String,
    args: Value,
) -> Result<Value, UserError> {
    let d = state.inner().clone();
    match cmd.as_str() {
        "update_check" => check_update(&app, &d, true).await,
        "update_install" => install_update(app, d).await.map(|_| Value::Null),
        "open_models_folder" => {
            open_folder(&d.core.models_dir());
            Ok(Value::Null)
        }
        _ => trurl_core::api::dispatch(&d.core, &cmd, args).await,
    }
}

fn open_folder(path: &std::path::Path) {
    let _ = std::fs::create_dir_all(path);
    #[cfg(windows)]
    let _ = std::process::Command::new("explorer").arg(path).spawn();
    #[cfg(target_os = "linux")]
    let _ = std::process::Command::new("xdg-open").arg(path).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(path).spawn();
}

/// Check GitHub Releases for a newer version. Silent unless `manual`.
async fn check_update(app: &AppHandle, d: &Desktop, manual: bool) -> Result<Value, UserError> {
    let updater = app
        .updater_builder()
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| UserError::update_check_failed().with_detail(e.to_string()))?;
    match updater.check().await {
        Ok(Some(update)) => {
            let info = json!({ "version": update.version, "notes": update.body, "date": update.date.map(|d| d.to_string()) });
            *d.pending.lock().unwrap() = Some(update);
            if !manual {
                let _ = app.emit(
                    "trurl",
                    json!({ "type": "update_available", "update": info }),
                );
            }
            Ok(info)
        }
        Ok(None) => Ok(Value::Null),
        Err(e) => Err(UserError::update_check_failed().with_detail(e.to_string())),
    }
}

/// Download first (the app keeps working), then stop the model server, install
/// and restart. Never while an answer is being written.
async fn install_update(app: AppHandle, d: Arc<Desktop>) -> Result<(), UserError> {
    if d.core.is_generating() {
        return Err(UserError::update_wait_for_answer());
    }
    {
        let mut busy = d.installing.lock().unwrap();
        if *busy {
            return Ok(());
        }
        *busy = true;
    }
    let result = async {
        let update = match d.pending.lock().unwrap().take() {
            Some(u) => u,
            None => {
                check_update(&app, &d, true).await?;
                d.pending.lock().unwrap().take().ok_or_else(UserError::update_failed)?
            }
        };
        let mut downloaded: u64 = 0;
        let progress_app = app.clone();
        let bytes = update
            .download(
                move |chunk, total| {
                    downloaded += chunk as u64;
                    let _ = progress_app.emit("trurl", json!({ "type": "update_progress", "downloaded": downloaded, "total": total }));
                },
                || {},
            )
            .await
            .map_err(|e| UserError::update_failed().with_detail(e.to_string()))?;
        if d.core.is_generating() {
            // An answer started during the download: keep the bytes for "Update now" again.
            *d.pending.lock().unwrap() = Some(update);
            return Err(UserError::update_wait_for_answer());
        }
        d.core.shutdown().await;
        update.install(bytes).map_err(|e| UserError::update_failed().with_detail(e.to_string()))?;
        app.restart();
    }
    .await;
    *d.installing.lock().unwrap() = false;
    if let Err(e) = &result {
        let _ = app.emit("trurl", json!({ "type": "update_failed", "error": e }));
    }
    result
}

fn main() {
    let builder = tauri::Builder::default()
        // Must be first: a second launch focuses the running window instead of
        // starting a second llama-server (two models would not fit in 4 GB).
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.unminimize();
                let _ = w.show();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .invoke_handler(tauri::generate_handler![call])
        .setup(|app| {
            let handle = app.handle().clone();
            let emit_handle = handle.clone();
            let core = tauri::async_runtime::block_on(async move {
                App::start(
                    AppConfig::from_env("desktop"),
                    Arc::new(move |e| {
                        let _ = emit_handle.emit("trurl", &e);
                    }),
                )
            });
            let desktop = Arc::new(Desktop {
                core,
                pending: Mutex::new(None),
                installing: Mutex::new(false),
            });
            app.manage(desktop.clone());

            if let Some(w) = app.get_webview_window("main") {
                // Small screens (the common case here): use all of it.
                if let Ok(Some(m)) = w.current_monitor() {
                    let size = m.size().to_logical::<f64>(m.scale_factor());
                    if size.width <= 1440.0 || size.height <= 900.0 {
                        let _ = w.maximize();
                    }
                }
                let _ = w.show();
            }

            // Update checks: once after launch, then every 24 hours. Offline: silently nothing.
            let d = desktop.clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(FIRST_CHECK_AFTER).await;
                loop {
                    let _ = check_update(&handle, &d, false).await;
                    tokio::time::sleep(UPDATE_EVERY).await;
                }
            });
            Ok(())
        });

    let app = builder
        .build(tauri::generate_context!())
        .expect("Trurl failed to start");
    app.run(|handle, event| {
        if let RunEvent::Exit = event {
            if let Some(d) = handle.try_state::<Arc<Desktop>>() {
                d.core.shutdown_blocking();
            }
        }
    });
}
