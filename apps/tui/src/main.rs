//! Trurl in the terminal. Same core, same behaviour as the desktop app.
//!
//!   trurl              start
//!   trurl --version    print the version
//!   trurl --help       keys and folders
//!
//! Used on Linux, and on Windows 7/8.1 where the desktop app cannot run.

mod selfupdate;
mod state;
mod text;
mod view;

use ratatui::crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event as TermEvent, KeyCode, KeyEvent,
    KeyEventKind, KeyModifiers,
};
use ratatui::crossterm::execute;
use state::*;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use trurl_core::app::{App, AppConfig};
use trurl_core::downloads::{DownloadRequest, DownloadStatus};
use trurl_core::errors::UserError;
use trurl_core::events::Event;
use trurl_core::hardware::RamFit;
use trurl_core::settings::CONTEXT_CHOICES;

/// Things that happen to the terminal app, from any source.
enum Msg {
    Core(Event),
    Key(KeyEvent),
    Paste(String),
    Resize,
    HubOnline(bool),
    HubResults(Result<Vec<trurl_core::hub::RepoHit>, UserError>),
    HubDetails(Result<trurl_core::hub::RepoFiles, UserError>),
    Update(
        Result<Option<trurl_core::update::UpdateInfo>, UserError>,
        bool,
    ),
    UpdateProgress(u64, Option<u64>),
    UpdateDone(Result<std::path::PathBuf, UserError>),
    Tick,
}

const UPDATE_EVERY: Duration = Duration::from_secs(24 * 60 * 60);

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("trurl {}", trurl_core::VERSION);
        return;
    }
    if args.iter().any(|a| a == "--help" || a == "-h") {
        let cfg = AppConfig::from_env("terminal");
        println!(
            "Trurl {} by Cyberiad Labs: run open-weight language models offline.\n\n\
             Keys: Enter send, Alt+Enter new line, Esc stop, Ctrl+N new chat, Ctrl+T show thinking,\n\
             F2 models, F3 settings, F4 Hugging Face, F1 about, Ctrl+Q quit.\n\n\
             Models folder: {}\nllama-server builds: {}",
            trurl_core::VERSION,
            cfg.paths.models().display(),
            cfg.llama_root
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "NOT FOUND".into())
        );
        return;
    }
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .expect("runtime");
    let code = rt.block_on(run());
    std::process::exit(code);
}

async fn run() -> i32 {
    let (tx, mut rx) = mpsc::unbounded_channel::<Msg>();
    let core_tx = tx.clone();
    let core = App::start(
        AppConfig::from_env("terminal"),
        Arc::new(move |e| {
            let _ = core_tx.send(Msg::Core(e));
        }),
    );

    // Terminal setup, restored on any exit, including a panic.
    let mut terminal = ratatui::init();
    let _ = execute!(std::io::stdout(), EnableBracketedPaste);
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(std::io::stdout(), DisableBracketedPaste);
        ratatui::restore();
        default_hook(info);
    }));

    // Keyboard on a plain thread: crossterm's blocking read is the most portable.
    let key_tx = tx.clone();
    std::thread::spawn(move || {
        loop {
            match event::read() {
                Ok(TermEvent::Key(k)) if k.kind != KeyEventKind::Release => {
                    if key_tx.send(Msg::Key(k)).is_err() {
                        break;
                    }
                }
                Ok(TermEvent::Paste(s)) => {
                    let _ = key_tx.send(Msg::Paste(s));
                }
                Ok(TermEvent::Resize(_, _)) => {
                    let _ = key_tx.send(Msg::Resize);
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
    });

    // Update checks: once after start (the model loads first), then every 24 h.
    let up_tx = tx.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(15)).await;
        loop {
            let _ = up_tx.send(Msg::Update(selfupdate::check().await, false));
            tokio::time::sleep(UPDATE_EVERY).await;
        }
    });
    // A slow tick keeps "Loading…" and download speeds fresh without busy redraws.
    let tick_tx = tx.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(500)).await;
            if tick_tx.send(Msg::Tick).is_err() {
                break;
            }
        }
    });

    let mut ui = Ui::new();
    ui.apply_snapshot(core.snapshot());
    let mut restart_into: Option<std::path::PathBuf> = None;
    loop {
        let _ = terminal.draw(|f| view::draw(f, &ui));
        let Some(first) = rx.recv().await else { break };
        // Drain everything already queued, then draw once: cheap while streaming.
        let mut batch = vec![first];
        while let Ok(m) = rx.try_recv() {
            batch.push(m);
        }
        for m in batch {
            if let Some(path) = handle(&mut ui, &core, &tx, m).await {
                restart_into = Some(path);
            }
        }
        if ui.quit {
            break;
        }
    }
    core.shutdown().await;
    let _ = execute!(std::io::stdout(), DisableBracketedPaste);
    ratatui::restore();
    if let Some(exe) = restart_into {
        selfupdate::restart(&exe);
    }
    0
}

