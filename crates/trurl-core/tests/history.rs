//! Chat history through the app: saved chats are listed, an opened chat comes
//! back with the settings of its last answer, and delete really deletes.
//! (Saving after a real answer is in the live evals: `live.rs`.)

use std::sync::{Arc, Mutex};
use trurl_core::app::{App, AppConfig};
use trurl_core::chat::{Turn, TurnStatus};
use trurl_core::events::Event;
use trurl_core::history::{Chat, History};
use trurl_core::paths::Paths;

fn turn(user: &str, answer: &str, sys: &str, temp: f32, ctx: u32) -> Turn {
    Turn {
        id: 1,
        user: user.into(),
        reasoning: String::new(),
        answer: answer.into(),
        model: "Gemma 3 1B Instruct".into(),
        status: TurnStatus::Done,
        thought_ms: None,
        error: None,
        system_prompt: sys.into(),
        temperature: temp,
        context: ctx,
    }
}

struct T {
    app: App,
    paths: Paths,
    events: Arc<Mutex<Vec<Event>>>,
    _home: tempfile::TempDir,
}

fn start(chats: &[Chat]) -> T {
    let home = tempfile::tempdir().unwrap();
    let paths = Paths::at(home.path());
    paths.ensure().unwrap();
    let h = History::new(paths.chats());
    for c in chats {
        h.save(c).unwrap();
    }
    let events: Arc<Mutex<Vec<Event>>> = Arc::default();
    let ev = events.clone();
    let app = App::start(
        AppConfig {
            paths: paths.clone(),
            llama_root: None,
            hf_base: "http://127.0.0.1:9".into(),
            user_agent: "trurl-test".into(),
        },
        Arc::new(move |e| ev.lock().unwrap().push(e)),
    );
    T {
        app,
        paths,
        events,
        _home: home,
    }
}

fn two_chats() -> Vec<Chat> {
    vec![
        Chat {
            id: "1000-0001".into(),
            created: 100,
            updated: 100,
            model_file: None,
            turns: vec![
                turn("Where is Accra?", "In Ghana.", "", 0.7, 2048),
                turn(
                    "And Kumasi?",
                    "Also in Ghana.",
                    "Answer like a pirate.",
                    0.2,
                    4096,
                ),
            ],
        },
        Chat {
            id: "2000-0001".into(),
            created: 200,
            updated: 200,
            model_file: None,
            turns: vec![turn("What is 17 + 25?", "42", "", 0.7, 2048)],
        },
    ]
}

#[tokio::test]
async fn saved_chats_are_listed_newest_first_in_the_snapshot() {
    let t = start(&two_chats());
    let s = t.app.snapshot();
    let titles: Vec<&str> = s.chats.iter().map(|c| c.title.as_str()).collect();
    assert_eq!(titles, ["What is 17 + 25?", "Where is Accra?"]);
    assert_eq!(s.chats[1].turns, 2);
    assert_eq!(s.chat_id, None, "Trurl starts on a new chat");
    t.app.shutdown().await;
}

#[tokio::test]
async fn opening_a_chat_brings_back_its_turns_and_its_last_settings() {
    let t = start(&two_chats());
    t.app.open_chat("1000-0001").unwrap();
    let s = t.app.snapshot();
    assert_eq!(s.chat_id.as_deref(), Some("1000-0001"));
    let users: Vec<&str> = s.turns.iter().map(|x| x.user.as_str()).collect();
    assert_eq!(users, ["Where is Accra?", "And Kumasi?"]);
    // Ids are fresh and distinct within this session.
    assert_ne!(s.turns[0].id, s.turns[1].id);
    // The settings of the last answer: the next answer is written the same way.
    assert_eq!(s.settings.system_prompt, "Answer like a pirate.");
    assert!((s.settings.temperature - 0.2).abs() < 1e-6);
    assert_eq!(s.settings.context_size, 4096);
    // The front ends were told: a snapshot with the chat on screen.
    assert!(t.events.lock().unwrap().iter().any(|e| matches!(e,
        Event::Snapshot { snapshot } if snapshot.chat_id.as_deref() == Some("1000-0001"))));
    t.app.shutdown().await;
}

