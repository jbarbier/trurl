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
        system_prompt: String::new(),
        temperature: 0.7,
        context: 2048,
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
    assert!(
        !s.contains("light scattering"),
        "closed once the answer started: {s}"
    );
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
    assert!(
        s.contains("Let me think about this very carefully"),
        "the reasoning stays open: it is all there is to read: {s}"
    );
    assert!(s.contains("The conversation ran out of room."), "{s}");
    assert!(
        s.contains("Start a new chat or increase the context size."),
        "{s}"
    );
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
        notes: "Faster model loading.\nA Copy button under each answer.\nFixes for old CPUs."
            .into(),
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
    for line in [
        "Faster model loading.",
        "A Copy button under each answer.",
        "Fixes for old CPUs.",
    ] {
        assert!(
            s.contains(line),
            "all the notes, not just the first line: {s}"
        );
    }
    assert!(
        s.contains("Enter: update now") && s.contains("F8: later"),
        "{s}"
    );
    assert!(s.contains("Your models and settings are kept."), "{s}");
}

#[test]
fn offline_hugging_face_says_what_to_do_and_nothing_else_breaks() {
    let mut ui = Ui::new();
    ready(&mut ui);
    ui.screen = Screen::Hub;
    ui.hub_view = HubView::Offline;
    let s = flat(&screen(&ui, 100, 30));
    assert!(
        s.contains("Connect to the internet to browse Hugging Face"),
        "{s}"
    );
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
        course: false,
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
    for want in [
        "Gemma 3 1B",
        "Fits easily",
        "Tight fit",
        "Too big",
        "806 MB",
        // Same file as the pin, hash checked (USB or download alike).
        "checked by Trurl",
    ] {
        assert!(s.contains(want), "missing {want:?}: {s}");
    }
}

