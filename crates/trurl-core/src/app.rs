//! The one object every front end drives.
//!
//! `App` owns the settings, the models list, the running llama-server, the
//! conversation and the downloads. Every change is pushed to the front end as
//! an [`Event`]; front ends never poll and never hold state of their own beyond
//! what they render.

use crate::chat::{self, ChatRequest, StreamDelta, StreamEnd, Turn, TurnStatus};
use crate::downloads::{DownloadInfo, DownloadRequest, Downloads};
use crate::errors::UserError;
use crate::events::{EngineState, Event, HardwareSummary, PublicSettings, Snapshot};
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
    /// Folder with `avx2/` and `compat/` llama-server builds. None: not found.
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
            hf_base: std::env::var("TRURL_HF_BASE").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| HF_BASE.to_string()),
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
    hw: Hardware,
    llama_root: Option<PathBuf>,
    hf_base: String,
    local_http: reqwest::Client,
    hub_http: reqwest::Client,
    sink: Sink,
    state: Mutex<State>,
    server: tokio::sync::Mutex<Option<Server>>,
    endpoint: Mutex<Option<Endpoint>>,
    gen_cancel: Mutex<Option<CancellationToken>>,
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
        let hw = Hardware::detect();
        let settings = Settings::load(&cfg.paths.settings());
        let verified = VerifiedCache::load(&cfg.paths.verified_cache());
        // llama-server is local: never send its traffic through a proxy.
        let local_http = reqwest::Client::builder().no_proxy().build().expect("http client");
        let hub_http = reqwest::Client::builder()
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
                    verified,
                    load_epoch: 0,
                }),
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
        *self.inner.downloads.lock().unwrap() = Some(d);
    }

    fn downloads(&self) -> Downloads {
        self.inner.downloads.lock().unwrap().clone().expect("downloads initialised")
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
        }
    }

    pub fn emit_snapshot(&self) {
        let s = self.snapshot();
        self.emit(Event::Snapshot { snapshot: Box::new(s) });
    }

    fn save_settings(&self, s: &Settings) {
        let _ = s.save(&self.inner.paths.settings());
        self.emit(Event::Settings { settings: Self::public_settings(s) });
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
        if changed {
            if let Some(f) = file {
                self.load(f);
            }
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
                if root.join(c.dir_name()).join(crate::paths::server_exe_name()).is_file() {
                    return c;
                }
                cur = c.fallback();
            }
        }
        v
    }

    // ----- models ------------------------------------------------------------

    pub fn rescan(&self) {
        let models = {
            let st = self.inner.state.lock().unwrap();
            models::scan(&self.inner.paths.models(), self.inner.hw.total_ram, &st.verified)
        };
        self.inner.state.lock().unwrap().models = models.clone();
        self.emit(Event::Models { models });
    }

    pub fn models(&self) -> Vec<ModelInfo> {
        self.inner.state.lock().unwrap().models.clone()
    }

    fn pick_initial_model(&self) -> Option<String> {
        let st = self.inner.state.lock().unwrap();
        let usable = |f: &str| st.models.iter().any(|m| m.file == f && !m.broken);
        if let Some(f) = st.settings.model.clone().filter(|f| usable(f)) {
            return Some(f);
        }
        if let Some(d) = default_model().filter(|d| usable(&d.file)) {
            return Some(d.file.clone());
        }
        st.models.iter().find(|m| !m.broken).map(|m| m.file.clone())
    }

    /// Hash bundled models once (cached by size and date), so a damaged USB copy is caught.
    fn verify_bundled_in_background(&self) {
        let pending = self.inner.state.lock().unwrap().verified.pending(&self.inner.paths.models());
        if pending.is_empty() {
            return;
        }
        let me = self.clone();
        tokio::task::spawn_blocking(move || {
            for p in pending {
                let file = p.file_name().unwrap().to_string_lossy().into_owned();
                let Some(pin) = crate::pins::pinned_model(&file) else { continue };
                let total = pin.size;
                let mut last = std::time::Instant::now();
                let r = models::sha256_file(&p, |done| {
                    if last.elapsed() > Duration::from_millis(500) {
                        last = std::time::Instant::now();
                        me.emit(Event::VerifyProgress { file: file.clone(), done, total });
                    }
                });
                if me.inner.shutdown.is_cancelled() {
                    return;
                }
                if let Ok(h) = r {
                    me.inner.state.lock().unwrap().verified.set(&p, h == pin.sha256);
                }
                me.emit(Event::VerifyProgress { file: file.clone(), done: total, total });
                me.rescan();
            }
        });
    }

    pub fn import_model(&self, src: PathBuf) {
        let me = self.clone();
        tokio::task::spawn_blocking(move || {
            let file = src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let mut last = std::time::Instant::now();
            let r = models::import(&src, &me.inner.paths.models(), |done, total| {
                if last.elapsed() > Duration::from_millis(200) || done == total {
                    last = std::time::Instant::now();
                    me.emit(Event::ImportProgress { file: file.clone(), done, total });
                }
            });
            match r {
                Ok(name) => {
                    me.rescan();
                    me.emit(Event::ImportFinished { file: Some(name.clone()), error: None });
                    let idle = matches!(me.inner.state.lock().unwrap().engine, EngineState::Idle);
                    if idle {
                        me.load(name);
                    }
                    me.verify_bundled_in_background();
                }
                Err(e) => me.emit(Event::ImportFinished { file: None, error: Some(e) }),
            }
        });
    }

    /// Delete a model. If it is the one loaded, the server is stopped first:
    /// Windows cannot delete a file that is memory-mapped.
    pub async fn delete_model(&self, file: &str) -> Result<(), UserError> {
        let loaded = self.inner.endpoint.lock().unwrap().as_ref().map(|e| e.file.clone());
        let engine_file = match &self.inner.state.lock().unwrap().engine {
            EngineState::Loading { file, .. } | EngineState::Ready { file, .. } | EngineState::Failed { file, .. } => Some(file.clone()),
            EngineState::Idle => None,
        };
        let is_current = loaded.as_deref() == Some(file) || engine_file.as_deref() == Some(file);
        if is_current {
            self.stop_generation();
            self.unload().await;
        }
        let dir = self.inner.paths.models();
        let f = file.to_string();
        let r = tokio::task::spawn_blocking(move || models::delete(&dir, &f)).await.unwrap_or_else(|e| Err(UserError::internal(e.to_string())));
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
        if is_current {
            if let Some(next) = self.pick_initial_model() {
                self.load(next);
            }
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
    pub fn reload(&self) {
        let file = self.inner.state.lock().unwrap().settings.model.clone();
        if let Some(f) = file {
            self.load(f);
        }
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
        self.stop_generation();
        let cancel = CancellationToken::new();
        if let Some(old) = self.inner.load_cancel.lock().unwrap().replace(cancel.clone()) {
            old.cancel();
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
        self.set_engine(EngineState::Loading { file: file.clone(), name: name.clone() });
        let me = self.clone();
        tokio::spawn(async move {
            let result = me.load_inner(&file, &name, &cancel).await;
            let current = me.inner.state.lock().unwrap().load_epoch == epoch;
            if !current || cancel.is_cancelled() {
                return;
            }
            match result {
                Ok(ctx) => me.set_engine(EngineState::Ready { file, name, context: ctx }),
                Err(error) => me.set_engine(EngineState::Failed { file, name, error }),
            }
        });
    }

    async fn load_inner(&self, file: &str, name: &str, cancel: &CancellationToken) -> Result<u32, UserError> {
        let Some(root) = self.inner.llama_root.clone() else { return Err(UserError::server_missing()) };
        let mut guard = self.inner.server.lock().await;
        if cancel.is_cancelled() {
            return Err(UserError::server_start());
        }
        if let Some(mut old) = guard.take() {
            tokio::task::spawn_blocking(move || old.stop()).await.ok();
        }
        if let Some(m) = self.inner.state.lock().unwrap().models.iter().find(|m| m.file == file) {
            if m.broken {
                return Err(UserError::model_corrupt(name));
            }
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
            let mut server = Server::spawn(&cfg, &root)?;
            match server.wait_ready(&self.inner.local_http, cancel).await {
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
                    let Ok(mut guard) = me.inner.server.try_lock() else { continue };
                    guard.as_mut().and_then(|s| s.exited()).is_some()
                };
                if died {
                    let name = me.model_name(&file);
                    me.set_engine(EngineState::Failed { file, name, error: UserError::server_crashed() });
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
        if let Some(c) = self.inner.gen_cancel.lock().unwrap().take() {
            c.cancel();
        }
    }

    /// Send a message. Errors are returned (and not emitted) when nothing started.
    pub fn send(&self, text: &str) -> Result<u64, UserError> {
        let text = text.trim_end().to_string();
        if text.trim().is_empty() {
            return Err(UserError::internal("empty message"));
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
            if st.generating {
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
            };
            st.turns.push(turn.clone());
            (turn, messages, st.settings.temperature)
        };
        *self.inner.gen_cancel.lock().unwrap() = Some(cancel.clone());
        self.emit(Event::Generating { generating: true });
        self.emit(Event::TurnStarted { turn: turn.clone() });
        let id = turn.id;
        let me = self.clone();
        tokio::spawn(async move {
            let on = |d: StreamDelta| me.apply_delta(id, d);
            let end = chat::stream(
                &me.inner.local_http,
                ChatRequest { base_url: &ep.base_url, api_key: &ep.api_key, messages, temperature },
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
            let Some(t) = st.turns.iter_mut().find(|t| t.id == id) else { return };
            match d {
                StreamDelta::Reasoning(r) => {
                    t.reasoning.push_str(&r);
                    Event::TurnDelta { id, reasoning: Some(r), answer: None, reasoning_from_answer: false }
                }
                StreamDelta::Answer(a) => {
                    t.answer.push_str(&a);
                    Event::TurnDelta { id, reasoning: None, answer: Some(a), reasoning_from_answer: false }
                }
                StreamDelta::AnswerWasReasoning => {
                    let moved = std::mem::take(&mut t.answer);
                    t.reasoning = moved + &t.reasoning;
                    Event::TurnDelta { id, reasoning: None, answer: None, reasoning_from_answer: true }
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
        let mut crashed = false;
        let turn = {
            let mut st = self.inner.state.lock().unwrap();
            st.generating = false;
            let Some(t) = st.turns.iter_mut().find(|t| t.id == id) else { return };
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
                    crashed = true;
                }
            }
            // Reasoning that never reached an answer still gets a duration for the label.
            t.clone()
        };
        *self.inner.gen_cancel.lock().unwrap() = None;
        self.emit(Event::TurnFinished { turn });
        self.emit(Event::Generating { generating: false });
        if crashed {
            let still_same = self.inner.endpoint.lock().unwrap().as_ref().map(|e| e.base_url == ep.base_url).unwrap_or(false);
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
        Hub::new(self.inner.hub_http.clone(), self.inner.hf_base.clone(), Some(token))
    }

    pub async fn hub_online(&self) -> bool {
        self.hub().online().await
    }

    pub async fn hub_search(&self, q: &str) -> Result<Vec<RepoHit>, UserError> {
        self.hub().search(q).await
    }

    pub async fn hub_details(&self, repo: &str) -> Result<RepoFiles, UserError> {
        self.hub().details(repo, self.inner.hw.total_ram).await
    }

    pub fn download_start(&self, req: DownloadRequest) -> Result<String, UserError> {
        self.downloads().start(req)
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
        if let Ok(mut guard) = self.inner.server.try_lock() {
            if let Some(mut s) = guard.take() {
                s.stop();
            }
        }
    }

    pub fn data_dir(&self) -> &Path {
        &self.inner.paths.home
    }
}
