//! The one object every front end drives.
//!
//! `App` owns the settings, the models list, the running llama-server, the
//! conversation and the downloads. Every change is pushed to the front end as
//! an [`Event`]; front ends never poll and never hold state of their own beyond
//! what they render.

use crate::chat::{self, ChatRequest, StreamDelta, StreamEnd, Turn, TurnStatus};
use crate::downloads::{DownloadInfo, DownloadRequest, Downloads};
use crate::errors::UserError;
use crate::events::{CourseModel, EngineState, Event, HardwareSummary, PublicSettings, Snapshot};
use crate::hardware::{CpuVariant, Hardware};
use crate::hub::{HF_BASE, Hub, RepoFiles, RepoHit};
use crate::models::{self, ModelInfo, VerifiedCache};
use crate::paths::Paths;
use crate::pins::{default_model, pins};
use crate::server::{ExitKind, Server, ServerConfig, exit_error};
use crate::settings::{self, Settings};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

pub type Sink = Arc<dyn Fn(Event) + Send + Sync>;

pub struct AppConfig {
    pub paths: Paths,
    /// Folder with the `avx2/`, `avx/`, `ssse3/` and `compat/` llama-server builds. None: not found.
    pub llama_root: Option<PathBuf>,
    /// Hugging Face base URL (tests point it at a local server).
    pub hf_base: String,
    /// Label sent as User-Agent to Hugging Face.
    pub user_agent: String,
}

impl AppConfig {
    pub fn from_env(front_end: &str) -> Self {
        AppConfig {
            paths: Paths::from_env(),
            llama_root: crate::paths::llama_root(),
            hf_base: std::env::var("TRURL_HF_BASE")
                .ok()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| HF_BASE.to_string()),
            user_agent: format!("Trurl/{} ({front_end})", crate::VERSION),
        }
    }
}

struct State {
    settings: Settings,
    models: Vec<ModelInfo>,
    engine: EngineState,
    turns: Vec<Turn>,
    next_turn: u64,
    generating: bool,
    /// Set while an update installs: no new answer may start.
    held: bool,
    verified: VerifiedCache,
    /// Bumped on every load request, so a slow old load cannot overwrite a newer one.
    load_epoch: u64,
}

/// Connection details of the running server, cloned out of the lock for each request.
#[derive(Clone)]
struct Endpoint {
    base_url: String,
    api_key: String,
    file: String,
    name: String,
}

struct Inner {
    paths: Paths,
    /// Settings writes, one at a time (see `save_settings`).
    settings_io: Mutex<()>,
    hw: Hardware,
    llama_root: Option<PathBuf>,
    hf_base: String,
    local_http: reqwest::Client,
    hub_http: reqwest::Client,
    sink: Sink,
    state: Mutex<State>,
    server: tokio::sync::Mutex<Option<Server>>,
    endpoint: Mutex<Option<Endpoint>>,
    /// The running answer's turn id and its cancel token.
    gen_cancel: Mutex<Option<(u64, CancellationToken)>>,
    load_cancel: Mutex<Option<CancellationToken>>,
    downloads: Mutex<Option<Downloads>>,
    shutdown: CancellationToken,
}

#[derive(Clone)]
pub struct App {
    inner: Arc<Inner>,
}

impl App {
    /// Build the app and load the selected model in the background.
    /// Must be called inside a tokio runtime.
    pub fn start(cfg: AppConfig, sink: Sink) -> App {
        let _ = cfg.paths.ensure();
        crate::server::LogFile::rotate(&cfg.paths.logs().join("llama-server.log"));
        crate::server::remove_stale_key_files(&cfg.paths.models());
        let hw = Hardware::detect();
        let settings = Settings::load(&cfg.paths.settings());
        let verified = VerifiedCache::load(&cfg.paths.verified_cache());
        // llama-server is local: never send its traffic through a proxy.
        let local_http = crate::net::client_builder()
            .no_proxy()
            .build()
            .expect("http client");
        let hub_http = crate::net::client_builder()
            .user_agent(cfg.user_agent.clone())
            .connect_timeout(Duration::from_secs(10))
            .read_timeout(Duration::from_secs(30))
            .build()
            .expect("http client");
        let app = App {
            inner: Arc::new(Inner {
                paths: cfg.paths,
                hw,
                llama_root: cfg.llama_root,
                hf_base: cfg.hf_base,
                local_http,
                hub_http,
                sink,
                state: Mutex::new(State {
                    settings,
                    models: Vec::new(),
                    engine: EngineState::Idle,
                    turns: Vec::new(),
                    next_turn: 1,
                    generating: false,
                    held: false,
                    verified,
                    load_epoch: 0,
                }),
                settings_io: Mutex::new(()),
                server: tokio::sync::Mutex::new(None),
                endpoint: Mutex::new(None),
                gen_cancel: Mutex::new(None),
                load_cancel: Mutex::new(None),
                downloads: Mutex::new(None),
                shutdown: CancellationToken::new(),
            }),
        };
        app.init_downloads();
        app.rescan();
        let initial = app.pick_initial_model();
        if let Some(file) = initial {
            app.load(file);
        }
        app.verify_bundled_in_background();
        app.spawn_watchdog();
        app
    }

