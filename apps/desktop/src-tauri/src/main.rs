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
use trurl_core::update;

/// An update download that receives nothing for this long has failed.
const UPDATE_STALL: Duration = Duration::from_secs(60);

struct Desktop {
    core: App,
    pending: Mutex<Option<Update>>,
    /// A downloaded update waiting for the answer in progress to finish: "Update
    /// now" again installs it without downloading it a second time.
    downloaded: Mutex<Option<(Update, Vec<u8>)>>,
    installing: Mutex<bool>,
}

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
        // Take it out first: a lock guard must not live across the awaits below.
        let ready = d.downloaded.lock().unwrap().take();
        let (update, bytes) = match ready {
            Some(r) => r,
            None => {
                let pending = d.pending.lock().unwrap().take();
                let update = match pending {
                    Some(u) => u,
                    None => {
                        check_update(&app, &d, true).await?;
                        d.pending.lock().unwrap().take().ok_or_else(UserError::update_failed)?
                    }
                };
                let mut downloaded: u64 = 0;
                let progress_app = app.clone();
                // The updater has no timeout of its own: a link that stalls would
                // leave "Downloading the update" on screen forever. No byte for a
                // minute fails the download; the next "Update now" starts again.
                let started = std::time::Instant::now();
                let last_byte = Arc::new(std::sync::atomic::AtomicU64::new(0));
                let seen = last_byte.clone();
                let download = update.download(
                    move |chunk, total| {
                        downloaded += chunk as u64;
                        seen.store(started.elapsed().as_secs(), std::sync::atomic::Ordering::Relaxed);
                        let _ = progress_app.emit("trurl", json!({ "type": "update_progress", "downloaded": downloaded, "total": total }));
                    },
                    || {},
                );
                let stalled = async {
                    loop {
                        tokio::time::sleep(Duration::from_secs(5)).await;
                        let quiet = started.elapsed().as_secs() - last_byte.load(std::sync::atomic::Ordering::Relaxed);
                        if quiet >= UPDATE_STALL.as_secs() {
                            return;
                        }
                    }
                };
                let bytes = tokio::select! {
                    r = download => r.map_err(|e| UserError::update_failed().with_detail(e.to_string()))?,
                    _ = stalled => {
                        return Err(UserError::update_failed().with_detail("no data for a minute"));
                    }
                };
                (update, bytes)
            }
        };
        // Stop only the model server (its files are about to be replaced). On Windows
        // the installer then closes Trurl and starts the new version. If anything
        // fails before that, the model comes back and Trurl carries on as it was.
        // Refused, atomically, if an answer started during the download: the bytes
        // are kept for "Update now" again.
        if let Err(e) = d.core.release_engine().await {
            *d.downloaded.lock().unwrap() = Some((update, bytes));
            return Err(e);
        }
        if let Err(e) = update.install(bytes) {
            d.core.reload();
            return Err(UserError::update_failed().with_detail(e.to_string()));
        }
        // Not reached on Windows (the installer exits the app); harmless elsewhere.
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
                downloaded: Mutex::new(None),
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
                tokio::time::sleep(update::FIRST_CHECK_AFTER).await;
                let mut schedule = update::CheckSchedule::default();
                loop {
                    let now = std::time::SystemTime::now();
                    // Never while an answer is being written or an update installs.
                    if schedule.due(now)
                        && !d.core.is_generating()
                        && !*d.installing.lock().unwrap()
                    {
                        let _ = check_update(&handle, &d, false).await;
                        schedule.checked(now);
                    }
                    tokio::time::sleep(update::SCHEDULE_TICK).await;
                }
            });
            Ok(())
        });

    let app = builder
        .build(tauri::generate_context!())
        .expect("Trurl failed to start");
    app.run(|handle, event| {
        if let RunEvent::Exit = event
            && let Some(d) = handle.try_state::<Arc<Desktop>>()
        {
            d.core.shutdown_blocking();
        }
    });
}