/// Apply one message. Returns a path when an update was installed and Trurl should restart.
async fn handle(
    ui: &mut Ui,
    core: &App,
    tx: &mpsc::UnboundedSender<Msg>,
    m: Msg,
) -> Option<std::path::PathBuf> {
    match m {
        Msg::Core(e) => ui.apply(e),
        Msg::Paste(s) => {
            if let Some((_, ed)) = ui.prompt.as_mut() {
                ed.insert_str(&s);
            } else if ui.screen == Screen::Chat {
                ui.input.insert_str(&s);
            } else if ui.screen == Screen::Hub {
                ui.hub_query.insert_str(&s.replace('\n', " "));
            } else if ui.screen == Screen::Token {
                ui.token.insert_str(s.trim());
            }
        }
        Msg::Resize | Msg::Tick => {}
        Msg::HubOnline(on) => {
            ui.hub_view = if on {
                HubView::Search
            } else {
                HubView::Offline
            };
            ui.hub_focus = HubFocus::Input;
        }
        Msg::HubResults(r) => {
            ui.hub_busy = false;
            match r {
                Ok(v) => {
                    ui.hub_results = v;
                    ui.hub_sel = 0;
                    ui.hub_focus = if ui.hub_results.is_empty() {
                        HubFocus::Input
                    } else {
                        HubFocus::Results
                    };
                }
                Err(e) => {
                    if e.code == trurl_core::ErrorCode::HubFailed {
                        ui.hub_view = HubView::Offline;
                    }
                    ui.notice = Some(e);
                }
            }
        }
        Msg::HubDetails(r) => match r {
            Ok(d) => {
                ui.hub_file_sel = d
                    .files
                    .iter()
                    .position(|f| f.quant.as_deref() == Some("Q4_K_M"))
                    .unwrap_or(0);
                ui.hub_details = Some(d);
                ui.hub_focus = HubFocus::Files;
                ui.hub_readme_scroll = 0;
            }
            Err(e) => {
                ui.hub_view = HubView::Search;
                ui.notice = Some(e);
            }
        },
        Msg::Update(r, manual) => match r {
            Ok(Some(u)) => {
                if !matches!(ui.update, UpdateState::Hidden(ref h) if h.version == u.version && !manual)
                {
                    ui.update = UpdateState::Available(u);
                }
                ui.update_msg = None;
            }
            Ok(None) => {
                if manual {
                    ui.update_msg = Some("You have the latest version.".into());
                }
            }
            Err(e) => {
                if manual {
                    ui.update_msg = Some(format!("{} {}", e.title, e.hint));
                }
            }
        },
        Msg::UpdateProgress(done, total) => {
            if let UpdateState::Downloading(u, _, _) = &ui.update {
                ui.update = UpdateState::Downloading(u.clone(), done, total);
            }
        }
        Msg::UpdateDone(r) => match r {
            Ok(exe) => {
                ui.update_msg = Some("Update installed. Restarting Trurl…".into());
                ui.quit = true;
                return Some(exe);
            }
            Err(e) => ui.update = UpdateState::Failed(e),
        },
        Msg::Key(k) => on_key(ui, core, tx, k).await,
    }
    None
}