    fn emit(&self, e: Event) {
        (self.inner.sink)(e);
    }

    fn init_downloads(&self) {
        let me = self.clone();
        let me2 = self.clone();
        let d = Downloads::new(
            self.inner.paths.downloads(),
            self.inner.paths.models(),
            self.inner.hf_base.clone(),
            self.inner.hub_http.clone(),
            Arc::new(move |list| me.emit(Event::Downloads { downloads: list })),
            Arc::new(move |file| {
                me2.rescan();
                // A first model on an empty machine: load it right away.
                let idle = matches!(me2.inner.state.lock().unwrap().engine, EngineState::Idle);
                if idle {
                    me2.load(file);
                }
            }),
        );
        d.set_token(&self.inner.state.lock().unwrap().settings.hf_token);
        // Downloads cut by closing Trurl carry on; ones the learner paused stay paused.
        d.resume_interrupted();
        *self.inner.downloads.lock().unwrap() = Some(d);
    }

    fn downloads(&self) -> Downloads {
        self.inner
            .downloads
            .lock()
            .unwrap()
            .clone()
            .expect("downloads initialised")
    }

    // ----- snapshot and settings -------------------------------------------

    fn public_settings(s: &Settings) -> PublicSettings {
        PublicSettings {
            system_prompt: s.system_prompt.clone(),
            temperature: s.temperature,
            context_size: s.context_size,
            model: s.model.clone(),
            has_hf_token: !s.hf_token.trim().is_empty(),
            default_temperature: settings::DEFAULT_TEMPERATURE,
            default_context: settings::DEFAULT_CONTEXT,
            context_choices: settings::CONTEXT_CHOICES.to_vec(),
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        let st = self.inner.state.lock().unwrap();
        let hw = &self.inner.hw;
        Snapshot {
            version: crate::VERSION.to_string(),
            llama_build: format!("{} ({})", pins().llama_cpp.release, pins().llama_cpp.build),
            models_dir: self.inner.paths.models().display().to_string(),
            hardware: HardwareSummary {
                total_ram: hw.total_ram,
                physical_cores: hw.physical_cores,
                threads: hw.threads(),
                cpu_name: hw.cpu_name.clone(),
                variant: self.variant_locked(&st.settings),
            },
            settings: Self::public_settings(&st.settings),
            models: st.models.clone(),
            engine: st.engine.clone(),
            turns: st.turns.clone(),
            generating: st.generating,
            downloads: self.downloads().list(),
            course_models: crate::pins::course_models()
                .map(|m| CourseModel {
                    id: m.id.clone(),
                    name: m.name.clone(),
                    note: m.course.clone().unwrap_or_default(),
                    file: m.file.clone(),
                    size: m.size,
                    fit: crate::hardware::ram_fit(m.size, hw.total_ram),
                    download_id: crate::downloads::download_id(&m.repo, &m.file, &m.revision),
                })
                .collect(),
        }
    }

    pub fn emit_snapshot(&self) {
        let s = self.snapshot();
        self.emit(Event::Snapshot {
            snapshot: Box::new(s),
        });
    }

    fn save_settings(&self, s: &Settings) {
        {
            // Write what is current when the write happens, one write at a time:
            // a slower thread can never put an older snapshot back on disk.
            let _one = self
                .inner
                .settings_io
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            let current = self.inner.state.lock().unwrap().settings.clone();
            let _ = current.save(&self.inner.paths.settings());
        }
        self.emit(Event::Settings {
            settings: Self::public_settings(s),
        });
    }

    pub fn set_system_prompt(&self, text: &str) {
        let s = {
            let mut st = self.inner.state.lock().unwrap();
            st.settings.system_prompt = text.to_string();
            st.settings.clone()
        };
        self.save_settings(&s);
    }

    pub fn set_temperature(&self, t: f32) {
        let s = {
            let mut st = self.inner.state.lock().unwrap();
            st.settings.temperature = settings::clamp_temperature(t);
            st.settings.clone()
        };
        self.save_settings(&s);
    }

    /// Changing the context restarts the model, like in every local-LLM app.
    pub fn set_context(&self, c: u32) {
        let (s, changed, file) = {
            let mut st = self.inner.state.lock().unwrap();
            let c = settings::clamp_context(c);
            let changed = st.settings.context_size != c;
            st.settings.context_size = c;
            (st.settings.clone(), changed, st.settings.model.clone())
        };
        self.save_settings(&s);
        if changed && let Some(f) = file {
            self.load(f);
        }
    }

    pub fn set_hf_token(&self, token: &str) {
        let s = {
            let mut st = self.inner.state.lock().unwrap();
            st.settings.hf_token = token.trim().to_string();
            st.settings.clone()
        };
        self.downloads().set_token(&s.hf_token);
        self.save_settings(&s);
    }

    pub fn hardware(&self) -> &Hardware {
        &self.inner.hw
    }

    pub fn models_dir(&self) -> PathBuf {
        self.inner.paths.models()
    }

    /// The detected build, capped by an earlier illegal-instruction crash, and
    /// lowered to the best build actually installed.
    fn variant_locked(&self, s: &Settings) -> CpuVariant {
        let mut v = self.inner.hw.variant;
        if let Some(cap) = s.max_variant {
            v = v.min(cap);
        }
        if let Some(root) = &self.inner.llama_root {
            let mut cur = Some(v);
            while let Some(c) = cur {
                if root
                    .join(c.dir_name())
                    .join(crate::paths::server_exe_name())
                    .is_file()
                {
                    return c;
                }
                cur = c.fallback();
            }
        }
        v
    }

    // ----- models ------------------------------------------------------------

    pub fn rescan(&self) {
        // Reading headers takes a moment on a slow disk: never hold the state lock for it.
        let verified = self.inner.state.lock().unwrap().verified.clone();
        let models = models::scan(
            &self.inner.paths.models(),
            self.inner.hw.total_ram,
            &verified,
        );
        self.inner.state.lock().unwrap().models = models.clone();
        self.emit(Event::Models { models });
    }

    pub fn models(&self) -> Vec<ModelInfo> {
        self.inner.state.lock().unwrap().models.clone()
    }

    fn pick_initial_model(&self) -> Option<String> {
        let st = self.inner.state.lock().unwrap();
        choose_initial_model(&st.models, st.settings.model.as_deref())
    }

    /// Hash bundled models once (cached by size and date), so a damaged USB copy is caught.
    fn verify_bundled_in_background(&self) {
        let pending = self
            .inner
            .state
            .lock()
            .unwrap()
            .verified
            .pending(&self.inner.paths.models());
        if pending.is_empty() {
            return;
        }
        let me = self.clone();
        tokio::task::spawn_blocking(move || {
            for p in pending {
                let file = p.file_name().unwrap().to_string_lossy().into_owned();
                let Some(pin) = crate::pins::pinned_model(&file) else {
                    continue;
                };
                let total = pin.size;
                let mut last = std::time::Instant::now();
                let r = models::sha256_file(&p, |done| {
                    if last.elapsed() > Duration::from_millis(500) {
                        last = std::time::Instant::now();
                        me.emit(Event::VerifyProgress {
                            file: file.clone(),
                            done,
                            total,
                        });
                    }
                });
                if me.inner.shutdown.is_cancelled() {
                    return;
                }
                if let Ok(h) = r {
                    me.inner
                        .state
                        .lock()
                        .unwrap()
                        .verified
                        .set(&p, h == pin.sha256);
                }
                me.emit(Event::VerifyProgress {
                    file: file.clone(),
                    done: total,
                    total,
                });
                me.rescan();
            }
        });
    }

    pub fn import_model(&self, src: PathBuf) {
        let me = self.clone();
        tokio::task::spawn_blocking(move || {
            let file = src
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let mut last = std::time::Instant::now();
            let r = models::import(&src, &me.inner.paths.models(), |done, total| {
                if last.elapsed() > Duration::from_millis(200) || done == total {
                    last = std::time::Instant::now();
                    me.emit(Event::ImportProgress {
                        file: file.clone(),
                        done,
                        total,
                    });
                }
            });
            match r {
                Ok(name) => {
                    me.rescan();
                    me.emit(Event::ImportFinished {
                        file: Some(name.clone()),
                        error: None,
                    });
                    let idle = matches!(me.inner.state.lock().unwrap().engine, EngineState::Idle);
                    if idle {
                        me.load(name);
                    }
                    me.verify_bundled_in_background();
                }
                Err(e) => me.emit(Event::ImportFinished {
                    file: None,
                    error: Some(e),
                }),
            }
        });
    }

    /// Delete a model. If it is the one loaded, the server is stopped first:
    /// Windows cannot delete a file that is memory-mapped.
    pub async fn delete_model(&self, file: &str) -> Result<(), UserError> {
        let loaded = self
            .inner
            .endpoint
            .lock()
            .unwrap()
            .as_ref()
            .map(|e| e.file.clone());
        let engine_file = match &self.inner.state.lock().unwrap().engine {
            EngineState::Loading { file, .. }
            | EngineState::Ready { file, .. }
            | EngineState::Failed { file, .. } => Some(file.clone()),
            EngineState::Idle => None,
        };
        let is_current = loaded.as_deref() == Some(file) || engine_file.as_deref() == Some(file);
        if is_current {
            self.stop_generation();
            self.unload().await;
        }
        let dir = self.inner.paths.models();
        let f = file.to_string();
        let r = tokio::task::spawn_blocking(move || models::delete(&dir, &f))
            .await
            .unwrap_or_else(|e| Err(UserError::internal(e.to_string())));
        if let Err(e) = &r {
            self.emit(Event::Notice { error: e.clone() });
        }
        {
            let mut st = self.inner.state.lock().unwrap();
            if st.settings.model.as_deref() == Some(file) {
                st.settings.model = None;
                let s = st.settings.clone();
                drop(st);
                self.save_settings(&s);
            }
        }
        self.rescan();
        if is_current && let Some(next) = self.pick_initial_model() {
            self.load(next);
        }
        r
    }

    // ----- engine ------------------------------------------------------------

    fn set_engine(&self, e: EngineState) {
        self.inner.state.lock().unwrap().engine = e.clone();
        self.emit(Event::Engine { engine: e });
    }

    fn model_name(&self, file: &str) -> String {
        self.inner
            .state
            .lock()
            .unwrap()
            .models
            .iter()
            .find(|m| m.file == file)
            .map(|m| m.name.clone())
            .unwrap_or_else(|| file.trim_end_matches(".gguf").to_string())
    }

    pub fn select_model(&self, file: &str) {
        self.load(file.to_string());
    }

    /// Reload the current model (after a crash, or from the error banner's "Try again").
    /// The update could not be installed: lift the hold and bring the model back.
    pub fn update_failed_reload(&self) {
        self.inner.state.lock().unwrap().held = false;
        self.reload();
    }

    pub fn reload(&self) {
        let file = self.inner.state.lock().unwrap().settings.model.clone();
        if let Some(f) = file {
            self.load(f);
        }
    }

    /// Stop the model server but keep the app alive (before installing an
    /// update). Refuses while an answer is being written, and from then on no
    /// new answer can start, in one step: there is no moment between "is it
    /// busy?" and "stop it" where a learner's answer could be cut off. If the
    /// install fails, `update_failed_reload()` lifts the hold and brings the model back.
    pub async fn release_engine(&self) -> Result<(), UserError> {
        {
            let mut st = self.inner.state.lock().unwrap();
            if st.generating {
                return Err(UserError::update_wait_for_answer());
            }
            st.held = true;
        }
        self.unload().await;
        Ok(())
    }

    async fn unload(&self) {
        if let Some(c) = self.inner.load_cancel.lock().unwrap().take() {
            c.cancel();
        }
        *self.inner.endpoint.lock().unwrap() = None;
        let mut guard = self.inner.server.lock().await;
        if let Some(mut s) = guard.take() {
            tokio::task::spawn_blocking(move || s.stop()).await.ok();
        }
        self.set_engine(EngineState::Idle);
    }

    /// Stop whatever runs, start llama-server with `file`, report progress.
    pub fn load(&self, file: String) {
        // An update is installing: the model server must stay down (its files are
        // being replaced). `update_failed_reload()` lifts the hold if the install fails.
        if self.inner.state.lock().unwrap().held {
            return;
        }
        self.stop_generation();
        let cancel = CancellationToken::new();
        if let Some(old) = self
            .inner
            .load_cancel
            .lock()
            .unwrap()
            .replace(cancel.clone())
        {
            old.cancel();
        }
        // An update may have taken the engine between the check above and now:
        // its unload cancels the token it found, so check again with ours in place.
        if self.inner.state.lock().unwrap().held {
            cancel.cancel();
            return;
        }
        let name = self.model_name(&file);
        let (epoch, cfg_base) = {
            let mut st = self.inner.state.lock().unwrap();
            st.load_epoch += 1;
            st.settings.model = Some(file.clone());
            (st.load_epoch, st.settings.clone())
        };
        self.save_settings(&cfg_base);
        *self.inner.endpoint.lock().unwrap() = None;
        self.set_engine(EngineState::Loading {
            file: file.clone(),
            name: name.clone(),
        });
        let me = self.clone();
        tokio::spawn(async move {
            let result = me.load_inner(&file, &name, &cancel).await;
            let current = me.inner.state.lock().unwrap().load_epoch == epoch;
            if !current || cancel.is_cancelled() {
                return;
            }
            match result {
                Ok(ctx) => me.set_engine(EngineState::Ready {
                    file,
                    name,
                    context: ctx,
                }),
                Err(error) => me.set_engine(EngineState::Failed { file, name, error }),
            }
        });
    }

    async fn load_inner(
        &self,
        file: &str,
        name: &str,
        cancel: &CancellationToken,
    ) -> Result<u32, UserError> {
        // Only a file that sits in the models folder, by its plain name: a path
        // (from a front end or an edited settings file) never reaches llama-server.
        if !crate::paths::is_plain_model_name(file) {
            return Err(UserError::model_missing(file));
        }
        let Some(root) = self.inner.llama_root.clone() else {
            return Err(UserError::server_missing());
        };
        let mut guard = self.inner.server.lock().await;
        if cancel.is_cancelled() {
            return Err(UserError::server_start());
        }
        if let Some(mut old) = guard.take() {
            *self.inner.endpoint.lock().unwrap() = None;
            tokio::task::spawn_blocking(move || old.stop()).await.ok();
        }
        if let Some(m) = self
            .inner
            .state
            .lock()
            .unwrap()
            .models
            .iter()
            .find(|m| m.file == file)
            && m.broken
        {
            return Err(UserError::model_corrupt(name));
        }
        let mut port_retries = 0;
        loop {
            let (ctx, variant) = {
                let st = self.inner.state.lock().unwrap();
                (st.settings.context_size, self.variant_locked(&st.settings))
            };
            let cfg = ServerConfig {
                models_dir: self.inner.paths.models(),
                model_file: file.to_string(),
                context: ctx,
                threads: self.inner.hw.threads(),
                variant,
            };
            let log = self.inner.paths.logs().join("llama-server.log");
            let mut server = Server::spawn_logged(&cfg, &root, Some(&log))?;
            match server.wait_ready(&self.inner.local_http, cancel).await {
                Ok(()) if cancel.is_cancelled() => {
                    // A newer load replaced this one while it finished: do not publish it.
                    tokio::task::spawn_blocking(move || server.stop())
                        .await
                        .ok();
                    return Err(UserError::server_start());
                }
                Ok(()) => {
                    *self.inner.endpoint.lock().unwrap() = Some(Endpoint {
                        base_url: server.base_url(),
                        api_key: server.api_key.clone(),
                        file: file.to_string(),
                        name: name.to_string(),
                    });
                    *guard = Some(server);
                    return Ok(ctx);
                }
                Err(kind) => {
                    let tail = server.log.tail(40);
                    drop(server);
                    if cancel.is_cancelled() {
                        return Err(UserError::server_start());
                    }
                    match kind {
                        ExitKind::IllegalInstruction if variant.fallback().is_some() => {
                            // This CPU cannot run this build after all: remember and step down.
                            let s = {
                                let mut st = self.inner.state.lock().unwrap();
                                st.settings.max_variant = variant.fallback();
                                st.settings.clone()
                            };
                            let _ = s.save(&self.inner.paths.settings());
                            continue;
                        }
                        ExitKind::PortInUse if port_retries < 3 => {
                            port_retries += 1;
                            continue;
                        }
                        k => return Err(exit_error(&k, name, &tail)),
                    }
                }
            }
        }
    }

    /// Every 2 s, check the server is still alive while nobody is chatting.
    /// A crash mid-chat is reported by the chat stream itself.
    fn spawn_watchdog(&self) {
        let me = self.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(2)) => {}
                    _ = me.inner.shutdown.cancelled() => return,
                }
                let ready_file = match &me.inner.state.lock().unwrap().engine {
                    EngineState::Ready { file, .. } => Some(file.clone()),
                    _ => None,
                };
                let Some(file) = ready_file else { continue };
                let died = {
                    let Ok(mut guard) = me.inner.server.try_lock() else {
                        continue;
                    };
                    guard.as_mut().and_then(|s| s.exited()).is_some()
                };
                if died {
                    let name = me.model_name(&file);
                    me.set_engine(EngineState::Failed {
                        file,
                        name,
                        error: UserError::server_crashed(),
                    });
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    me.reload();
                }
            }
        });
    }

    // ----- chat --------------------------------------------------------------

    pub fn turns(&self) -> Vec<Turn> {
        self.inner.state.lock().unwrap().turns.clone()
    }

    pub fn is_generating(&self) -> bool {
        self.inner.state.lock().unwrap().generating
    }

    pub fn new_chat(&self) {
        self.stop_generation();
        self.inner.state.lock().unwrap().turns.clear();
        self.emit(Event::ChatCleared);
    }

    pub fn stop_generation(&self) {
        if let Some((_, c)) = self.inner.gen_cancel.lock().unwrap().take() {
            c.cancel();
        }
    }

    /// Send a message. Errors are returned (and not emitted) when nothing started.
    pub fn send(&self, text: &str) -> Result<u64, UserError> {
        let text = text.trim_end().to_string();
        if text.trim().is_empty() {
            return Err(UserError::internal("empty message"));
        }
        // First: during an update the model is gone on purpose, and "no model"
        // would send the learner looking for one.
        if self.inner.state.lock().unwrap().held {
            return Err(UserError::installing_update());
        }
        let Some(ep) = self.inner.endpoint.lock().unwrap().clone() else {
            let st = self.inner.state.lock().unwrap();
            return Err(match &st.engine {
                EngineState::Idle => UserError::no_model(),
                EngineState::Failed { error, .. } => error.clone(),
                EngineState::Loading { .. } | EngineState::Ready { .. } => UserError::busy(),
            });
        };
        let cancel = CancellationToken::new();
        let (turn, messages, temperature) = {
            let mut st = self.inner.state.lock().unwrap();
            if st.held {
                return Err(UserError::installing_update());
            }
            if st.generating {
                return Err(UserError::busy());
            }
            // The endpoint may belong to a model that is being replaced right now.
            let ready = matches!(&st.engine, EngineState::Ready { file, .. } if *file == ep.file);
            if !ready {
                return Err(UserError::busy());
            }
            st.generating = true;
            let messages = chat::history(&st.settings.system_prompt, &st.turns, &text);
            let id = st.next_turn;
            st.next_turn += 1;
            let turn = Turn {
                id,
                user: text.clone(),
                reasoning: String::new(),
                answer: String::new(),
                model: ep.name.clone(),
                status: TurnStatus::Streaming,
                thought_ms: None,
                error: None,
                system_prompt: st.settings.system_prompt.clone(),
                temperature: st.settings.temperature,
                context: st.settings.context_size,
            };
            st.turns.push(turn.clone());
            (turn, messages, st.settings.temperature)
        };
        *self.inner.gen_cancel.lock().unwrap() = Some((turn.id, cancel.clone()));
        self.emit(Event::Generating { generating: true });
        self.emit(Event::TurnStarted { turn: turn.clone() });
        let id = turn.id;
        let me = self.clone();
        tokio::spawn(async move {
            let on = |d: StreamDelta| me.apply_delta(id, d);
            let end = chat::stream(
                &me.inner.local_http,
                ChatRequest {
                    base_url: &ep.base_url,
                    api_key: &ep.api_key,
                    messages,
                    temperature,
                },
                &cancel,
                on,
            )
            .await;
            me.finish_turn(id, end, &ep);
        });
        Ok(id)
    }

    fn apply_delta(&self, id: u64, d: StreamDelta) {
        let ev = {
            let mut st = self.inner.state.lock().unwrap();
            let Some(t) = st.turns.iter_mut().find(|t| t.id == id) else {
                return;
            };
            match d {
                StreamDelta::Reasoning(r) => {
                    t.reasoning.push_str(&r);
                    Event::TurnDelta {
                        id,
                        reasoning: Some(r),
                        answer: None,
                        reasoning_from_answer: false,
                    }
                }
                StreamDelta::Answer(a) => {
                    t.answer.push_str(&a);
                    Event::TurnDelta {
                        id,
                        reasoning: None,
                        answer: Some(a),
                        reasoning_from_answer: false,
                    }
                }
                StreamDelta::AnswerWasReasoning => {
                    let moved = std::mem::take(&mut t.answer);
                    t.reasoning = moved + &t.reasoning;
                    Event::TurnDelta {
                        id,
                        reasoning: None,
                        answer: None,
                        reasoning_from_answer: true,
                    }
                }
                StreamDelta::ThoughtFor(ms) => {
                    t.thought_ms = Some(ms);
                    Event::TurnThought { id, ms }
                }
            }
        };
        self.emit(ev);
    }

    fn finish_turn(&self, id: u64, end: StreamEnd, ep: &Endpoint) {
        // A crash is only a crash if the server went away, not if we closed it on purpose.
        let crashed = matches!(end, StreamEnd::ServerGone(_));
        let turn = {
            let mut st = self.inner.state.lock().unwrap();
            st.generating = false;
            // "New chat" during an answer removes the turn: the answer is over all the same,
            // and the front ends must still hear that nothing is generating any more.
            // "First" for the context: no earlier turn goes back to the model (a
            // question that got no answer costs no room), so a new chat cannot help.
            let first = st
                .turns
                .iter()
                .filter(|t| t.id != id)
                .all(|t| !t.in_history());
            st.turns.iter_mut().find(|t| t.id == id).map(|t| {
                match end {
                    StreamEnd::Done => t.status = TurnStatus::Done,
                    StreamEnd::Stopped => t.status = TurnStatus::Stopped,
                    StreamEnd::Length => {
                        t.status = TurnStatus::Cut;
                        t.error = Some(UserError::context_full());
                    }
                    StreamEnd::Failed(e) => {
                        t.status = TurnStatus::Error;
                        t.error = Some(e);
                    }
                    StreamEnd::ServerGone(detail) => {
                        t.status = TurnStatus::Error;
                        t.error = Some(UserError::server_crashed().with_detail(detail));
                    }
                }
                // The first message of a chat: "Start a new chat" cannot help.
                if first
                    && t.error
                        .as_ref()
                        .is_some_and(|e| e.code == crate::errors::ErrorCode::ContextFull)
                {
                    let detail = t.error.as_ref().and_then(|e| e.detail.clone());
                    let mut e = if t.status == TurnStatus::Cut {
                        UserError::context_full_first()
                    } else {
                        UserError::message_too_long()
                    };
                    e.detail = detail;
                    t.error = Some(e);
                }
                t.clone()
            })
        };
        {
            // Only clear our own token: a new answer may already have started.
            let mut gc = self.inner.gen_cancel.lock().unwrap();
            if gc.as_ref().map(|(tid, _)| *tid == id).unwrap_or(false) {
                *gc = None;
            }
        }
        if let Some(turn) = turn {
            self.emit(Event::TurnFinished { turn });
        }
        self.emit(Event::Generating { generating: false });
        if crashed {
            let still_same = self
                .inner
                .endpoint
                .lock()
                .unwrap()
                .as_ref()
                .map(|e| e.base_url == ep.base_url)
                .unwrap_or(false);
            if still_same {
                self.set_engine(EngineState::Failed {
                    file: ep.file.clone(),
                    name: ep.name.clone(),
                    error: UserError::server_crashed(),
                });
                self.reload();
            }
        }
    }

    // ----- Hugging Face ------------------------------------------------------

    fn hub(&self) -> Hub {
        let token = self.inner.state.lock().unwrap().settings.hf_token.clone();
        Hub::new(
            self.inner.hub_http.clone(),
            self.inner.hf_base.clone(),
            Some(token),
        )
    }

    pub async fn hub_online(&self) -> bool {
        self.hub().online().await
    }

    pub async fn hub_search(&self, q: &str) -> Result<Vec<RepoHit>, UserError> {
        self.hub().search(q).await
    }

    pub async fn hub_details(&self, repo: &str) -> Result<RepoFiles, UserError> {
        let mut d = self.hub().details(repo, self.inner.hw.total_ram).await?;
        // The build that runs here, not the detected one: after an AVX2 crash the
        // AVX build answers, and Q4_0 is its faster format.
        let variant = {
            let st = self.inner.state.lock().unwrap();
            self.variant_locked(&st.settings)
        };
        crate::hub::recommend(&mut d.files, variant);
        Ok(d)
    }

    pub fn download_start(&self, req: DownloadRequest) -> Result<String, UserError> {
        self.downloads().start(req)
    }
    /// Download a course model by its id in pins.json: the pinned repo, commit,
    /// file, size and SHA-256, never what a front end passes.
    pub fn download_course_model(&self, id: &str) -> Result<String, UserError> {
        let m = crate::pins::course_models()
            .find(|m| m.id == id)
            .ok_or_else(|| UserError::internal(format!("no course model {id}")))?;
        self.download_start(DownloadRequest {
            repo: m.repo.clone(),
            file: m.file.clone(),
            revision: m.revision.clone(),
            size: m.size,
            sha256: Some(m.sha256.clone()),
        })
    }
    pub fn download_pause(&self, id: &str) {
        self.downloads().pause(id)
    }
    pub fn download_resume(&self, id: &str) -> Result<(), UserError> {
        self.downloads().resume(id)
    }
    pub fn download_cancel(&self, id: &str) {
        self.downloads().cancel(id)
    }
    pub fn downloads_list(&self) -> Vec<DownloadInfo> {
        self.downloads().list()
    }

    // ----- lifecycle ---------------------------------------------------------

    /// Stop everything and wait for llama-server to exit. Safe to call twice.
    pub async fn shutdown(&self) {
        self.inner.shutdown.cancel();
        self.stop_generation();
        self.downloads().pause_all();
        if let Some(c) = self.inner.load_cancel.lock().unwrap().take() {
            c.cancel();
        }
        let mut guard = self.inner.server.lock().await;
        if let Some(mut s) = guard.take() {
            tokio::task::spawn_blocking(move || s.stop()).await.ok();
        }
    }

    /// Synchronous last resort for exit paths that cannot await.
    pub fn shutdown_blocking(&self) {
        self.inner.shutdown.cancel();
        self.stop_generation();
        if let Ok(mut guard) = self.inner.server.try_lock()
            && let Some(mut s) = guard.take()
        {
            s.stop();
        }
    }

    pub fn data_dir(&self) -> &Path {
        &self.inner.paths.home
    }
}