#[test]
fn about_names_the_maker_and_the_version() {
    let mut ui = Ui::new();
    ui.version = trurl_core::VERSION.into(); // from the core's snapshot
    ui.screen = Screen::About;
    let s = flat(&screen(&ui, 100, 30));
    assert!(s.contains("By Julien Barbier, for Cyberiad Labs"), "{s}");
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

#[test]
fn each_answer_shows_the_settings_it_was_written_with() {
    let mut ui = Ui::new();
    ready(&mut ui);
    let mut t = turn(1, TurnStatus::Done);
    t.answer = "Bonjour !".into();
    t.temperature = 0.0;
    t.system_prompt = "Answer only in French.".into();
    ui.turns = vec![t];
    let s = flat(&screen(&ui, 100, 30));
    assert!(s.contains("temperature 0.0"), "{s}");
    assert!(s.contains("system prompt: “Answer only in French.”"), "{s}");
}

#[test]
fn math_is_shown_as_text() {
    let mut ui = Ui::new();
    ready(&mut ui);
    let mut t = turn(1, TurnStatus::Done);
    t.answer = "So $x^2 = 9$ and the answer is $\\boxed{3}$.".into();
    ui.turns = vec![t];
    let s = flat(&screen(&ui, 100, 30));
    assert!(s.contains("So x² = 9 and the answer is 3."), "{s}");
}

#[test]
fn a_model_that_is_not_thinking_is_not_called_thinking() {
    let mut ui = Ui::new();
    ready(&mut ui);
    ui.generating = true;
    ui.turns = vec![turn(1, TurnStatus::Streaming)];
    assert_eq!(ui.engine_label().1, "starting");
    ui.turns[0].reasoning = "Hmm".into();
    assert_eq!(ui.engine_label().1, "thinking");
    ui.turns[0].answer = "42".into();
    assert_eq!(ui.engine_label().1, "answering");
}

#[test]
fn ctrl_n_during_an_answer_asks_first() {
    let mut ui = Ui::new();
    ready(&mut ui);
    ui.confirm = Some(Confirm::NewChat);
    let s = flat(&screen(&ui, 100, 30));
    assert!(s.contains("The answer so far will be lost."), "{s}");
}

#[test]
fn a_question_about_the_editor_is_drawn_on_top_of_it() {
    let mut ui = Ui::new();
    ready(&mut ui);
    ui.screen = Screen::Settings;
    ui.prompt = Some((
        PromptKind::SystemPrompt,
        crate::text::Editor::with("You are teaching me."),
    ));
    ui.confirm = Some(Confirm::DiscardSystemPrompt);
    for (w, h) in [(80, 24), (120, 34)] {
        let s = flat(&screen(&ui, w, h));
        assert!(s.contains("Leave without saving?"), "{w}x{h}: {s}");
        assert!(
            s.contains("Enter yes") || s.contains("Enter: leave"),
            "{w}x{h}: {s}"
        );
    }
}

#[test]
fn search_results_scroll_to_the_selection() {
    let mut ui = Ui::new();
    ready(&mut ui);
    ui.screen = Screen::Hub;
    ui.hub_view = HubView::Search;
    ui.hub_focus = HubFocus::Results;
    ui.hub_results = (0..30)
        .map(|i| trurl_core::hub::RepoHit {
            id: format!("someone/model-{i:02}-GGUF"),
            downloads: 324_976,
            likes: 1,
            gated: false,
            updated: None,
            chat: Some(true),
        })
        .collect();
    ui.hub_sel = 25;
    let s = flat(&screen(&ui, 80, 24));
    assert!(s.contains("› someone/model-25-GGUF"), "{s}");
    assert!(s.contains("more above"), "{s}");
    assert!(s.contains("324,976"), "thousands separators: {s}");
}

#[test]
fn hugging_face_warnings_wrap_at_80_columns() {
    let mut ui = Ui::new();
    ready(&mut ui);
    ui.screen = Screen::Hub;
    ui.hub_view = HubView::Details;
    ui.hub_details = Some(trurl_core::hub::RepoFiles {
        id: "Qwen/Qwen3-Embedding-0.6B-GGUF".into(),
        author: Some("Qwen".into()),
        revision: "abc".into(),
        gated: false,
        license: None,
        downloads: 1000,
        likes: 5,
        updated: None,
        base_model: None,
        architecture: None,
        context_length: None,
        parameters: None,
        tags: vec![],
        chat: Some(false),
        readme: None,
        files: vec![],
    });
    let s = flat(&screen(&ui, 80, 24));
    assert!(
        s.contains("Look for a model whose name ends in Instruct or Chat."),
        "{s}"
    );
}

#[test]
fn the_files_legend_says_why_q4_0_is_recommended_only_where_it_is_faster() {
    use trurl_core::hardware::CpuVariant;
    let file = |q: &str, recommended: bool| trurl_core::hub::RepoFile {
        path: format!("m-{q}.gguf"),
        size: 500_000_000,
        sha256: None,
        quant: Some(q.into()),
        fit: RamFit::Green,
        split: false,
        recommended,
    };
    let show = |variant: CpuVariant, files: Vec<trurl_core::hub::RepoFile>| {
        let mut ui = Ui::new();
        ready(&mut ui);
        ui.hardware = Some(trurl_core::events::HardwareSummary {
            total_ram: 4 << 30,
            physical_cores: 2,
            threads: 2,
            cpu_name: "test".into(),
            variant,
        });
        ui.screen = Screen::Hub;
        ui.hub_view = HubView::Details;
        ui.hub_details = Some(trurl_core::hub::RepoFiles {
            id: "someone/m-GGUF".into(),
            author: None,
            revision: "abc".into(),
            gated: false,
            license: None,
            downloads: 1,
            likes: 0,
            updated: None,
            base_model: None,
            architecture: None,
            context_length: None,
            parameters: None,
            tags: vec![],
            chat: Some(true),
            readme: None,
            files,
        });
        flat(&screen(&ui, 120, 40))
    };
    let faster = "On this computer's processor, Q4_0 answers fastest";
    // A Core 2: the core recommended Q4_0 because it is faster here.
    let s = show(
        CpuVariant::Ssse3,
        vec![file("Q4_0", true), file("Q4_K_M", false)],
    );
    assert!(s.contains(faster), "{s}");
    // A recent processor, on a repo that only has Q4_0: recommended, but not
    // for speed, so the screen must not claim it.
    let s = show(
        CpuVariant::Avx2,
        vec![file("Q4_0", true), file("Q8_0", false)],
    );
    assert!(!s.contains(faster), "{s}");
    assert!(
        s.contains("The recommended one, Q4_0, is listed first."),
        "{s}"
    );
    let s = show(
        CpuVariant::Avx2,
        vec![file("Q4_K_M", true), file("Q4_0", false)],
    );
    assert!(
        s.contains("The recommended one, Q4_K_M, is listed first."),
        "{s}"
    );
}

// ----- Course models ------------------------------------------------------------

fn course_ui() -> Ui {
    let mut ui = Ui::new();
    ready(&mut ui);
    ui.screen = Screen::Hub;
    ui.hub_view = HubView::Search;
    ui.course_models = trurl_core::pins::course_models()
        .map(|m| trurl_core::events::CourseModel {
            id: m.id.clone(),
            name: m.name.clone(),
            note: m.course.clone().unwrap(),
            file: m.file.clone(),
            size: m.size,
            fit: RamFit::Green,
            download_id: trurl_core::downloads::download_id(&m.repo, &m.file, &m.revision),
        })
        .collect();
    // Gemma 3 1B is here and in use; the other two are not.
    let g = &ui.course_models[0];
    ui.models = vec![ModelInfo {
        file: g.file.clone(),
        name: g.name.clone(),
        quant: Some("Q4_K_M".into()),
        size: g.size,
        fit: RamFit::Green,
        bundled: true,
        course: true,
        verified: Some(true),
        broken: false,
        has_chat_template: true,
        reasoning_hint: false,
        train_context: Some(32768),
        gguf: None,
    }];
    ui.engine = EngineState::Ready {
        file: g.file.clone(),
        name: g.name.clone(),
        context: 2048,
    };
    ui.hub_focus = ui.hub_start_focus();
    ui
}

#[test]
fn the_course_models_come_first_on_the_hugging_face_screen() {
    let mut ui = course_ui();
    assert_eq!(ui.hub_focus, HubFocus::Course, "a course model is missing");
    ui.course_sel = 1;
    for (w, h) in [(80, 24), (120, 30)] {
        let s = flat(&screen(&ui, w, h));
        for want in [
            "For the course",
            "Gemma 3 1B Instruct",
            "◆ in use",
            "› Gemma 3 1B PT",
            "Enter downloads it",
            "Qwen3 0.6B",
            "not downloaded yet",
            "Gemma 3 1B PT: Optional. The same model before it was taught to answer",
            "Search Hugging Face",
            "Enter download",
        ] {
            assert!(s.contains(want), "{w}x{h} missing {want:?}: {s}");
        }
        let a = s.find("For the course").unwrap();
        let b = s.find("Search Hugging Face").unwrap();
        assert!(a < b, "the course list is above the search");
    }
}

#[test]
fn a_course_model_downloading_shows_its_progress_and_all_here_opens_on_search() {
    let mut ui = course_ui();
    let q = ui.course_models[2].clone();
    let mut d: trurl_core::downloads::DownloadInfo = serde_json::from_value(serde_json::json!({
        "id": q.download_id, "repo": "r", "file": q.file, "revision": "x", "dest_file": q.file,
        "size": 1000, "sha256": null, "done": 420, "status": "running"
    }))
    .unwrap();
    d.rate = 0;
    ui.downloads = vec![d];
    let s = flat(&screen(&ui, 80, 24));
    assert!(s.contains("downloading 42%"), "{s}");

    // Every course model here: the screen opens on the search box, as before.
    for c in ui.course_models.clone() {
        let mut m = ui.models[0].clone();
        m.file = c.file;
        ui.models.push(m);
    }
    assert_eq!(ui.hub_start_focus(), HubFocus::Input);
}

#[test]
fn offline_the_course_list_says_how_to_get_the_missing_ones() {
    let mut ui = course_ui();
    ui.hub_view = HubView::Offline;
    ui.hub_focus = HubFocus::Input;
    let s = flat(&screen(&ui, 80, 24));
    assert!(s.contains("For the course"), "{s}");
    assert!(
        s.contains("Connect to the internet to download them, or press I in Models (F2)"),
        "{s}"
    );
    assert!(
        s.contains("Connect to the internet to browse Hugging Face"),
        "{s}"
    );
    assert!(
        !s.contains("Enter downloads it"),
        "nothing to download offline: {s}"
    );
}

#[test]
fn search_results_still_scroll_to_the_selection_under_the_course_list() {
    let mut ui = course_ui();
    ui.hub_focus = HubFocus::Results;
    ui.hub_results = (0..30)
        .map(|i| trurl_core::hub::RepoHit {
            id: format!("someone/model-{i:02}-GGUF"),
            downloads: 1,
            likes: 1,
            gated: false,
            updated: None,
            chat: Some(true),
        })
        .collect();
    ui.hub_sel = 25;
    let s = flat(&screen(&ui, 80, 24));
    assert!(s.contains("› someone/model-25-GGUF"), "{s}");
}

#[test]
fn the_models_screen_marks_course_models() {
    let mut ui = course_ui();
    ui.screen = Screen::Models;
    let s = flat(&screen(&ui, 100, 30));
    assert!(s.contains("course model"), "{s}");
    assert!(
        !s.contains("checked by Trurl"),
        "a course model says course model: {s}"
    );
}

/// Keys on the course list, against a real core: Enter downloads exactly the
/// pinned file; Down from the last one and typing both go to the search box.
#[tokio::test]
async fn enter_on_a_course_model_downloads_the_pinned_file() {
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let home = tempfile::tempdir().unwrap();
    let paths = trurl_core::paths::Paths::at(home.path());
    std::fs::create_dir_all(paths.models()).unwrap();
    let core = trurl_core::app::App::start(
        trurl_core::app::AppConfig {
            paths,
            llama_root: None,
            // Nothing listens here: the download just waits to retry.
            hf_base: "http://127.0.0.1:9".into(),
            user_agent: "trurl-test".into(),
        },
        std::sync::Arc::new(|_| {}),
    );
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut ui = course_ui();
    ui.course_sel = 0;
    let key = |c| KeyEvent::new(c, KeyModifiers::NONE);

    // Gemma 3 1B is here and in use: nothing starts.
    crate::hub_key(&mut ui, &core, &tx, key(KeyCode::Enter));
    assert!(core.downloads_list().is_empty());
    assert!(
        ui.update_msg
            .as_deref()
            .unwrap_or("")
            .contains("is the model in use"),
        "{:?}",
        ui.update_msg
    );

    crate::hub_key(&mut ui, &core, &tx, key(KeyCode::Down));
    crate::hub_key(&mut ui, &core, &tx, key(KeyCode::Down));
    crate::hub_key(&mut ui, &core, &tx, key(KeyCode::Enter));
    let pin = trurl_core::pins::course_models().nth(2).unwrap();
    let d = core.downloads_list();
    assert_eq!(d.len(), 1, "{d:?}");
    assert_eq!(
        (
            d[0].repo.as_str(),
            d[0].revision.as_str(),
            d[0].dest_file.as_str()
        ),
        (pin.repo.as_str(), pin.revision.as_str(), pin.file.as_str())
    );
    assert_eq!(d[0].sha256.as_deref(), Some(pin.sha256.as_str()));

    crate::hub_key(&mut ui, &core, &tx, key(KeyCode::Down));
    assert_eq!(ui.hub_focus, HubFocus::Input);
    crate::hub_key(&mut ui, &core, &tx, key(KeyCode::Up));
    assert_eq!(ui.hub_focus, HubFocus::Course);
    crate::hub_key(&mut ui, &core, &tx, key(KeyCode::Char('q')));
    assert_eq!(ui.hub_focus, HubFocus::Input);
    assert_eq!(ui.hub_query.text, "q");
    core.shutdown().await;
}

#[test]
fn the_course_list_starts_on_the_first_missing_model() {
    let ui = course_ui();
    assert_eq!(
        ui.first_missing_course(),
        Some(1),
        "Gemma 3 1B is here, the PT is not"
    );
}

#[test]
fn a_failed_download_a_damaged_file_and_a_look_alike_each_say_what_to_do() {
    let mut ui = course_ui();
    // The PT download failed.
    let pt = ui.course_models[1].clone();
    let d: trurl_core::downloads::DownloadInfo = serde_json::from_value(serde_json::json!({
        "id": pt.download_id, "repo": "r", "file": pt.file, "revision": "x", "dest_file": pt.file,
        "size": 1000, "sha256": null, "done": 420, "status": "failed",
        "error": { "code": "download_failed", "title": "The download stopped.", "hint": "Try again." }
    }))
    .unwrap();
    ui.downloads = vec![d];
    // Qwen3: a different file with the same name.
    let q = ui.course_models[2].clone();
    let mut other = ui.models[0].clone();
    other.file = q.file.clone();
    other.course = false;
    other.bundled = false;
    ui.models.push(other);
    ui.course_sel = 1;
    let s = flat(&screen(&ui, 120, 30));
    assert!(
        s.contains("failed: The download stopped. Enter tries again."),
        "{s}"
    );
    assert!(s.contains("another file has this name"), "{s}");
    assert!(ui.course_clash(&q));
    // Gemma 3 1B damaged.
    ui.models[0].broken = true;
    let s = flat(&screen(&ui, 120, 30));
    assert!(s.contains("damaged file"), "{s}");
}

#[test]
fn a_paused_course_download_says_paused_and_the_key_bar_matches_each_row() {
    let mut ui = course_ui();
    let pt = ui.course_models[1].clone();
    let d: trurl_core::downloads::DownloadInfo = serde_json::from_value(serde_json::json!({
        "id": pt.download_id, "repo": "r", "file": pt.file, "revision": "x", "dest_file": pt.file,
        "size": 1000, "sha256": null, "done": 420, "status": "paused"
    }))
    .unwrap();
    ui.downloads = vec![d];
    let s = flat(&screen(&ui, 80, 24));
    assert!(s.contains("paused at 42%"), "{s}");
    assert!(!s.contains("downloading 42%"), "{s}");
    // Key bar: in use -> no Enter; paused -> no Enter; missing -> download.
    for (sel, want) in [(0, None), (1, None), (2, Some("download"))] {
        ui.course_sel = sel;
        assert_eq!(ui.course_enter(), want, "row {sel}");
    }
    ui.course_sel = 2;
    let s = flat(&screen(&ui, 80, 24));
    assert!(s.contains("Enter download"), "{s}");
}

#[test]
fn the_whole_note_fits_at_80_by_24_even_with_a_failed_row() {
    let mut ui = course_ui();
    let pt = ui.course_models[1].clone();
    let d: trurl_core::downloads::DownloadInfo = serde_json::from_value(serde_json::json!({
        "id": pt.download_id, "repo": "r", "file": pt.file, "revision": "x", "dest_file": pt.file,
        "size": 1000, "sha256": null, "done": 420, "status": "failed",
        "error": { "code": "download_failed", "title": "The download stopped.", "hint": "Try again." }
    }))
    .unwrap();
    ui.downloads = vec![d];
    ui.course_sel = 1;
    let s = flat(&screen(&ui, 80, 24));
    assert!(
        s.contains("so you stop it."),
        "the end of the note is cut: {s}"
    );
    assert_eq!(ui.course_enter(), Some("try again"));
}

#[tokio::test]
async fn tab_goes_course_list_search_box_downloads_and_back() {
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let home = tempfile::tempdir().unwrap();
    let paths = trurl_core::paths::Paths::at(home.path());
    std::fs::create_dir_all(paths.models()).unwrap();
    let core = trurl_core::app::App::start(
        trurl_core::app::AppConfig {
            paths,
            llama_root: None,
            hf_base: "http://127.0.0.1:9".into(),
            user_agent: "trurl-test".into(),
        },
        std::sync::Arc::new(|_| {}),
    );
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut ui = course_ui();
    let tab = KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE);
    let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(ui.hub_focus, HubFocus::Course);
    crate::hub_key(&mut ui, &core, &tx, tab);
    assert_eq!(ui.hub_focus, HubFocus::Input);
    crate::hub_key(&mut ui, &core, &tx, tab);
    assert_eq!(
        ui.hub_focus,
        HubFocus::Course,
        "no results, no downloads: back to the list"
    );

    // Two downloads; Enter on the PT again selects ITS download, not the first.
    ui.course_sel = 1;
    crate::hub_key(&mut ui, &core, &tx, enter);
    ui.course_sel = 2;
    crate::hub_key(&mut ui, &core, &tx, enter);
    ui.downloads = core.downloads_list();
    assert_eq!(ui.downloads.len(), 2);
    ui.hub_focus = HubFocus::Course;
    ui.dl_sel = 0;
    ui.course_sel = 1;
    crate::hub_key(&mut ui, &core, &tx, enter);
    assert_eq!(ui.hub_focus, HubFocus::Downloads);
    assert_eq!(ui.downloads[ui.dl_sel].dest_file, ui.course_models[1].file);
    crate::hub_key(&mut ui, &core, &tx, tab);
    assert_eq!(
        ui.hub_focus,
        HubFocus::Course,
        "from Downloads, Tab goes back to the list"
    );
    core.shutdown().await;
}

#[test]
fn a_search_download_of_another_commit_is_not_the_course_download() {
    let mut ui = course_ui();
    let q = ui.course_models[2].clone();
    // Same file name, found by searching, but not the pinned commit.
    let d: trurl_core::downloads::DownloadInfo = serde_json::from_value(serde_json::json!({
        "id": "not-the-pin", "repo": "bartowski/Qwen_Qwen3-0.6B-GGUF", "file": q.file,
        "revision": "0000000000000000000000000000000000000000", "dest_file": q.file,
        "size": 1000, "sha256": null, "done": 420, "status": "failed"
    }))
    .unwrap();
    ui.downloads = vec![d];
    assert!(ui.course_download(&q).is_none());
    ui.course_sel = 2;
    assert_eq!(
        ui.course_enter(),
        Some("download"),
        "Enter fetches the pinned file, never resumes the other one"
    );
}