fn ctrl(k: &KeyEvent, c: char) -> bool {
    k.modifiers.contains(KeyModifiers::CONTROL) && k.code == KeyCode::Char(c)
}

async fn on_key(ui: &mut Ui, core: &App, tx: &mpsc::UnboundedSender<Msg>, k: KeyEvent) {
    // Popups first.
    if let Some(c) = ui.confirm.clone() {
        match k.code {
            KeyCode::Enter | KeyCode::Char('y') => {
                ui.confirm = None;
                match c {
                    Confirm::Delete { file, .. } => {
                        if let Err(e) = core.delete_model(&file).await {
                            ui.notice = Some(e);
                        }
                    }
                    Confirm::LoadRed { file, .. } => {
                        core.select_model(&file);
                        ui.screen = Screen::Chat;
                    }
                }
            }
            KeyCode::Esc | KeyCode::Char('n') => ui.confirm = None,
            _ => {}
        }
        return;
    }
    if let Some((kind, ed)) = ui.prompt.as_mut() {
        let kind = *kind;
        match k.code {
            KeyCode::Esc => ui.prompt = None,
            KeyCode::Char('s')
                if k.modifiers.contains(KeyModifiers::CONTROL)
                    && kind == PromptKind::SystemPrompt =>
            {
                core.set_system_prompt(ed.text.trim_end());
                ui.prompt = None;
            }
            KeyCode::Enter if kind == PromptKind::ImportPath => {
                let p = ed.text.trim().trim_matches(['"', '\'']).to_string();
                ui.prompt = None;
                if !p.is_empty() {
                    let path = std::path::PathBuf::from(expand_home(&p));
                    if path.is_file() {
                        core.import_model(path);
                    } else {
                        ui.notice =
                            Some(UserError::import_failed().with_detail(format!("no file at {p}")));
                    }
                }
            }
            _ => edit(ed, &k, kind == PromptKind::SystemPrompt),
        }
        return;
    }

    // Global keys.
    if ctrl(&k, 'q') || ctrl(&k, 'c') {
        ui.quit = true;
        return;
    }
    match k.code {
        KeyCode::F(1) => {
            ui.screen = Screen::About;
            ui.license_open = None;
            return;
        }
        KeyCode::F(2) => {
            ui.screen = Screen::Models;
            return;
        }
        KeyCode::F(3) => {
            ui.screen = Screen::Settings;
            ui.pending_context = None;
            return;
        }
        KeyCode::F(4) => {
            open_hub(ui, core, tx);
            return;
        }
        KeyCode::F(12) => {
            ui.screen = Screen::Token;
            ui.token.clear();
            return;
        }
        _ => {}
    }
    if let (Some('u' | 'U'), true) = (
        char_of(&k),
        matches!(ui.update, UpdateState::Available(_))
            && ui.screen == Screen::Chat
            && ui.input.text.is_empty(),
    ) {
        start_update(ui, core, tx);
        return;
    }
    if let (Some('l' | 'L'), UpdateState::Available(u)) = (char_of(&k), &ui.update) {
        if ui.screen == Screen::Chat && ui.input.text.is_empty() {
            ui.update = UpdateState::Hidden(u.clone());
            return;
        }
    }

    match ui.screen {
        Screen::Chat => chat_key(ui, core, k),
        Screen::Settings => settings_key(ui, core, k),
        Screen::Models => models_key(ui, core, tx, k),
        Screen::Hub => hub_key(ui, core, tx, k),
        Screen::About => about_key(ui, tx, k),
        Screen::Token => match k.code {
            KeyCode::Esc => ui.screen = Screen::Chat,
            KeyCode::Enter => {
                core.set_hf_token(ui.token.text.trim());
                ui.token.clear();
                ui.update_msg = Some("Token saved.".into());
                ui.screen = Screen::Chat;
            }
            KeyCode::Char('d') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                core.set_hf_token("");
                ui.update_msg = Some("Token removed.".into());
                ui.screen = Screen::Chat;
            }
            _ => edit(&mut ui.token, &k, false),
        },
    }
}