#[tokio::test]
async fn a_chat_that_cannot_be_read_says_so_and_changes_nothing() {
    let t = start(&two_chats());
    std::fs::write(t.paths.chats().join("3000-0001.json"), b"{ damaged").unwrap();
    let e = t.app.open_chat("3000-0001").unwrap_err();
    assert_eq!(e.title, "This chat could not be opened.");
    assert!(t.app.open_chat("../settings").is_err());
    assert_eq!(t.app.snapshot().chat_id, None);
    assert!(t.app.turns().is_empty());
    t.app.shutdown().await;
}

#[tokio::test]
async fn deleting_the_open_chat_clears_the_screen_and_the_file() {
    let t = start(&two_chats());
    t.app.open_chat("1000-0001").unwrap();
    t.app.delete_chat("1000-0001").unwrap();
    assert!(t.app.turns().is_empty());
    assert_eq!(t.app.snapshot().chat_id, None);
    assert!(!t.paths.chats().join("1000-0001.json").exists());
    assert_eq!(t.app.chats().len(), 1);
    // Deleting another chat leaves the screen alone.
    t.app.open_chat("2000-0001").unwrap();
    std::fs::write(t.paths.chats().join("notes.txt"), b"keep").unwrap();
    t.app.delete_all_chats().unwrap();
    assert!(t.app.chats().is_empty());
    assert!(t.app.turns().is_empty());
    assert!(
        t.paths.chats().join("notes.txt").exists(),
        "only chat files go"
    );
    // A restart finds nothing.
    t.app.shutdown().await;
    let again = start(&[]);
    assert!(again.app.chats().is_empty());
    again.app.shutdown().await;
}

#[tokio::test]
async fn new_chat_with_nothing_asked_saves_nothing() {
    let t = start(&[]);
    t.app.new_chat();
    assert!(t.app.chats().is_empty());
    assert!(
        std::fs::read_dir(t.paths.chats())
            .map(|d| d.count())
            .unwrap_or(0)
            == 0,
        "no empty chat files"
    );
    t.app.shutdown().await;
}

// ----- With a running (fake) model server -----------------------------------------
// tests/fixtures/fake-llama-server.py stands in for llama-server: same command
// line, streamed answers, slow ones on demand. Unix only (it is a script).

#[cfg(unix)]
mod with_server {
    use super::*;
    use std::time::{Duration, Instant};
    use trurl_core::events::EngineState;

    const A: &str = "a-model.gguf";
    const B: &str = "b-model.gguf";

    fn fake_llama_root(dir: &std::path::Path) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/fake-llama-server.py");
        let root = dir.join("llama");
        for v in ["avx2", "avx", "ssse3", "compat"] {
            std::fs::create_dir_all(root.join(v)).unwrap();
            let exe = root.join(v).join("llama-server");
            std::fs::copy(&src, &exe).unwrap();
            std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        root
    }

    struct S {
        app: App,
        paths: Paths,
        events: Arc<Mutex<Vec<Event>>>,
        _home: tempfile::TempDir,
    }

    fn start(chats: &[Chat], models: &[&str]) -> S {
        let home = tempfile::tempdir().unwrap();
        let paths = Paths::at(home.path().join("data"));
        paths.ensure().unwrap();
        for m in models {
            std::fs::write(
                paths.models().join(m),
                trurl_core::gguf::testfile::chat_model(m),
            )
            .unwrap();
        }
        let h = History::new(paths.chats());
        for c in chats {
            h.save(c).unwrap();
        }
        let events: Arc<Mutex<Vec<Event>>> = Arc::default();
        let ev = events.clone();
        let app = App::start(
            AppConfig {
                paths: paths.clone(),
                llama_root: Some(fake_llama_root(home.path())),
                hf_base: "http://127.0.0.1:9".into(),
                user_agent: "trurl-test".into(),
            },
            Arc::new(move |e| ev.lock().unwrap().push(e)),
        );
        S {
            app,
            paths,
            events,
            _home: home,
        }
    }

