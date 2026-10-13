//! One entry point for every UI command: `dispatch(app, "send", {"text": "hi"})`.
//!
//! The Tauri shell and the dev bridge both forward to this, so the desktop app
//! and the browser-tested UI cannot drift apart. Desktop-only commands
//! (updates, file pickers, opening folders) are handled by the shell before it
//! gets here. Contract: `contracts/events.md`.

use crate::app::App;
use crate::downloads::DownloadRequest;
use crate::errors::UserError;
use serde::Deserialize;
use serde_json::{Value, json};

fn arg<T: for<'de> Deserialize<'de>>(args: &Value) -> Result<T, UserError> {
    serde_json::from_value(args.clone())
        .map_err(|e| UserError::internal(format!("bad arguments: {e}")))
}

#[derive(Deserialize)]
struct Text {
    text: String,
}
#[derive(Deserialize)]
struct Num {
    value: f64,
}
#[derive(Deserialize)]
struct File {
    file: String,
}
#[derive(Deserialize)]
struct PathArg {
    path: String,
}
#[derive(Deserialize)]
struct Query {
    query: String,
}
#[derive(Deserialize)]
struct Repo {
    repo: String,
}
#[derive(Deserialize)]
struct Id {
    id: String,
}
#[derive(Deserialize)]
struct Token {
    token: String,
}
#[derive(Deserialize)]
struct Dl {
    repo: String,
    file: String,
    revision: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    sha256: Option<String>,
}

pub async fn dispatch(app: &App, cmd: &str, args: Value) -> Result<Value, UserError> {
    let ok = || Ok(Value::Null);
    match cmd {
        "snapshot" => Ok(serde_json::to_value(app.snapshot()).unwrap_or(Value::Null)),
        "send" => app.send(&arg::<Text>(&args)?.text).map(|id| json!(id)),
        "stop" => {
            app.stop_generation();
            ok()
        }
        "new_chat" => {
            app.new_chat();
            ok()
        }
        "set_system_prompt" => {
            app.set_system_prompt(&arg::<Text>(&args)?.text);
            ok()
        }
        "set_temperature" => {
            app.set_temperature(arg::<Num>(&args)?.value as f32);
            ok()
        }
        "set_context" => {
            app.set_context(arg::<Num>(&args)?.value.max(0.0) as u32);
            ok()
        }
        "select_model" => {
            app.select_model(&arg::<File>(&args)?.file);
            ok()
        }
        "reload" => {
            app.reload();
            ok()
        }
        "import_model" => {
            app.import_model(arg::<PathArg>(&args)?.path.into());
            ok()
        }
        "delete_model" => app
            .delete_model(&arg::<File>(&args)?.file)
            .await
            .map(|_| Value::Null),
        "hub_online" => Ok(json!(app.hub_online().await)),
        "hub_search" => app
            .hub_search(&arg::<Query>(&args)?.query)
            .await
            .map(|v| json!(v)),
        "hub_details" => app
            .hub_details(&arg::<Repo>(&args)?.repo)
            .await
            .map(|v| json!(v)),
        "download_start" => {
            let d = arg::<Dl>(&args)?;
            app.download_start(DownloadRequest {
                repo: d.repo,
                file: d.file,
                revision: d.revision,
                size: d.size,
                sha256: d.sha256,
            })
            .map(|id| json!(id))
        }
        "download_course_model" => app
            .download_course_model(&arg::<Id>(&args)?.id)
            .map(|id| json!(id)),
        "download_pause" => {
            app.download_pause(&arg::<Id>(&args)?.id);
            ok()
        }
        "download_resume" => app
            .download_resume(&arg::<Id>(&args)?.id)
            .map(|_| Value::Null),
        "download_cancel" => {
            app.download_cancel(&arg::<Id>(&args)?.id);
            ok()
        }
        "set_hf_token" => {
            app.set_hf_token(&arg::<Token>(&args)?.token);
            ok()
        }
        "licenses" => Ok(json!(crate::licenses::all())),
        _ => Err(UserError::internal(format!("unknown command {cmd}"))),
    }
}