fn char_of(k: &KeyEvent) -> Option<char> {
    match k.code {
        KeyCode::Char(c)
            if !k.modifiers.contains(KeyModifiers::CONTROL)
                && !k.modifiers.contains(KeyModifiers::ALT) =>
        {
            Some(c)
        }
        _ => None,
    }
}

fn expand_home(p: &str) -> String {
    if let Some(rest) = p.strip_prefix("~/") {
        if let Some(h) = std::env::var_os("HOME") {
            return format!("{}/{rest}", h.to_string_lossy());
        }
    }
    p.to_string()
}

/// Shared text editing keys. `multiline`: Enter inserts a new line.
fn edit(ed: &mut text::Editor, k: &KeyEvent, multiline: bool) {
    match k.code {
        KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => ed.insert(c),
        KeyCode::Enter if multiline => ed.insert('\n'),
        KeyCode::Backspace => ed.backspace(),
        KeyCode::Delete => ed.delete(),
        KeyCode::Left => ed.left(),
        KeyCode::Right => ed.right(),
        KeyCode::Home => ed.home(),
        KeyCode::End => ed.end(),
        KeyCode::Char('u') if k.modifiers.contains(KeyModifiers::CONTROL) => ed.clear(),
        _ => {}
    }
}

fn chat_key(ui: &mut Ui, core: &App, k: KeyEvent) {
    if ctrl(&k, 'n') {
        core.new_chat();
        return;
    }
    if ctrl(&k, 't') {
        if let Some(t) = ui.turns.iter().rev().find(|t| !t.reasoning.is_empty()) {
            let id = t.id;
            if !ui.toggled.remove(&id) {
                ui.toggled.insert(id);
            }
        }
        return;
    }
    if ctrl(&k, 'r') {
        core.reload();
        return;
    }
    match k.code {
        KeyCode::Esc => {
            if ui.generating {
                core.stop_generation();
            } else {
                ui.notice = None;
                ui.update_msg = None;
                if matches!(ui.update, UpdateState::Failed(_)) {
                    ui.update = UpdateState::None;
                }
            }
        }
        KeyCode::Enter
            if k.modifiers.contains(KeyModifiers::ALT)
                || k.modifiers.contains(KeyModifiers::SHIFT) =>
        {
            ui.input.insert('\n')
        }
        KeyCode::Char('j') if k.modifiers.contains(KeyModifiers::CONTROL) => ui.input.insert('\n'),
        KeyCode::Enter => {
            if ui.input.text.trim().is_empty() || ui.generating {
                return;
            }
            match core.send(&ui.input.text) {
                Ok(_) => {
                    ui.input.clear();
                    ui.notice = None;
                }
                Err(e) => ui.notice = Some(e),
            }
        }
        KeyCode::PageUp => ui.scroll += 10,
        KeyCode::PageDown => ui.scroll = ui.scroll.saturating_sub(10),
        KeyCode::Up if k.modifiers.contains(KeyModifiers::CONTROL) => ui.scroll += 1,
        KeyCode::Down if k.modifiers.contains(KeyModifiers::CONTROL) => {
            ui.scroll = ui.scroll.saturating_sub(1)
        }
        KeyCode::End if ui.input.text.is_empty() => ui.scroll = 0,
        _ => edit(&mut ui.input, &k, false),
    }
}

