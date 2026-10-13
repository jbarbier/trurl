//! Evals against the real, pinned llama-server and real models.
//!
//! Slow (minutes) and need files that are not in the repo, so they are
//! `#[ignore]`d. Run them with:
//!
//!   python3 scripts/build_llama.py            # vendor/llama/<os>/{avx2,avx,compat}
//!   python3 scripts/fetch_models.py --dest ~/trurl-models
//!   TRURL_TEST_MODELS=~/trurl-models cargo test -p trurl-core --test live -- --ignored --test-threads 1
//!
//! `scripts/run_evals.sh` does all of that, offline (network blocked).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use trurl_core::app::{App, AppConfig};
use trurl_core::chat::TurnStatus;
use trurl_core::errors::ErrorCode;
use trurl_core::events::{EngineState, Event};
use trurl_core::paths::Paths;

const GEMMA: &str = "gemma-3-1b-it-Q4_K_M.gguf";
const QWEN: &str = "Qwen_Qwen3-0.6B-Q4_K_M.gguf";

struct Harness {
    app: App,
    events: Arc<Mutex<Vec<Event>>>,
    _home: tempfile::TempDir,
}

fn models_src() -> PathBuf {
    PathBuf::from(
        std::env::var("TRURL_TEST_MODELS")
            .expect("set TRURL_TEST_MODELS to a folder with the pinned models"),
    )
}

fn llama_root() -> PathBuf {
    if let Some(d) = std::env::var_os("TRURL_LLAMA_DIR") {
        return d.into();
    }
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../vendor/llama")
        .join(std::env::consts::OS);
    assert!(
        p.join("compat").is_dir(),
        "build llama-server first: python3 scripts/build_llama.py"
    );
    p
}

/// A fresh Trurl home with hard links to the requested models (no copy of gigabytes).
fn harness(models: &[&str], tweak: impl FnOnce(&Path)) -> Harness {
    harness_in("", models, tweak)
}

/// A harness whose data folder sits under `sub` (for example a user name with
/// accents, like `C:\\Users\\Amédée`).
fn harness_in(sub: &str, models: &[&str], tweak: impl FnOnce(&Path)) -> Harness {
    let home = tempfile::tempdir().unwrap();
    let paths = Paths::at(home.path().join(sub));
    paths.ensure().unwrap();
    for m in models {
        let src = models_src().join(m);
        let dst = paths.models().join(m);
        std::fs::hard_link(&src, &dst)
            .or_else(|_| std::fs::copy(&src, &dst).map(|_| ()))
            .unwrap();
    }
    tweak(home.path());
    let events: Arc<Mutex<Vec<Event>>> = Arc::default();
    let ev = events.clone();
    let cfg = AppConfig {
        paths,
        llama_root: Some(llama_root()),
        hf_base: "http://127.0.0.1:9".into(),
        user_agent: "trurl-test".into(),
    };
    let app = App::start(cfg, Arc::new(move |e| ev.lock().unwrap().push(e)));
    Harness {
        app,
        events,
        _home: home,
    }
}

