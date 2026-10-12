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
    PathBuf::from(std::env::var("TRURL_TEST_MODELS").expect("set TRURL_TEST_MODELS to a folder with the pinned models"))
}

fn llama_root() -> PathBuf {
    if let Some(d) = std::env::var_os("TRURL_LLAMA_DIR") {
        return d.into();
    }
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../vendor/llama").join(std::env::consts::OS);
    assert!(p.join("compat").is_dir(), "build llama-server first: python3 scripts/build_llama.py");
    p
}

/// A fresh Trurl home with hard links to the requested models (no copy of gigabytes).
fn harness(models: &[&str], tweak: impl FnOnce(&Path)) -> Harness {
    let home = tempfile::tempdir().unwrap();
    let paths = Paths::at(home.path());
    paths.ensure().unwrap();
    for m in models {
        let src = models_src().join(m);
        let dst = paths.models().join(m);
        std::fs::hard_link(&src, &dst).or_else(|_| std::fs::copy(&src, &dst).map(|_| ())).unwrap();
    }
    tweak(home.path());
    let events: Arc<Mutex<Vec<Event>>> = Arc::default();
    let ev = events.clone();
    let cfg = AppConfig { paths, llama_root: Some(llama_root()), hf_base: "http://127.0.0.1:9".into(), user_agent: "trurl-test".into() };
    let app = App::start(cfg, Arc::new(move |e| ev.lock().unwrap().push(e)));
    Harness { app, events, _home: home }
}

impl Harness {
    async fn wait<T>(&self, what: &str, secs: u64, mut f: impl FnMut(&App, &[Event]) -> Option<T>) -> T {
        let t0 = Instant::now();
        loop {
            {
                let ev = self.events.lock().unwrap();
                if let Some(v) = f(&self.app, &ev) {
                    return v;
                }
            }
            assert!(t0.elapsed() < Duration::from_secs(secs), "timed out waiting for {what}");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    async fn ready(&self, file: &str) {
        let f = file.to_string();
        self.wait(&format!("{file} ready"), 300, |app, _| {
            match app.snapshot().engine {
                EngineState::Ready { file, .. } if file == f => Some(()),
                EngineState::Failed { error, .. } => panic!("load failed: {error:?}"),
                _ => None,
            }
        })
        .await
    }

    /// Send and wait for the turn to finish; returns the finished turn.
    async fn ask(&self, text: &str) -> trurl_core::chat::Turn {
        let id = self.app.send(text).expect("send");
        self.wait("turn finished", 600, |app, _| app.turns().into_iter().find(|t| t.id == id && t.status != TurnStatus::Streaming)).await
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn chat_streams_tokens_and_answers() {
    let h = harness(&[GEMMA], |_| {});
    h.ready(GEMMA).await;
    let t = h.ask("What is the capital of Ghana? Answer in one word.").await;
    assert_eq!(t.status, TurnStatus::Done, "{t:?}");
    assert!(t.answer.to_lowercase().contains("accra"), "{}", t.answer);
    assert!(t.reasoning.is_empty());
    let deltas = h.events.lock().unwrap().iter().filter(|e| matches!(e, Event::TurnDelta { answer: Some(_), .. })).count();
    assert!(deltas >= 2, "answer should stream in several pieces, got {deltas}");
    h.app.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn reasoning_is_separate_timed_and_never_sent_back() {
    let h = harness(&[QWEN], |_| {});
    h.ready(QWEN).await;
    let t = h.ask("What is 17 + 25? Reply with the number only.").await;
    assert_eq!(t.status, TurnStatus::Done, "{t:?}");
    assert!(!t.reasoning.trim().is_empty(), "Qwen3 should think");
    assert!(t.answer.contains("42"), "answer: {}", t.answer);
    assert!(!t.answer.contains("<think>") && !t.answer.contains("</think>"));
    assert!(t.thought_ms.is_some());
    // The reasoning streamed live, before the answer.
    let ev = h.events.lock().unwrap().clone();
    let first_r = ev.iter().position(|e| matches!(e, Event::TurnDelta { reasoning: Some(_), .. })).unwrap();
    let thought = ev.iter().position(|e| matches!(e, Event::TurnThought { .. })).unwrap();
    let first_a = ev.iter().position(|e| matches!(e, Event::TurnDelta { answer: Some(_), .. })).unwrap();
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
    assert!(t.reasoning.trim().is_empty(), "reasoning: {:?}", t.reasoning);
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
    // Too long to even start.
    let long = "word ".repeat(400);
    let t = h.ask(&long).await;
    assert_eq!(t.status, TurnStatus::Error);
    assert_eq!(t.error.as_ref().unwrap().code, ErrorCode::ContextFull);
    // Fills up while answering.
    let t = h.ask("Count from 1 to 300, one number per line.").await;
    assert_eq!(t.status, TurnStatus::Cut, "{t:?}");
    assert_eq!(t.error.as_ref().unwrap().code, ErrorCode::ContextFull);
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
    let t = h.wait("stopped", 60, |app, _| app.turns().into_iter().find(|t| t.id == id && t.status != TurnStatus::Streaming)).await;
    assert_eq!(t.status, TurnStatus::Stopped);
    // Stop in the middle of an answer.
    let id = h.app.send("Write a long story about a cat.").unwrap();
    h.wait("some answer", 120, |app, _| app.turns().into_iter().find(|t| t.id == id && t.answer.len() > 20)).await;
    h.app.stop_generation();
    let t = h.wait("stopped", 60, |app, _| app.turns().into_iter().find(|t| t.id == id && t.status != TurnStatus::Streaming)).await;
    assert_eq!(t.status, TurnStatus::Stopped);
    let t = h.ask("Thanks. What animal was the story about? One word.").await;
    assert_eq!(t.status, TurnStatus::Done, "{t:?}");
    assert!(t.answer.to_lowercase().contains("cat"), "{}", t.answer);
    h.app.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn system_prompt_is_obeyed_and_new_chat_forgets() {
    let h = harness(&[GEMMA], |_| {});
    h.ready(GEMMA).await;
    h.app.set_system_prompt("You are a pirate. Always start your reply with the word Arr.");
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

fn kill_llama_servers() {
    #[cfg(unix)]
    {
        let _ = std::process::Command::new("pkill").args(["-9", "-f", "llama-server -m gemma"]).status();
    }
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("taskkill").args(["/F", "/IM", "llama-server.exe"]).status();
    }
}