fn settings_key(ui: &mut Ui, core: &App, k: KeyEvent) {
    let Some(s) = ui.settings.clone() else { return };
    let field = SettingsField::ALL[ui.settings_sel.min(3)];
    match k.code {
        KeyCode::Esc => {
            ui.pending_context = None;
            ui.screen = Screen::Chat;
        }
        KeyCode::Up => ui.settings_sel = ui.settings_sel.saturating_sub(1),
        KeyCode::Down | KeyCode::Tab => ui.settings_sel = (ui.settings_sel + 1).min(3),
        KeyCode::Left | KeyCode::Right => {
            let up = k.code == KeyCode::Right;
            match field {
                SettingsField::Temperature => {
                    core.set_temperature(s.temperature + if up { 0.1 } else { -0.1 })
                }
                SettingsField::Context => {
                    let cur = ui.pending_context.unwrap_or(s.context_size);
                    let i = CONTEXT_CHOICES.iter().position(|c| *c == cur).unwrap_or(3);
                    let j = if up {
                        (i + 1).min(CONTEXT_CHOICES.len() - 1)
                    } else {
                        i.saturating_sub(1)
                    };
                    ui.pending_context = Some(CONTEXT_CHOICES[j]);
                }
                _ => {}
            }
        }
        KeyCode::Enter => match field {
            SettingsField::Model => ui.screen = Screen::Models,
            SettingsField::SystemPrompt => {
                ui.prompt = Some((
                    PromptKind::SystemPrompt,
                    text::Editor::with(&s.system_prompt),
                ))
            }
            SettingsField::Context => {
                if let Some(c) = ui.pending_context.take() {
                    core.set_context(c);
                }
            }
            SettingsField::Temperature => {}
        },
        KeyCode::Char('r') | KeyCode::Char('R') => match field {
            SettingsField::Temperature => core.set_temperature(s.default_temperature),
            SettingsField::Context => {
                ui.pending_context = None;
                core.set_context(s.default_context);
            }
            SettingsField::SystemPrompt => core.set_system_prompt(""),
            SettingsField::Model => {}
        },
        _ => {}
    }
}

fn models_key(ui: &mut Ui, core: &App, tx: &mpsc::UnboundedSender<Msg>, k: KeyEvent) {
    let n = ui.models.len();
    match k.code {
        KeyCode::Esc => ui.screen = Screen::Chat,
        KeyCode::Up => ui.models_sel = ui.models_sel.saturating_sub(1),
        KeyCode::Down => ui.models_sel = (ui.models_sel + 1).min(n.saturating_sub(1)),
        KeyCode::Enter => {
            if let Some(m) = ui.models.get(ui.models_sel) {
                if m.fit == RamFit::Red {
                    ui.confirm = Some(Confirm::LoadRed {
                        file: m.file.clone(),
                        name: m.name.clone(),
                    });
                } else {
                    core.select_model(&m.file);
                    ui.screen = Screen::Chat;
                }
            }
        }
        KeyCode::Char('d') | KeyCode::Char('D') | KeyCode::Delete => {
            if let Some(m) = ui.models.get(ui.models_sel) {
                ui.confirm = Some(Confirm::Delete {
                    file: m.file.clone(),
                    name: m.name.clone(),
                    size: m.size,
                });
            }
        }
        KeyCode::Char('i') | KeyCode::Char('I') => {
            ui.prompt = Some((PromptKind::ImportPath, text::Editor::default()))
        }
        KeyCode::Char('h') | KeyCode::Char('H') => open_hub(ui, core, tx),
        _ => {}
    }
}

fn open_hub(ui: &mut Ui, core: &App, tx: &mpsc::UnboundedSender<Msg>) {
    ui.screen = Screen::Hub;
    if ui.hub_view == HubView::Details && ui.hub_details.is_some() {
        return;
    }
    ui.hub_view = HubView::Checking;
    let (c, t) = (core.clone(), tx.clone());
    tokio::spawn(async move {
        let _ = t.send(Msg::HubOnline(c.hub_online().await));
    });
}

