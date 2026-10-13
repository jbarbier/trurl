//! Trurl in the terminal. Same core, same behaviour as the desktop app.
//!
//!   trurl              start
//!   trurl --version    print the version
//!   trurl --help       keys and folders
//!
//! Used on Linux, and on Windows 7/8.1 where the desktop app cannot run.

#[cfg(test)]
mod parity_tests;
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
use trurl_core::events::{EngineState, Event};
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

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("trurl {}", trurl_core::VERSION);
        return;
    }
    if args.iter().any(|a| a == "--help" || a == "-h") {
        let cfg = AppConfig::from_env("terminal");
        println!(
            "Trurl {} by Julien Barbier, for Cyberiad Labs: run open-weight language models offline.\n\n\
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
    let cfg = AppConfig::from_env("terminal");
    let _instance = match cfg.paths.lock_instance() {
        Ok(f) => Some(f),
        Err(trurl_core::paths::InstanceError::AlreadyRunning) => {
            eprintln!(
                "Trurl is already open in another terminal window.\n\
                 Use that one, or close it first: two copies would load two models,\n\
                 and most laptops cannot hold both in memory."
            );
            return 1;
        }
        // Cannot lock (read-only folder, odd file system): carry on, as before.
        Err(trurl_core::paths::InstanceError::Io(_)) => None,
    };
    let (tx, mut rx) = mpsc::unbounded_channel::<Msg>();
    let core_tx = tx.clone();
    let core = App::start(
        cfg,
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
    let up_core = core.clone();
    tokio::spawn(async move {
        tokio::time::sleep(trurl_core::update::FIRST_CHECK_AFTER).await;
        let mut schedule = trurl_core::update::CheckSchedule::default();
        loop {
            let now = std::time::SystemTime::now();
            if schedule.due(now) && !up_core.is_generating() {
                if up_tx
                    .send(Msg::Update(selfupdate::check().await, false))
                    .is_err()
                {
                    break;
                }
                schedule.checked(now);
            }
            tokio::time::sleep(trurl_core::update::SCHEDULE_TICK).await;
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
        Msg::Core(e) => {
            ui.apply(e);
            if ui.queued && !matches!(ui.engine, EngineState::Loading { .. }) {
                ui.queued = false;
                if matches!(ui.engine, EngineState::Ready { .. }) {
                    send_input(ui, core);
                }
            }
        }
        Msg::Paste(s) => {
            if let Some((_, ed)) = ui.prompt.as_mut() {
                ed.insert_str(&s);
            } else if ui.screen == Screen::Chat {
                ui.input.insert_str(&s);
            } else if ui.screen == Screen::Hub {
                ui.hub_query.insert_str(&s.replace('\n', " "));
            } else if ui.screen == Screen::Token {
                ui.token.insert_str(s.trim());
            } else if ui.screen == Screen::Models {
                // A file dragged into the window arrives as a pasted path.
                let path = s.trim().trim_matches(['"', '\'']).to_string();
                ui.prompt = Some((PromptKind::ImportPath, text::Editor::with(&path)));
            }
        }
        Msg::Resize => {}
        Msg::Tick => {
            if ui
                .msg_at
                .is_some_and(|t| t.elapsed() > Duration::from_secs(10))
            {
                ui.update_msg = None;
                ui.msg_at = None;
            }
        }
        Msg::HubOnline(on) => {
            ui.hub_view = if on {
                HubView::Search
            } else {
                HubView::Offline
            };
            ui.hub_focus = if on {
                ui.hub_start_focus()
            } else {
                HubFocus::Input
            };
            if let Some(i) = ui.first_missing_course() {
                ui.course_sel = i;
            }
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
                    if e.code == trurl_core::ErrorCode::Offline {
                        ui.hub_view = HubView::Offline;
                    }
                    ui.notice = Some(e);
                }
            }
        }
        Msg::HubDetails(r) => match r {
            Ok(d) => {
                // The core lists the file to pick on this computer first.
                ui.hub_file_sel = 0;
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
                // "Later" hides the offer until the next check (at start or every 24 hours):
                // every check, automatic or not, shows it again. Never during a download.
                if !matches!(ui.update, UpdateState::Downloading(..)) {
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
            Err(e) if e.code == trurl_core::ErrorCode::UpdateBusy => {
                // An answer started during the download: offer again when it is done.
                ui.notice = Some(e);
                if let UpdateState::Downloading(u, _, _) = &ui.update {
                    ui.update = UpdateState::Available(u.clone());
                }
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
                    Confirm::Update { .. } => start_update(ui, core, tx),
                    Confirm::NewChat => core.new_chat(),
                    Confirm::DeleteChat { id, .. } => {
                        if let Err(e) = core.delete_chat(&id) {
                            ui.notice = Some(e);
                        }
                    }
                    Confirm::DeleteAllChats { .. } => {
                        if let Err(e) = core.delete_all_chats() {
                            ui.notice = Some(e);
                        }
                    }
                    Confirm::OpenChat { id } => open_chat(ui, core, &id),
                    Confirm::Quit => ui.quit = true,
                    Confirm::ClearSystemPrompt => core.set_system_prompt(""),
                    Confirm::DiscardSystemPrompt => ui.prompt = None,
                }
            }
            // "Later": hidden until the next start or the next 24-hour check.
            KeyCode::F(8) if matches!(c, Confirm::Update { .. }) => {
                ui.confirm = None;
                ui.update = UpdateState::Hidden;
            }
            KeyCode::Esc | KeyCode::Char('n') => ui.confirm = None,
            _ => {}
        }
        return;
    }
    if let Some((kind, ed)) = ui.prompt.as_mut() {
        let kind = *kind;
        match k.code {
            KeyCode::Esc => {
                let changed = kind == PromptKind::SystemPrompt
                    && ui
                        .settings
                        .as_ref()
                        .is_some_and(|s| s.system_prompt.trim_end() != ed.text.trim_end());
                if changed {
                    ui.confirm = Some(Confirm::DiscardSystemPrompt);
                } else {
                    ui.prompt = None;
                }
            }
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
                        ui.notice = None;
                        core.import_model(path);
                    } else if path.is_dir() {
                        ui.notice = Some(UserError::import_folder(&p));
                    } else {
                        ui.notice = Some(UserError::import_missing(&p));
                    }
                }
            }
            _ => edit(ed, &k, kind == PromptKind::SystemPrompt),
        }
        return;
    }

    // Global keys. Ctrl+C is "copy" to most learners: never quit on it without asking.
    if ctrl(&k, 'q') || ctrl(&k, 'c') {
        if ui.turns.is_empty() && !ui.generating && ui.input.text.trim().is_empty() {
            ui.quit = true;
        } else {
            ui.confirm = Some(Confirm::Quit);
        }
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
        KeyCode::F(9) => {
            ui.screen = Screen::Chats;
            return;
        }
        KeyCode::F(12) => {
            ui.screen = Screen::Token;
            ui.token.clear();
            return;
        }
        _ => {}
    }
    // Update keys are function keys: a plain letter would steal the first letter of a message.
    if k.code == KeyCode::F(7)
        && let UpdateState::Available(u) = &ui.update
    {
        ui.confirm = Some(Confirm::Update {
            version: u.version.clone(),
            notes: u.notes.clone(),
        });
        return;
    }
    if k.code == KeyCode::F(8) && matches!(ui.update, UpdateState::Available(_)) {
        ui.update = UpdateState::Hidden;
        return;
    }

    // Esc closes an error first, on every screen ("Esc to close" must work there).
    if k.code == KeyCode::Esc && ui.notice.is_some() && ui.screen != Screen::Chat {
        ui.notice = None;
        return;
    }
    match ui.screen {
        Screen::Chat => chat_key(ui, core, k),
        Screen::Settings => settings_key(ui, core, k),
        Screen::Models => models_key(ui, core, tx, k),
        Screen::Hub => hub_key(ui, core, tx, k),
        Screen::Chats => chats_key(ui, core, k),
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

fn expand_home(p: &str) -> String {
    if let Some(rest) = p.strip_prefix("~/")
        && let Some(h) = std::env::var_os("HOME")
    {
        return format!("{}/{rest}", h.to_string_lossy());
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
    if k.code == KeyCode::F(6) {
        copy_last_answer(ui);
        return;
    }
    if k.code == KeyCode::F(5)
        && let Some(t) = ui.turns.last()
        && t.error
            .as_ref()
            .is_some_and(|e| e.code == trurl_core::ErrorCode::ContextFull)
    {
        // The desktop's "Use 512 tokens and ask again": the next size, the same question.
        let now = ui
            .settings
            .as_ref()
            .map_or(t.context, |s| s.context_size.max(t.context));
        if let Some(next) = CONTEXT_CHOICES.iter().copied().find(|c| *c > now) {
            core.set_context(next);
            ui.input = text::Editor::with(&t.user);
            ui.say(format!(
                "Context size {next} tokens. Press Enter to ask again."
            ));
        } else {
            ui.say("This is already the largest context size. Start a new chat (Ctrl+N).");
        }
        return;
    }
    if ctrl(&k, 'n') {
        if ui.generating {
            ui.confirm = Some(Confirm::NewChat);
        } else {
            core.new_chat();
        }
        return;
    }
    if ctrl(&k, 't') {
        ui.toggle_all_thinking();
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
                ui.queued = false;
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
            if matches!(ui.engine, EngineState::Loading { .. }) {
                ui.queued = true;
                return;
            }
            send_input(ui, core);
        }
        // Bounded by the drawing (view.rs clamps it), so PgDn always moves back.
        KeyCode::PageUp => ui.scroll = (ui.scroll + 10).min(ui.max_scroll.get()),
        KeyCode::PageDown => ui.scroll = ui.scroll.saturating_sub(10),
        KeyCode::Up if k.modifiers.contains(KeyModifiers::CONTROL) => ui.scroll += 1,
        KeyCode::Down if k.modifiers.contains(KeyModifiers::CONTROL) => {
            ui.scroll = ui.scroll.saturating_sub(1)
        }
        KeyCode::End if ui.input.text.is_empty() => ui.scroll = 0,
        _ => edit(&mut ui.input, &k, false),
    }
}

/// F6: the last answer, as text, to the clipboard (terminals that allow it)
/// and to a notes file in the learner's home folder, with what it was asked and
/// how. The course asks learners to keep answers in their notes.
fn copy_last_answer(ui: &mut Ui) {
    use std::io::Write;
    if ui.generating {
        ui.say("Wait until the answer is finished (or press Esc to stop it), then press F6.");
        return;
    }
    let Some(t) = ui.turns.iter().rev().find(|t| !t.answer.trim().is_empty()) else {
        ui.say("There is no answer to copy yet.");
        return;
    };
    let answer = text::plain_math(t.answer.trim());
    // OSC 52: the terminal puts it on the clipboard (Windows Terminal, most Linux ones).
    let b64 = {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(answer.as_bytes())
    };
    let _ = write!(std::io::stdout(), "\x1b]52;c;{b64}\x07");
    let _ = std::io::stdout().flush();
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_default();
    let docs = home.join("Documents");
    let notes = if docs.is_dir() { docs } else { home }.join("Trurl notes.txt");
    let sp = if t.system_prompt.trim().is_empty() {
        "no system prompt".to_string()
    } else {
        format!("system prompt: {}", t.system_prompt.trim())
    };
    let stopped = match t.status {
        trurl_core::chat::TurnStatus::Stopped => " (stopped before the end)",
        trurl_core::chat::TurnStatus::Cut => " (cut: the context ran out)",
        _ => "",
    };
    let entry = format!(
        "----- {} · {} · temperature {:.1} · context {} · {sp}\nQuestion: {}\n\n{answer}{stopped}\n\n",
        today(),
        t.model,
        t.temperature,
        t.context,
        t.user.trim()
    );
    let saved = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&notes)
        .and_then(|mut f| f.write_all(entry.as_bytes()));
    // The clipboard part depends on the terminal (Windows Terminal: yes; GNOME
    // Terminal: no), so the message promises only what is certain.
    let short = notes
        .strip_prefix(
            std::env::var_os("HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_default(),
        )
        .map(|p| format!("~/{}", p.display()))
        .unwrap_or_else(|_| notes.display().to_string());
    ui.say(match saved {
        Ok(()) => format!("Saved in {short}. Also copied, if your terminal allows it."),
        Err(_) => {
            "Copied, if your terminal allows it (the notes file could not be written).".into()
        }
    });
}

/// Today's date as YYYY-MM-DD, from the system clock (no time zone library).
fn today() -> String {
    let days = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() / 86_400)
        .unwrap_or(0) as i64;
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

fn send_input(ui: &mut Ui, core: &App) {
    match core.send(&ui.input.text) {
        Ok(_) => {
            ui.input.clear();
            ui.notice = None;
        }
        Err(e) => ui.notice = Some(e),
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
                    // From the value on screen, not from the last event: a fast
                    // run of presses must never lose a step.
                    let t = ((s.temperature + if up { 0.1 } else { -0.1 }) * 10.0).round() / 10.0;
                    let t = t.clamp(0.0, 2.0);
                    if let Some(st) = ui.settings.as_mut() {
                        st.temperature = t;
                    }
                    core.set_temperature(t)
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
            SettingsField::SystemPrompt if !s.system_prompt.is_empty() => {
                ui.confirm = Some(Confirm::ClearSystemPrompt)
            }
            SettingsField::SystemPrompt => {}
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

fn chats_key(ui: &mut Ui, core: &App, k: KeyEvent) {
    let n = ui.chats.len();
    let sel = ui.chats.get(ui.chats_sel).cloned();
    match k.code {
        KeyCode::Esc => ui.screen = Screen::Chat,
        KeyCode::Up => ui.chats_sel = ui.chats_sel.saturating_sub(1),
        KeyCode::Down => ui.chats_sel = (ui.chats_sel + 1).min(n.saturating_sub(1)),
        KeyCode::Enter => {
            let Some(c) = sel else { return };
            if ui.chat_id.as_deref() == Some(c.id.as_str()) {
                ui.screen = Screen::Chat;
            } else if ui.generating {
                ui.confirm = Some(Confirm::OpenChat { id: c.id });
            } else {
                open_chat(ui, core, &c.id);
            }
        }
        KeyCode::Char('d') | KeyCode::Char('D') | KeyCode::Delete => {
            if let Some(c) = sel {
                let stops = ui.generating && ui.chat_id.as_deref() == Some(c.id.as_str());
                ui.confirm = Some(Confirm::DeleteChat {
                    id: c.id,
                    title: c.title,
                    stops,
                });
            }
        }
        KeyCode::Char('a') | KeyCode::Char('A') if n > 0 => {
            ui.confirm = Some(Confirm::DeleteAllChats { count: n });
        }
        _ => {}
    }
}

fn open_chat(ui: &mut Ui, core: &App, id: &str) {
    match core.open_chat(id) {
        Ok(()) => {
            ui.screen = Screen::Chat;
            ui.scroll = 0;
        }
        Err(e) => ui.notice = Some(e),
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
    if ui.hub_focus == HubFocus::Downloads
        && let Some(d) = ui.downloads.get(ui.dl_sel).cloned()
    {
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
    if k.code == KeyCode::Tab {
        let course = ui.hub_view == HubView::Search && !ui.course_models.is_empty();
        ui.hub_focus = match (ui.hub_view.clone(), ui.hub_focus) {
            (HubView::Details, HubFocus::Downloads) => HubFocus::Files,
            (_, HubFocus::Downloads) if course => HubFocus::Course,
            (_, HubFocus::Downloads) => HubFocus::Input,
            (HubView::Search, HubFocus::Course) => HubFocus::Input,
            (HubView::Search, HubFocus::Input) if !ui.hub_results.is_empty() => HubFocus::Results,
            _ if !ui.downloads.is_empty() => HubFocus::Downloads,
            (HubView::Details, _) => HubFocus::Files,
            _ if course => HubFocus::Course,
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
            (HubFocus::Course, KeyCode::Up) => ui.course_sel = ui.course_sel.saturating_sub(1),
            (HubFocus::Course, KeyCode::Down) => {
                if ui.course_sel + 1 >= ui.course_models.len() {
                    ui.hub_focus = HubFocus::Input;
                } else {
                    ui.course_sel += 1;
                }
            }
            (HubFocus::Course, KeyCode::Enter) => {
                let Some(c) = ui.course_models.get(ui.course_sel).cloned() else {
                    return;
                };
                if let Some(m) = ui.course_model_here(&c).cloned() {
                    if m.broken {
                        ui.say(format!(
                            "{} is damaged: delete it in Models (F2), then download it again.",
                            c.name
                        ));
                    } else if ui.running_file() == Some(m.file.as_str()) {
                        ui.say(format!("{} is the model in use.", c.name));
                    } else {
                        core.select_model(&m.file);
                        ui.screen = Screen::Chat;
                    }
                } else if ui.course_clash(&c) {
                    ui.say(format!(
                        "A different file named {} is on this computer: delete it in Models (F2), then download the course one.",
                        c.file
                    ));
                } else if let Some(d) = ui.course_download(&c).cloned() {
                    if d.status == DownloadStatus::Failed {
                        if let Err(e) = core.download_resume(&d.id) {
                            ui.notice = Some(e);
                        }
                    } else {
                        // Select that download, so P acts on it and not on another one.
                        if let Some(i) = ui.downloads.iter().position(|x| x.id == d.id) {
                            ui.dl_sel = i;
                        }
                        ui.hub_focus = HubFocus::Downloads;
                        ui.say(format!(
                            "{} is already downloading: P pauses or resumes it.",
                            c.name
                        ));
                    }
                } else {
                    match core.download_course_model(&c.id) {
                        Ok(_) => ui.say(format!("Downloading {}.", c.name)),
                        Err(e) => ui.notice = Some(e),
                    }
                }
            }
            // Typing on the list starts a search, as it would in the box.
            (HubFocus::Course, KeyCode::Char(_))
                if !k.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                ui.hub_focus = HubFocus::Input;
                edit(&mut ui.hub_query, &k, false);
            }
            (HubFocus::Course, _) => {}
            (HubFocus::Input, KeyCode::Up) if !ui.course_models.is_empty() => {
                ui.hub_focus = HubFocus::Course
            }
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
                let dest = f.path.rsplit('/').next().unwrap_or(&f.path).to_string();
                if ui.models.iter().any(|m| m.file == dest) {
                    ui.say(format!("{dest} is already on this computer (F2)."));
                    return;
                }
                if ui.downloads.iter().any(|x| x.dest_file == dest) {
                    ui.hub_focus = HubFocus::Downloads;
                    ui.say(format!(
                        "{dest} is already downloading: P pauses or resumes it."
                    ));
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
    let n = view::tui_licenses().len();
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
        KeyCode::PageDown => ui.about_scroll = ui.about_scroll.saturating_add(10),
        KeyCode::PageUp => ui.about_scroll = ui.about_scroll.saturating_sub(10),
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