    async fn until<T>(what: &str, mut f: impl FnMut() -> Option<T>) -> T {
        let t0 = Instant::now();
        loop {
            if let Some(v) = f() {
                return v;
            }
            assert!(t0.elapsed() < Duration::from_secs(30), "timed out: {what}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    async fn ready(app: &App, file: &str) {
        until(&format!("{file} ready"), || match app.snapshot().engine {
            EngineState::Ready { file: f, .. } if f == file => Some(()),
            EngineState::Failed { error, .. } => panic!("load failed: {error:?}"),
            _ => None,
        })
        .await
    }

    async fn some_words(app: &App) {
        until("some words", || {
            app.turns()
                .last()
                .filter(|t| t.answer.len() > 20)
                .map(|_| ())
        })
        .await
    }

    fn saved(paths: &Paths, id: &str) -> Chat {
        History::new(paths.chats()).load(id).expect("saved chat")
    }

    fn chat_on(id: &str, model: &str, ctx: u32) -> Chat {
        Chat {
            id: id.into(),
            created: 1,
            updated: 1,
            model_file: Some(model.into()),
            turns: vec![turn(
                "Earlier question",
                "Earlier answer",
                "Be brief.",
                0.5,
                ctx,
            )],
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn each_answer_is_saved_into_its_chat_with_its_model() {
        let s = start(&[], &[A]);
        ready(&s.app, A).await;
        s.app.send("Hi").unwrap();
        until("answer saved", || (s.app.chats().len() == 1).then_some(())).await;
        let c = s.app.snapshot().chat_id.unwrap();
        let file = saved(&s.paths, &c);
        assert_eq!(file.model_file.as_deref(), Some(A));
        assert_eq!(file.turns.len(), 1);
        assert_eq!(file.turns[0].answer, "Hello from test.");
        s.app.send("Again").unwrap();
        until("second answer saved", || {
            (saved(&s.paths, &c).turns.len() == 2).then_some(())
        })
        .await;
        assert_eq!(s.app.chats().len(), 1, "the same chat, not a new one");
        s.app.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn opening_a_chat_loads_its_model_and_context() {
        let s = start(&[chat_on("10-1", B, 4096)], &[A, B]);
        ready(&s.app, A).await;
        s.app.open_chat("10-1").unwrap();
        ready(&s.app, B).await;
        let snap = s.app.snapshot();
        assert_eq!(snap.settings.model.as_deref(), Some(B));
        assert_eq!(snap.settings.context_size, 4096);
        assert!(
            !s.events
                .lock()
                .unwrap()
                .iter()
                .any(|e| matches!(e, Event::Notice { .. })),
            "no notice: the model was there"
        );
        s.app.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_chat_whose_model_is_gone_or_damaged_goes_on_with_the_current_one_and_says_so() {
        for missing in ["gone.gguf", "broken.gguf"] {
            let s = start(&[chat_on("20-1", missing, 2048)], &[A]);
            if missing == "broken.gguf" {
                std::fs::write(s.paths.models().join("broken.gguf"), b"not a model").unwrap();
                s.app.rescan();
            }
            ready(&s.app, A).await;
            s.app.open_chat("20-1").unwrap();
            assert_eq!(
                s.app.snapshot().settings.model.as_deref(),
                Some(A),
                "{missing}"
            );
            let told = s.events.lock().unwrap().iter().any(|e| {
                matches!(e, Event::Notice { error }
                if error.title.contains(if missing == "broken.gguf" {
                    "is damaged"
                } else {
                    "is not on this computer any more"
                }))
            });
            assert!(told, "{missing}: the learner is told");
            s.app.shutdown().await;
        }
    }

    /// The race the first review found: an answer finishing while another chat
    /// opens must never be saved into the opened chat (wrong model, wrong turns).
    #[tokio::test(flavor = "multi_thread")]
    async fn opening_a_chat_mid_answer_keeps_both_chats_right() {
        for round in 0..8u64 {
            let s = start(&[chat_on("30-1", B, 2048)], &[A, B]);
            ready(&s.app, A).await;
            s.app.send("SLOW please").unwrap();
            some_words(&s.app).await;
            // Vary the moment a little from round to round.
            tokio::time::sleep(Duration::from_millis(round * 7)).await;
            s.app.open_chat("30-1").unwrap();
            ready(&s.app, B).await;
            tokio::time::sleep(Duration::from_millis(300)).await;
            let y = saved(&s.paths, "30-1");
            assert_eq!(
                y.model_file.as_deref(),
                Some(B),
                "round {round}: the opened chat keeps its model"
            );
            assert_eq!(y.turns.len(), 1, "round {round}: and only its own turns");
            // The chat that was left is kept, its answer cut short and stopped.
            let left: Vec<_> = s
                .app
                .chats()
                .into_iter()
                .filter(|c| c.id != "30-1")
                .collect();
            assert_eq!(left.len(), 1, "round {round}");
            let x = saved(&s.paths, &left[0].id);
            assert_eq!(x.turns[0].status, TurnStatus::Stopped);
            assert!(
                x.turns[0].answer.starts_with("word "),
                "round {round}: the partial answer is kept"
            );
            assert_eq!(x.model_file.as_deref(), Some(A));
            s.app.shutdown().await;
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn new_chat_mid_answer_keeps_the_partial_answer_and_starts_clean() {
        let s = start(&[], &[A]);
        ready(&s.app, A).await;
        s.app.send("SLOW please").unwrap();
        some_words(&s.app).await;
        s.app.new_chat();
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(s.app.turns().is_empty());
        assert_eq!(s.app.snapshot().chat_id, None);
        let l = s.app.chats();
        assert_eq!(l.len(), 1);
        let x = saved(&s.paths, &l[0].id);
        assert_eq!(x.turns[0].status, TurnStatus::Stopped);
        assert!(x.turns[0].answer.starts_with("word "));
        s.app.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_chat_deleted_mid_answer_stays_deleted() {
        let s = start(&[], &[A]);
        ready(&s.app, A).await;
        s.app.send("Hi").unwrap();
        until("first answer saved", || {
            (s.app.chats().len() == 1).then_some(())
        })
        .await;
        let id = s.app.snapshot().chat_id.unwrap();
        s.app.send("SLOW please").unwrap();
        some_words(&s.app).await;
        s.app.delete_chat(&id).unwrap();
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert!(s.app.chats().is_empty());
        assert!(
            !s.paths.chats().join(format!("{id}.json")).exists(),
            "not saved back"
        );
        assert!(s.app.turns().is_empty());
        s.app.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn closing_trurl_mid_answer_keeps_the_answer_so_far() {
        let s = start(&[], &[A]);
        ready(&s.app, A).await;
        s.app.send("SLOW please").unwrap();
        some_words(&s.app).await;
        s.app.shutdown().await;
        let l = History::new(s.paths.chats()).list();
        assert_eq!(l.len(), 1);
        let x = saved(&s.paths, &l[0].id);
        assert_eq!(x.turns[0].status, TurnStatus::Stopped);
        assert!(x.turns[0].answer.starts_with("word "));
    }

    /// Round 2 of the review: switching models while looking at a chat, then
    /// leaving it, must not change the model saved with it.
    #[tokio::test(flavor = "multi_thread")]
    async fn looking_at_a_chat_and_switching_models_changes_nothing_in_it() {
        let s = start(&[chat_on("10-1", B, 2048)], &[A, B]);
        ready(&s.app, A).await;
        s.app.open_chat("10-1").unwrap();
        ready(&s.app, B).await;
        s.app.select_model(A);
        ready(&s.app, A).await;
        s.app.new_chat();
        let c = saved(&s.paths, "10-1");
        assert_eq!(
            c.model_file.as_deref(),
            Some(B),
            "still the model of its last answer"
        );
        assert_eq!(c.updated, 1, "not saved again: nothing changed");
        s.app.shutdown().await;
    }

    /// Opening chats only to read them never moves them to the top of the list.
    #[tokio::test(flavor = "multi_thread")]
    async fn reading_chats_does_not_change_their_date() {
        let mut other = chat_on("11-1", A, 2048);
        other.updated = 2;
        let s = start(&[chat_on("10-1", A, 2048), other], &[A]);
        ready(&s.app, A).await;
        s.app.open_chat("10-1").unwrap();
        s.app.open_chat("11-1").unwrap();
        s.app.new_chat();
        assert_eq!(saved(&s.paths, "10-1").updated, 1);
        assert_eq!(saved(&s.paths, "11-1").updated, 2);
        let order: Vec<String> = s.app.chats().into_iter().map(|c| c.id).collect();
        assert_eq!(order, ["11-1", "10-1"]);
        // A question in it does: it is saved, with the model that answered.
        s.app.open_chat("10-1").unwrap();
        s.app.send("Hi").unwrap();
        until("saved", || {
            (saved(&s.paths, "10-1").turns.len() == 2).then_some(())
        })
        .await;
        assert!(saved(&s.paths, "10-1").updated > 2);
        assert_eq!(s.app.chats()[0].id, "10-1");
        s.app.shutdown().await;
    }

    /// A chat that cannot be saved (disk full, folder read-only) stays on
    /// screen when the learner presses New chat, instead of vanishing.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_chat_that_cannot_be_saved_stays_on_screen() {
        use std::os::unix::fs::PermissionsExt;
        let s = start(&[], &[A]);
        ready(&s.app, A).await;
        s.app.send("Hi").unwrap();
        until("saved", || (s.app.chats().len() == 1).then_some(())).await;
        let dir = s.paths.chats();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500)).unwrap();
        if std::fs::write(dir.join(".probe"), b"x").is_ok() {
            // Running as root: permissions do not stop writes, nothing to test.
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
            s.app.shutdown().await;
            return;
        }
        let id = s.app.send("Again").unwrap();
        until("answer", || {
            s.app
                .turns()
                .into_iter()
                .find(|t| t.id == id && t.status != TurnStatus::Streaming)
        })
        .await;
        s.app.new_chat();
        assert_eq!(s.app.turns().len(), 2, "the unsaved chat stays on screen");
        assert!(s.events.lock().unwrap().iter().any(|e| matches!(e,
            Event::Notice { error } if error.title == "This chat could not be saved.")));
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        s.app.new_chat();
        assert!(
            s.app.turns().is_empty(),
            "once it can be saved, New chat works"
        );
        s.app.shutdown().await;
    }

    /// Round 3 of the review: a question sent the instant the previous answer
    /// ends, then New chat during it. The partial answer must be kept.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_question_sent_right_after_an_answer_is_kept_when_leaving() {
        for round in 0..15 {
            let s = start(&[], &[A]);
            ready(&s.app, A).await;
            let first = s.app.send("Hi").unwrap();
            // Send again the moment the first answer is marked finished.
            loop {
                if s.app
                    .turns()
                    .iter()
                    .any(|t| t.id == first && t.status != TurnStatus::Streaming)
                    && s.app.send("SLOW please").is_ok()
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
            some_words(&s.app).await;
            s.app.new_chat();
            let l = s.app.chats();
            assert_eq!(l.len(), 1, "round {round}");
            let c = saved(&s.paths, &l[0].id);
            assert_eq!(c.turns.len(), 2, "round {round}");
            assert!(
                c.turns[1].answer.starts_with("word "),
                "round {round}: the partial answer is kept: {:?}",
                c.turns[1].answer
            );
            s.app.shutdown().await;
        }
    }
}
