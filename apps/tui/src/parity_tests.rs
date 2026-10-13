//! The terminal app must show a learner what the desktop app shows. These tests
//! draw real screens into a test terminal and read them back as text, at the
//! smallest size worth supporting (80x24) and at a typical laptop terminal.

use crate::state::*;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use trurl_core::chat::{Turn, TurnStatus};
use trurl_core::errors::UserError;
use trurl_core::events::EngineState;
use trurl_core::hardware::RamFit;
use trurl_core::models::ModelInfo;
use trurl_core::update::UpdateInfo;

fn screen(ui: &Ui, w: u16, h: u16) -> String {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| crate::view::draw(f, ui)).unwrap();
    let buf = t.backend().buffer().clone();
    let mut out = String::new();
    for y in 0..h {
        for x in 0..w {
            out.push_str(buf[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

/// Text on screen with the line breaks and borders taken out, so a sentence
/// wrapped over two lines still matches.
fn flat(s: &str) -> String {
    s.lines()
        .map(|l| l.trim_matches(|c: char| c == '│' || c.is_whitespace()))
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn ready(ui: &mut Ui) {
    ui.engine = EngineState::Ready {
        file: "gemma-3-1b-it-Q4_K_M.gguf".into(),
        name: "Gemma 3 1B".into(),
        context: 2048,
    };
}

fn turn(id: u64, status: TurnStatus) -> Turn {
    Turn {
        id,
        user: "Why is the sky blue?".into(),
        reasoning: String::new(),
        answer: String::new(),
        model: "Qwen3 0.6B".into(),
        status,
        thought_ms: None,
        error: None,
    }
}

#[test]
fn thinking_streams_open_then_says_how_long_it_took() {
    let mut ui = Ui::new();
    ready(&mut ui);
    let mut t = turn(1, TurnStatus::Streaming);
    t.reasoning = "The user asks about light scattering.".into();
    ui.turns = vec![t.clone()];
    let s = flat(&screen(&ui, 100, 30));
    assert!(s.contains("Thinking…"), "{s}");
    assert!(s.contains("light scattering"), "open while thinking: {s}");

    t.answer = "Because of Rayleigh scattering.".into();
    t.thought_ms = Some(12_000);
    t.status = TurnStatus::Done;
    ui.turns = vec![t];
    let s = flat(&screen(&ui, 100, 30));
    assert!(s.contains("Thought for 12s"), "{s}");
    assert!(!s.contains("light scattering"), "closed once the answer started: {s}");
    assert!(s.contains("Rayleigh scattering"));
}

#[test]
fn ctrl_t_opens_every_thinking_block_then_closes_them_all() {
    let mut ui = Ui::new();
    let mut a = turn(1, TurnStatus::Done);
    a.reasoning = "first thoughts".into();
    a.answer = "one".into();
    let mut b = turn(2, TurnStatus::Done);
    b.reasoning = "second thoughts".into();
    b.answer = "two".into();
    ui.turns = vec![a, b];
    ui.toggle_all_thinking();
    assert!(ui.turns.iter().all(|t| ui.thinking_open(t)));
    ui.toggle_all_thinking();
    assert!(ui.turns.iter().all(|t| !ui.thinking_open(t)));
}

#[test]
fn running_out_of_room_says_so_in_plain_words() {
    let mut ui = Ui::new();
    ready(&mut ui);
    let mut t = turn(1, TurnStatus::Cut);
    t.reasoning = "Let me think about this very carefully".into();
    t.error = Some(UserError::context_full());
    ui.turns = vec![t];
    let s = flat(&screen(&ui, 100, 30));
    assert!(s.contains("Ran out of room while thinking"), "{s}");
    assert!(s.contains("The conversation ran out of room."), "{s}");
    assert!(s.contains("Start a new chat or increase the context size."), "{s}");
}

#[test]
fn a_message_typed_while_the_model_loads_waits_for_it() {
    let mut ui = Ui::new();
    ui.engine = EngineState::Loading {
        file: "gemma-3-1b-it-Q4_K_M.gguf".into(),
        name: "Gemma 3 1B".into(),
    };
    let s = flat(&screen(&ui, 100, 30));
    assert!(s.contains("Enter sends it when the model is ready"), "{s}");
    ui.queued = true;
    let s = flat(&screen(&ui, 100, 30));
    assert!(s.contains("sent as soon as the model is ready"), "{s}");
}

#[test]
fn the_update_offer_shows_the_version_then_the_full_notes() {
    let mut ui = Ui::new();
    ready(&mut ui);
    let info = UpdateInfo {
        version: "0.2.0".into(),
        notes: "Faster model loading.\nA Copy button under each answer.\nFixes for old CPUs.".into(),
        date: None,
        url: String::new(),
        signature: String::new(),
    };
    ui.update = UpdateState::Available(info.clone());
    let s = flat(&screen(&ui, 120, 30));
    assert!(s.contains("Update available: version 0.2.0"), "{s}");
    assert!(s.contains("F8 for later"), "{s}");
    ui.confirm = Some(Confirm::Update {
        version: info.version,
        notes: info.notes,
    });
    let s = flat(&screen(&ui, 120, 30));
    for line in ["Faster model loading.", "A Copy button under each answer.", "Fixes for old CPUs."] {
        assert!(s.contains(line), "all the notes, not just the first line: {s}");
    }
    assert!(s.contains("Enter: update now") && s.contains("F8: later"), "{s}");
    assert!(s.contains("Your models and settings are kept."), "{s}");
}

#[test]
fn offline_hugging_face_says_what_to_do_and_nothing_else_breaks() {
    let mut ui = Ui::new();
    ready(&mut ui);
    ui.screen = Screen::Hub;
    ui.hub_view = HubView::Offline;
    let s = flat(&screen(&ui, 100, 30));
    assert!(s.contains("Connect to the internet to browse Hugging Face"), "{s}");
}

#[test]
fn models_show_size_and_how_well_they_fit_in_memory() {
    let mut ui = Ui::new();
    ready(&mut ui);
    ui.screen = Screen::Models;
    let model = |file: &str, name: &str, size: u64, fit: RamFit| ModelInfo {
        file: file.into(),
        name: name.into(),
        quant: Some("Q4_K_M".into()),
        size,
        fit,
        bundled: true,
        verified: Some(true),
        broken: false,
        has_chat_template: true,
        reasoning_hint: false,
        train_context: Some(32768),
        gguf: None,
    };
    ui.models = vec![
        model("a.gguf", "Gemma 3 1B", 806_000_000, RamFit::Green),
        model("b.gguf", "Gemma 3 4B", 2_490_000_000, RamFit::Orange),
        model("c.gguf", "Big model", 7_000_000_000, RamFit::Red),
    ];
    let s = flat(&screen(&ui, 120, 30));
    for want in ["Gemma 3 1B", "Fits easily", "Tight fit", "Too big", "806 MB"] {
        assert!(s.contains(want), "missing {want:?}: {s}");
    }
}

#[test]
fn about_names_the_maker_and_the_version() {
    let mut ui = Ui::new();
    ui.version = trurl_core::VERSION.into(); // from the core's snapshot
    ui.screen = Screen::About;
    let s = flat(&screen(&ui, 100, 30));
    assert!(s.contains("by Cyberiad Labs"), "{s}");
    assert!(s.contains(trurl_core::VERSION), "{s}");
}

#[test]
fn every_screen_draws_at_80_by_24_without_panicking() {
    for scr in [
        Screen::Chat,
        Screen::Settings,
        Screen::Models,
        Screen::Hub,
        Screen::About,
        Screen::Token,
    ] {
        let mut ui = Ui::new();
        ready(&mut ui);
        ui.screen = scr;
        let _ = screen(&ui, 80, 24);
        let _ = screen(&ui, 40, 12);
    }
}