/// Which model to load at start: the one the learner used last, else the
/// pinned default (Gemma 3 1B), else the first usable file. Damaged files never.
pub fn choose_initial_model(models: &[ModelInfo], last: Option<&str>) -> Option<String> {
    let usable = |f: &str| models.iter().any(|m| m.file == f && !m.broken);
    if let Some(f) = last.filter(|f| usable(f)) {
        return Some(f.to_string());
    }
    if let Some(d) = default_model().filter(|d| usable(&d.file)) {
        return Some(d.file.clone());
    }
    models.iter().find(|m| !m.broken).map(|m| m.file.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware::RamFit;

    fn model(file: &str, broken: bool) -> ModelInfo {
        ModelInfo {
            file: file.into(),
            name: file.into(),
            quant: None,
            size: 1,
            fit: RamFit::Green,
            bundled: false,
            course: false,
            verified: None,
            broken,
            has_chat_template: true,
            reasoning_hint: false,
            train_context: None,
            gguf: None,
        }
    }

    #[test]
    fn gemma_1b_is_preselected_whatever_sorts_first() {
        let m = vec![
            model("a-model.gguf", false),
            model("Qwen_Qwen3-0.6B-Q4_K_M.gguf", false),
            model("gemma-3-1b-it-Q4_K_M.gguf", false),
        ];
        assert_eq!(
            choose_initial_model(&m, None).as_deref(),
            Some("gemma-3-1b-it-Q4_K_M.gguf")
        );
    }

    #[test]
    fn the_last_used_model_wins_unless_it_is_gone_or_damaged() {
        let m = vec![
            model("gemma-3-1b-it-Q4_K_M.gguf", false),
            model("mine.gguf", false),
            model("bad.gguf", true),
        ];
        assert_eq!(
            choose_initial_model(&m, Some("mine.gguf")).as_deref(),
            Some("mine.gguf")
        );
        assert_eq!(
            choose_initial_model(&m, Some("gone.gguf")).as_deref(),
            Some("gemma-3-1b-it-Q4_K_M.gguf")
        );
        assert_eq!(
            choose_initial_model(&m, Some("bad.gguf")).as_deref(),
            Some("gemma-3-1b-it-Q4_K_M.gguf")
        );
    }

    #[test]
    fn no_usable_model_means_none() {
        assert_eq!(choose_initial_model(&[model("bad.gguf", true)], None), None);
        assert_eq!(choose_initial_model(&[], None), None);
    }
}