fn hub_key(ui: &mut Ui, core: &App, tx: &mpsc::UnboundedSender<Msg>, k: KeyEvent) {
    // Downloads are controllable from every hub view.
    if ui.hub_focus == HubFocus::Downloads {
        if let Some(d) = ui.downloads.get(ui.dl_sel).cloned() {
            match k.code {
                KeyCode::Char('p') | KeyCode::Char('P') | KeyCode::Char(' ') => {
                    if matches!(d.status, DownloadStatus::Paused | DownloadStatus::Failed) {
                        if let Err(e) = core.download_resume(&d.id) {
                            ui.notice = Some(e);
                        }
                    } else {
                        core.download_pause(&d.id);
                    }
                    return;
                }
                KeyCode::Char('x') | KeyCode::Char('X') => {
                    core.download_cancel(&d.id);
                    return;
                }
                KeyCode::Up => {
                    ui.dl_sel = ui.dl_sel.saturating_sub(1);
                    return;
                }
                KeyCode::Down => {
                    ui.dl_sel = (ui.dl_sel + 1).min(ui.downloads.len().saturating_sub(1));
                    return;
                }
                _ => {}
            }
        }
    }
    if k.code == KeyCode::Tab {
        ui.hub_focus = match (ui.hub_view.clone(), ui.hub_focus) {
            (_, HubFocus::Downloads) => match ui.hub_view {
                HubView::Details => HubFocus::Files,
                _ => HubFocus::Input,
            },
            (HubView::Search, HubFocus::Input) if !ui.hub_results.is_empty() => HubFocus::Results,
            _ if !ui.downloads.is_empty() => HubFocus::Downloads,
            (HubView::Details, _) => HubFocus::Files,
            _ => HubFocus::Input,
        };
        return;
    }
    match ui.hub_view {
        HubView::Checking => {
            if k.code == KeyCode::Esc {
                ui.screen = Screen::Chat;
            }
        }
        HubView::Offline => match k.code {
            KeyCode::Esc => ui.screen = Screen::Chat,
            KeyCode::Enter => open_hub(ui, core, tx),
            _ => {}
        },
        HubView::Search => match (ui.hub_focus, k.code) {
            (_, KeyCode::Esc) => ui.screen = Screen::Chat,
            (HubFocus::Input, KeyCode::Enter) => {
                let q = ui.hub_query.text.trim().to_string();
                if q.is_empty() {
                    return;
                }
                ui.hub_busy = true;
                let (c, t) = (core.clone(), tx.clone());
                tokio::spawn(async move {
                    let _ = t.send(Msg::HubResults(c.hub_search(&q).await));
                });
            }
            (HubFocus::Input, KeyCode::Down) if !ui.hub_results.is_empty() => {
                ui.hub_focus = HubFocus::Results
            }
            (HubFocus::Input, _) => edit(&mut ui.hub_query, &k, false),
            (HubFocus::Results, KeyCode::Up) => {
                if ui.hub_sel == 0 {
                    ui.hub_focus = HubFocus::Input;
                } else {
                    ui.hub_sel -= 1;
                }
            }
            (HubFocus::Results, KeyCode::Down) => {
                ui.hub_sel = (ui.hub_sel + 1).min(ui.hub_results.len().saturating_sub(1))
            }
            (HubFocus::Results, KeyCode::Enter) => {
                if let Some(h) = ui.hub_results.get(ui.hub_sel) {
                    let repo = h.id.clone();
                    ui.hub_view = HubView::Details;
                    ui.hub_details = None;
                    let (c, t) = (core.clone(), tx.clone());
                    tokio::spawn(async move {
                        let _ = t.send(Msg::HubDetails(c.hub_details(&repo).await));
                    });
                }
            }
            _ => {}
        },
        HubView::Details => match k.code {
            KeyCode::Esc | KeyCode::Backspace => {
                ui.hub_view = HubView::Search;
                ui.hub_focus = if ui.hub_results.is_empty() {
                    HubFocus::Input
                } else {
                    HubFocus::Results
                };
            }
            KeyCode::Up => ui.hub_file_sel = ui.hub_file_sel.saturating_sub(1),
            KeyCode::Down => {
                let n = ui.hub_details.as_ref().map(|d| d.files.len()).unwrap_or(0);
                ui.hub_file_sel = (ui.hub_file_sel + 1).min(n.saturating_sub(1));
            }
            KeyCode::PageDown => ui.hub_readme_scroll = ui.hub_readme_scroll.saturating_add(10),
            KeyCode::PageUp => ui.hub_readme_scroll = ui.hub_readme_scroll.saturating_sub(10),
            KeyCode::Enter => {
                let Some(d) = &ui.hub_details else { return };
                let Some(f) = d.files.get(ui.hub_file_sel) else {
                    return;
                };
                if f.split {
                    ui.notice = Some(
                        UserError::not_gguf(&f.path).with_detail("split files are not supported"),
                    );
                    return;
                }
                let req = DownloadRequest {
                    repo: d.id.clone(),
                    file: f.path.clone(),
                    revision: d.revision.clone(),
                    size: f.size,
                    sha256: f.sha256.clone(),
                };
                match core.download_start(req) {
                    Ok(_) => ui.hub_focus = HubFocus::Downloads,
                    Err(e) => ui.notice = Some(e),
                }
            }
            _ => {}
        },
    }
}