impl Harness {
    async fn wait<T>(
        &self,
        what: &str,
        secs: u64,
        mut f: impl FnMut(&App, &[Event]) -> Option<T>,
    ) -> T {
        let t0 = Instant::now();
        loop {
            {
                let ev = self.events.lock().unwrap();
                if let Some(v) = f(&self.app, &ev) {
                    return v;
                }
            }
            assert!(
                t0.elapsed() < Duration::from_secs(secs),
                "timed out waiting for {what}"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    async fn ready(&self, file: &str) {
        let f = file.to_string();
        self.wait(&format!("{file} ready"), 300, |app, _| {
            match app.snapshot().engine {
                EngineState::Ready { file, .. } if file == f => Some(()),
                // A crash is reported, then the model reloads by itself: keep waiting.
                EngineState::Failed { error, .. } if error.code == ErrorCode::ServerCrashed => None,
                EngineState::Failed { error, .. } => panic!("load failed: {error:?}"),
                _ => None,
            }
        })
        .await
    }

    /// Send and wait for the turn to finish; returns the finished turn.
    async fn ask(&self, text: &str) -> trurl_core::chat::Turn {
        let id = self.app.send(text).expect("send");
        self.wait("turn finished", 600, |app, _| {
            app.turns()
                .into_iter()
                .find(|t| t.id == id && t.status != TurnStatus::Streaming)
        })
        .await
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn chat_streams_tokens_and_answers() {
    let h = harness(&[GEMMA], |_| {});
    h.ready(GEMMA).await;
    let t = h
        .ask("What is the capital of Ghana? Answer in one word.")
        .await;
    assert_eq!(t.status, TurnStatus::Done, "{t:?}");
    assert!(t.answer.to_lowercase().contains("accra"), "{}", t.answer);
    assert!(t.reasoning.is_empty());
    let deltas = h
        .events
        .lock()
        .unwrap()
        .iter()
        .filter(|e| {
            matches!(
                e,
                Event::TurnDelta {
                    answer: Some(_),
                    ..
                }
            )
        })
        .count();
    assert!(
        deltas >= 2,
        "answer should stream in several pieces, got {deltas}"
    );
    h.app.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn reasoning_is_separate_timed_and_never_sent_back() {
    let h = harness(&[QWEN], |_| {});
    h.ready(QWEN).await;
    // A 0.6B model sometimes thinks in circles until the context is full. That is
    // real behaviour (Trurl then says the conversation ran out of room), so this
    // eval has a pass threshold: one of three attempts must finish. Every attempt
    // must keep the reasoning out of the answer.
    let mut t = None;
    for attempt in 0..3 {
        if attempt > 0 {
            h.app.new_chat();
            h.events.lock().unwrap().clear();
        }
        let turn = h.ask("What is 17 + 25? Reply with the number only.").await;
        assert!(!turn.answer.contains("<think>") && !turn.answer.contains("</think>"));
        assert!(!turn.reasoning.trim().is_empty(), "Qwen3 should think");
        if turn.status == TurnStatus::Done {
            t = Some(turn);
            break;
        }
        assert_eq!(turn.status, TurnStatus::Cut, "{turn:?}");
    }
    let t = t.expect("Qwen3 did not finish thinking in 3 attempts");
    assert!(t.answer.contains("42"), "answer: {}", t.answer);
    assert!(!t.answer.contains("<think>") && !t.answer.contains("</think>"));
    assert!(t.thought_ms.is_some());
    // The reasoning streamed live, before the answer.
    let ev = h.events.lock().unwrap().clone();
    let first_r = ev
        .iter()
        .position(|e| {
            matches!(
                e,
                Event::TurnDelta {
                    reasoning: Some(_),
                    ..
                }
            )
        })
        .unwrap();
    let thought = ev
        .iter()
        .position(|e| matches!(e, Event::TurnThought { .. }))
        .unwrap();
    let first_a = ev
        .iter()
        .position(|e| {
            matches!(
                e,
                Event::TurnDelta {
                    answer: Some(_),
                    ..
                }
            )
        })
        .unwrap();
    assert!(first_r < thought && thought < first_a);
    let h2 = trurl_core::chat::history("", &h.app.turns(), "next");
    assert!(h2.iter().all(|m| !m.content.contains(t.reasoning.trim())));
    h.app.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn no_think_switch_leaves_reasoning_empty() {
    let h = harness(&[QWEN], |_| {});
    h.ready(QWEN).await;
    let t = h.ask("/no_think Say hello in French, one word.").await;
    assert_eq!(t.status, TurnStatus::Done);
    assert!(
        t.reasoning.trim().is_empty(),
        "reasoning: {:?}",
        t.reasoning
    );
    assert!(!t.answer.trim().is_empty());
    h.app.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn temperature_zero_is_repeatable() {
    let h = harness(&[GEMMA], |_| {});
    h.ready(GEMMA).await;
    h.app.set_temperature(0.0);
    let a = h.ask("Write one sentence about the sea.").await.answer;
    h.app.new_chat();
    let b = h.ask("Write one sentence about the sea.").await.answer;
    assert_eq!(a, b);
    h.app.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn small_context_runs_out_of_room_with_the_course_message() {
    let h = harness(&[GEMMA], |_| {});
    h.ready(GEMMA).await;
    h.app.set_context(256);
    h.wait("reload at 256", 300, |app, _| match app.snapshot().engine {
        EngineState::Ready { context: 256, .. } => Some(()),
        _ => None,
    })
    .await;
    // Too long to even start, as the first message: "New chat" would not help.
    let long = "word ".repeat(400);
    let t = h.ask(&long).await;
    assert_eq!(t.status, TurnStatus::Error);
    let e = t.error.as_ref().unwrap();
    assert_eq!(e.code, ErrorCode::ContextFull);
    assert_eq!(e.title, "Your message is too long for the context size.");
    // Fills up while answering, later in the chat: the course's own words.
    let t = h.ask("Count from 1 to 300, one number per line.").await;
    assert_eq!(t.status, TurnStatus::Cut, "{t:?}");
    let e = t.error.as_ref().unwrap();
    assert_eq!(
        format!("{} {}", e.title, e.hint),
        "The conversation ran out of room. Start a new chat or increase the context size."
    );
    assert_eq!(t.context, 256, "the turn remembers the context it ran in");
    h.app.shutdown().await;
}

/// Course 1.1.4 with a system prompt (1.2.2): at temperature 0 the same
/// question gives the same answer, chat after chat, even when an earlier chat
/// left part of the prompt in llama-server's cache.
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn temperature_zero_is_repeatable_with_a_system_prompt() {
    let h = harness(&[GEMMA], |_| {});
    h.ready(GEMMA).await;
    h.app.set_system_prompt("Answer only in French.");
    h.ask("Say hello in one sentence.").await; // leaves a cached prompt behind
    h.app.set_temperature(0.0);
    let mut answers = Vec::new();
    for _ in 0..3 {
        h.app.new_chat();
        let t = h
            .ask("Give me one name for a cat, and say why in one sentence.")
            .await;
        assert_eq!(t.status, TurnStatus::Done);
        assert_eq!(t.temperature, 0.0);
        assert_eq!(t.system_prompt, "Answer only in French.");
        answers.push(t.answer);
    }
    assert!(
        answers.iter().all(|a| *a == answers[0]),
        "temperature 0 must repeat: {answers:#?}"
    );
    h.app.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn stop_then_chat_again_keeps_gemma_working() {
    let h = harness(&[GEMMA], |_| {});
    h.ready(GEMMA).await;
    // Stop before any answer arrives: the turn must not break the next request.
    let id = h.app.send("Write a long story about a goat.").unwrap();
    h.app.stop_generation();
    let t = h
        .wait("stopped", 60, |app, _| {
            app.turns()
                .into_iter()
                .find(|t| t.id == id && t.status != TurnStatus::Streaming)
        })
        .await;
    assert_eq!(t.status, TurnStatus::Stopped);
    // Stop in the middle of an answer.
    let id = h.app.send("Write a long story about a cat.").unwrap();
    h.wait("some answer", 120, |app, _| {
        app.turns()
            .into_iter()
            .find(|t| t.id == id && t.answer.len() > 20)
    })
    .await;
    h.app.stop_generation();
    let t = h
        .wait("stopped", 60, |app, _| {
            app.turns()
                .into_iter()
                .find(|t| t.id == id && t.status != TurnStatus::Streaming)
        })
        .await;
    assert_eq!(t.status, TurnStatus::Stopped);
    let partial = t.answer.clone();
    // What goes back to the model: the partial cat story, not the empty goat turn.
    let next = trurl_core::chat::history("", &h.app.turns(), "next");
    let users: Vec<&str> = next
        .iter()
        .filter(|m| m.role == "user")
        .map(|m| m.content.as_str())
        .collect();
    assert_eq!(users, vec!["Write a long story about a cat.", "next"]);
    assert_eq!(next[1].content, partial);
    // And Gemma's template accepts it: the chat goes on normally.
    let t = h.ask("Thanks. Now say hello.").await;
    assert_eq!(t.status, TurnStatus::Done, "{t:?}");
    assert!(!t.answer.trim().is_empty());
    h.app.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn system_prompt_is_obeyed_and_new_chat_forgets() {
    let h = harness(&[GEMMA], |_| {});
    h.ready(GEMMA).await;
    h.app
        .set_system_prompt("You are a pirate. Always start your reply with the word Arr.");
    let t = h.ask("Hello!").await;
    assert!(t.answer.to_lowercase().starts_with("arr"), "{}", t.answer);
    h.app.set_system_prompt("");
    h.app.new_chat();
    assert!(h.app.turns().is_empty());
    h.app.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn switching_models_restarts_the_server() {
    let h = harness(&[GEMMA, QWEN], |_| {});
    h.ready(GEMMA).await;
    h.app.select_model(QWEN);
    h.ready(QWEN).await;
    let t = h.ask("/no_think Say OK.").await;
    assert_eq!(t.model, "Qwen3 0.6B");
    h.app.select_model(GEMMA);
    h.ready(GEMMA).await;
    let t = h.ask("Say OK.").await;
    assert_eq!(t.model, "Gemma 3 1B Instruct");
    h.app.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn server_crash_is_reported_and_recovered() {
    let h = harness(&[GEMMA], |_| {});
    h.ready(GEMMA).await;
    // Find llama-server's pid from its port and kill it like a crash would.
    kill_llama_servers();
    h.wait("crash reported", 30, |_, ev| {
        ev.iter().any(|e| matches!(e, Event::Engine { engine: EngineState::Failed { error, .. } } if error.code == ErrorCode::ServerCrashed)).then_some(())
    })
    .await;
    h.ready(GEMMA).await;
    let t = h.ask("Say OK.").await;
    assert_eq!(t.status, TurnStatus::Done);
    h.app.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn deleting_the_loaded_model_stops_the_server_first() {
    let h = harness(&[GEMMA, QWEN], |_| {});
    h.ready(GEMMA).await;
    h.app.delete_model(GEMMA).await.expect("delete");
    assert!(!h.app.models_dir().join(GEMMA).exists());
    // Falls back to the other model.
    h.ready(QWEN).await;
    h.app.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn empty_folder_waits_for_a_model_then_import_loads_it() {
    let h = harness(&[], |_| {});
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(h.app.snapshot().engine, EngineState::Idle);
    assert_eq!(h.app.send("hi").unwrap_err().code, ErrorCode::NoModel);
    h.app.import_model(models_src().join(QWEN));
    h.ready(QWEN).await;
    h.app.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn damaged_model_gets_a_plain_error() {
    let h = harness(&[], |home| {
        let src = models_src().join(GEMMA);
        let dst = home.join("models").join("gemma-cut.gguf");
        let mut f = std::fs::File::open(src).unwrap();
        let mut out = std::fs::File::create(dst).unwrap();
        std::io::copy(&mut std::io::Read::take(&mut f, 50 * 1024 * 1024), &mut out).unwrap();
    });
    let m = h.app.models();
    assert_eq!(m.len(), 1);
    assert!(m[0].broken, "a cut file must show as broken");
    h.app.select_model("gemma-cut.gguf");
    h.wait("failure", 60, |app, _| match app.snapshot().engine {
        EngineState::Failed { error, .. } if error.code == ErrorCode::ModelCorrupt => Some(()),
        _ => None,
    })
    .await;
    h.app.shutdown().await;
}

/// Windows CI only (TRURL_EXPECT_OFFLINE=1, set by scripts/run_evals.ps1 after
/// blocking the network): proves the firewall block works, so every other eval
/// really ran offline. On Linux the network shim proves the same thing.
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn network_is_really_blocked() {
    if std::env::var_os("TRURL_EXPECT_OFFLINE").is_none() {
        return;
    }
    let home = tempfile::tempdir().unwrap();
    let cfg = AppConfig {
        paths: Paths::at(home.path()),
        llama_root: Some(llama_root()),
        hf_base: trurl_core::hub::HF_BASE.into(),
        user_agent: "trurl-test".into(),
    };
    let app = App::start(cfg, Arc::new(|_| {}));
    assert!(
        !app.hub_online().await,
        "Hugging Face answered: the network is not blocked, so the evals did not run offline"
    );
    app.shutdown().await;
}

fn kill_llama_servers() {
    #[cfg(unix)]
    {
        let _ = std::process::Command::new("pkill")
            .args(["-9", "-f", "llama-server -m gemma"])
            .status();
    }
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("taskkill")
            .args(["/F", "/IM", "llama-server.exe"])
            .status();
    }
}

/// Regression: "New chat" while an answer streams used to leave both apps stuck in
/// "generating" forever. The answer must end, the apps must hear it, and the next
/// message must go through.
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn new_chat_during_an_answer_frees_the_chat() {
    let h = harness(&[GEMMA], |_| {});
    h.ready(GEMMA).await;
    let id = h.app.send("Write a long story about a lion.").unwrap();
    h.wait("some answer", 120, |app, _| {
        app.turns()
            .into_iter()
            .find(|t| t.id == id && t.answer.len() > 20)
    })
    .await;
    h.app.new_chat();
    h.wait("generating false", 30, |app, ev| {
        let told = ev
            .iter()
            .rev()
            .any(|e| matches!(e, Event::Generating { generating: false }));
        (told && !app.is_generating()).then_some(())
    })
    .await;
    assert!(h.app.turns().is_empty());
    let t = h.ask("Say hello.").await;
    assert_eq!(t.status, TurnStatus::Done);
    h.app.shutdown().await;
}

/// Learners' Windows user names often have accents, and model files get renamed
/// in their own language. llama-server must still find and load the file.
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn a_model_with_accents_in_its_name_and_folder_loads() {
    let tiny = models_src().join("stories260K-f32.gguf");
    assert!(
        tiny.exists(),
        "scripts/run_evals.sh fetches stories260K-f32.gguf; run it first"
    );
    let name = "Modèle été ünïcode.gguf";
    let h = harness_in("Amédée Ngozi", &[], |home| {
        let models = home.join("Amédée Ngozi").join("models");
        std::fs::create_dir_all(&models).unwrap();
        std::fs::copy(&tiny, models.join(name)).unwrap();
    });
    h.app.rescan();
    h.app.select_model(name);
    h.ready(name).await;
    let t = h.ask("Once upon a time").await;
    assert!(!t.answer.is_empty(), "{t:?}");
    h.app.shutdown().await;
}

/// The update rule born from the Bionic incident: an update never cuts off an
/// answer. Releasing the engine is refused while one is being written; once
/// released, no new answer can start until the update fails and the model
/// comes back.
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn an_update_never_cuts_off_an_answer() {
    let h = harness(&[GEMMA], |_| {});
    h.ready(GEMMA).await;
    let id = h.app.send("Write a long story about a lion.").unwrap();
    h.wait("answer started", 120, |app, _| {
        app.turns()
            .into_iter()
            .find(|t| t.id == id && !t.answer.is_empty())
    })
    .await;
    let refused = h.app.release_engine().await.unwrap_err();
    assert_eq!(refused.code, trurl_core::ErrorCode::UpdateBusy);
    assert!(h.app.is_generating(), "the answer goes on");
    h.app.stop_generation();
    h.wait("generating false", 30, |app, _| {
        (!app.is_generating()).then_some(())
    })
    .await;
    h.app.release_engine().await.unwrap();
    let held = h.app.send("Hello?").unwrap_err();
    assert_eq!(held.title, "Trurl is installing an update.");
    // Ctrl+R / "Try again" during the install must not bring the model back.
    h.app.reload();
    assert_eq!(
        h.app.send("Hello?").unwrap_err().title,
        "Trurl is installing an update."
    );
    // The install failed: the model comes back and chatting works again.
    h.app.update_failed_reload();
    h.ready(GEMMA).await;
    let t = h.ask("Say hello.").await;
    assert_eq!(t.status, TurnStatus::Done);
    h.app.shutdown().await;
}

/// Session 1.1.2: a pre-trained (base) model has no chat template. It must
/// still load and write something, and Stop must work, like in the course.
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn a_model_without_a_chat_template_still_runs() {
    let tiny = models_src().join("stories260K-f32.gguf");
    if !tiny.exists() {
        panic!("scripts/run_evals.sh fetches stories260K-f32.gguf; run it first");
    }
    let h = harness(&["stories260K-f32.gguf"], |_| {});
    h.ready("stories260K-f32.gguf").await;
    assert!(
        !h.app.models()[0].has_chat_template,
        "this test needs a file without a template"
    );
    let t = h.ask("Once upon a time").await;
    assert!(
        matches!(t.status, TurnStatus::Done | TurnStatus::Cut),
        "{t:?}"
    );
    assert!(!t.answer.trim().is_empty(), "the base model wrote nothing");
    h.app.shutdown().await;
}

/// Reading a model's header runs at start and after every import or download:
/// it must stay fast even for big vocabularies (262k tokens for Gemma 3).
#[test]
#[ignore]
fn reading_model_headers_is_fast() {
    for m in [GEMMA, QWEN] {
        let p = models_src().join(m);
        let t0 = Instant::now();
        let info = trurl_core::gguf::read(&p).expect("parse");
        let ms = t0.elapsed().as_millis();
        eprintln!("{m}: {ms} ms, {:?}", info.architecture);
        assert!(ms < 400, "{m} header took {ms} ms");
    }
}