fn about_key(ui: &mut Ui, tx: &mpsc::UnboundedSender<Msg>, k: KeyEvent) {
    let n = trurl_core::licenses::all().len();
    if ui.license_open.is_some() {
        match k.code {
            KeyCode::Esc => ui.license_open = None,
            KeyCode::PageDown | KeyCode::Down => {
                ui.about_scroll =
                    ui.about_scroll
                        .saturating_add(if k.code == KeyCode::Down { 1 } else { 20 })
            }
            KeyCode::PageUp | KeyCode::Up => {
                ui.about_scroll =
                    ui.about_scroll
                        .saturating_sub(if k.code == KeyCode::Up { 1 } else { 20 })
            }
            _ => {}
        }
        return;
    }
    match k.code {
        KeyCode::Esc => ui.screen = Screen::Chat,
        KeyCode::Up => ui.license_sel = ui.license_sel.saturating_sub(1),
        KeyCode::Down => ui.license_sel = (ui.license_sel + 1).min(n - 1),
        KeyCode::Enter => {
            ui.license_open = Some(ui.license_sel);
            ui.about_scroll = 0;
        }
        KeyCode::Char('u') | KeyCode::Char('U') => {
            ui.update_msg = Some("Checking for updates…".into());
            let t = tx.clone();
            tokio::spawn(async move {
                let _ = t.send(Msg::Update(selfupdate::check().await, true));
            });
        }
        _ => {}
    }
}

fn start_update(ui: &mut Ui, core: &App, tx: &mpsc::UnboundedSender<Msg>) {
    let UpdateState::Available(info) = ui.update.clone() else {
        return;
    };
    if ui.generating {
        ui.notice = Some(UserError::update_wait_for_answer());
        return;
    }
    if !selfupdate::can_install() {
        ui.update_msg = Some(format!(
            "Download version {} from https://github.com/jbarbier/trurl/releases",
            info.version
        ));
        return;
    }
    ui.update = UpdateState::Downloading(info.clone(), 0, None);
    let (t, c) = (tx.clone(), core.clone());
    tokio::spawn(async move {
        let pt = t.clone();
        let r = selfupdate::install(
            &info,
            move |d, total| {
                let _ = pt.send(Msg::UpdateProgress(d, total));
            },
            &c,
        )
        .await;
        let _ = t.send(Msg::UpdateDone(r));
    });
}
