//! Conversations and streaming.
//!
//! What goes back to the model is exactly what the learner can see: the system
//! prompt box, then each finished question and answer. Reasoning is shown but
//! never sent back. A turn that produced no answer (stopped too early, or
//! failed) is left out together with its question, so the history always
//! alternates user/assistant: Gemma's chat template refuses anything else.

use crate::errors::UserError;
use crate::sse::{SseItem, SseParser, parse_error_body};
use crate::think::{Piece, ThinkSplitter};
use futures_util::StreamExt;
use serde::Serialize;
use std::time::Instant;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnStatus {
    Streaming,
    Done,
    Stopped,
    /// The context filled up while answering.
    Cut,
    Error,
}

#[derive(Debug, Clone, Serialize)]
pub struct Turn {
    pub id: u64,
    pub user: String,
    pub reasoning: String,
    pub answer: String,
    /// Display name of the model that answered.
    pub model: String,
    pub status: TurnStatus,
    /// Time from the first reasoning token to the first answer token.
    pub thought_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<UserError>,
}

impl Turn {
    /// Does this turn go back to the model next time?
    pub fn in_history(&self) -> bool {
        matches!(
            self.status,
            TurnStatus::Done | TurnStatus::Stopped | TurnStatus::Cut
        ) && !self.answer.trim().is_empty()
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Message {
    pub role: &'static str,
    pub content: String,
}

pub fn history(system_prompt: &str, turns: &[Turn], new_user: &str) -> Vec<Message> {
    let mut m = Vec::new();
    if !system_prompt.trim().is_empty() {
        m.push(Message {
            role: "system",
            content: system_prompt.to_string(),
        });
    }
    for t in turns.iter().filter(|t| t.in_history()) {
        m.push(Message {
            role: "user",
            content: t.user.clone(),
        });
        m.push(Message {
            role: "assistant",
            content: t.answer.clone(),
        });
    }
    m.push(Message {
        role: "user",
        content: new_user.to_string(),
    });
    m
}

/// What arrives while a turn streams.
#[derive(Debug, Clone, PartialEq)]
pub enum StreamDelta {
    Reasoning(String),
    Answer(String),
    /// Text so far shown as answer was reasoning (raw `</think>` with no `<think>`).
    AnswerWasReasoning,
    /// First answer token after some reasoning: how long the model thought.
    ThoughtFor(u64),
}

#[derive(Debug, Clone, PartialEq)]
pub enum StreamEnd {
    Done,
    /// The context window filled while answering.
    Length,
    Stopped,
    Failed(UserError),
    /// The connection to llama-server broke: it probably crashed.
    ServerGone(String),
}

/// Keeps the learner-visible text tidy: models often start with "\n\n".
#[derive(Default)]
struct Trim {
    reasoning_started: bool,
    answer_started: bool,
}

impl Trim {
    fn reasoning(&mut self, s: String) -> Option<String> {
        if self.reasoning_started {
            return Some(s);
        }
        let t = s.trim_start().to_string();
        if t.is_empty() {
            None
        } else {
            self.reasoning_started = true;
            Some(t)
        }
    }
    fn answer(&mut self, s: String) -> Option<String> {
        if self.answer_started {
            return Some(s);
        }
        let t = s.trim_start().to_string();
        if t.is_empty() {
            None
        } else {
            self.answer_started = true;
            Some(t)
        }
    }
}

pub struct ChatRequest<'a> {
    pub base_url: &'a str,
    pub api_key: &'a str,
    pub messages: Vec<Message>,
    pub temperature: f32,
}

/// Stream one answer. `on` is called for every visible change.
pub async fn stream(
    http: &reqwest::Client,
    req: ChatRequest<'_>,
    cancel: &CancellationToken,
    mut on: impl FnMut(StreamDelta),
) -> StreamEnd {
    let body = serde_json::json!({
        "messages": req.messages,
        "stream": true,
        "temperature": req.temperature,
        "cache_prompt": true,
    });
    let send = http
        .post(format!("{}/v1/chat/completions", req.base_url))
        .bearer_auth(req.api_key)
        .json(&body)
        .send();
    let resp = tokio::select! {
        r = send => r,
        _ = cancel.cancelled() => return StreamEnd::Stopped,
    };
    let resp = match resp {
        Ok(r) => r,
        Err(e) => return StreamEnd::ServerGone(e.to_string()),
    };
    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        return StreamEnd::Failed(error_from_body(status, &text));
    }

    let mut parser = SseParser::new();
    let mut splitter = ThinkSplitter::new();
    let mut trim = Trim::default();
    let mut first_reasoning: Option<Instant> = None;
    let mut answered = false;
    let mut bytes = resp.bytes_stream();

    let mut handle = |items: Vec<SseItem>, on: &mut dyn FnMut(StreamDelta)| -> Option<StreamEnd> {
        for item in items {
            match item {
                SseItem::Done => return Some(StreamEnd::Done),
                SseItem::Error(e) => {
                    return Some(StreamEnd::Failed(error_from_server(
                        e.kind.as_deref(),
                        &e.message,
                    )));
                }
                SseItem::Chunk(c) => {
                    let Some(ch) = c.choices.into_iter().next() else {
                        continue;
                    };
                    let mut pieces: Vec<Piece> = Vec::new();
                    if let Some(r) = ch.delta.reasoning_content.filter(|s| !s.is_empty()) {
                        pieces.push(Piece::Reasoning(r));
                    }
                    if let Some(t) = ch.delta.content.filter(|s| !s.is_empty()) {
                        pieces.extend(splitter.push(&t));
                    }
                    if ch.finish_reason.is_some() {
                        pieces.extend(splitter.finish());
                    }
                    for p in pieces {
                        match p {
                            Piece::Reasoning(r) => {
                                first_reasoning.get_or_insert_with(Instant::now);
                                if let Some(r) = trim.reasoning(r) {
                                    on(StreamDelta::Reasoning(r));
                                }
                            }
                            Piece::Answer(a) => {
                                if let Some(a) = trim.answer(a) {
                                    if !answered {
                                        answered = true;
                                        if let Some(t0) = first_reasoning {
                                            on(StreamDelta::ThoughtFor(
                                                t0.elapsed().as_millis() as u64
                                            ));
                                        }
                                    }
                                    on(StreamDelta::Answer(a));
                                }
                            }
                            Piece::AnswerWasReasoning => {
                                answered = false;
                                trim.answer_started = false;
                                trim.reasoning_started = true;
                                first_reasoning.get_or_insert_with(Instant::now);
                                on(StreamDelta::AnswerWasReasoning);
                            }
                        }
                    }
                    match ch.finish_reason.as_deref() {
                        Some("length") => return Some(StreamEnd::Length),
                        Some(_) => return Some(StreamEnd::Done),
                        None => {}
                    }
                }
            }
        }
        None
    };

    loop {
        let next = tokio::select! {
            n = bytes.next() => n,
            // Dropping the response closes the connection; llama-server then stops generating.
            _ = cancel.cancelled() => return StreamEnd::Stopped,
        };
        match next {
            Some(Ok(b)) => {
                if let Some(end) = handle(parser.push(&b), &mut on) {
                    return end;
                }
            }
            Some(Err(e)) => return StreamEnd::ServerGone(e.to_string()),
            None => {
                let items = parser.finish();
                return handle(items, &mut on)
                    .unwrap_or(StreamEnd::ServerGone("stream ended early".into()));
            }
        }
    }
}

fn error_from_body(status: u16, body: &str) -> UserError {
    match parse_error_body(body) {
        Some(e) => error_from_server(e.kind.as_deref(), &e.message),
        None if status == 503 => UserError::server_start().with_detail(body.to_string()),
        None => UserError::internal(format!("HTTP {status}: {body}")),
    }
}

fn error_from_server(kind: Option<&str>, message: &str) -> UserError {
    let m = message.to_ascii_lowercase();
    if kind == Some("exceed_context_size_error")
        || m.contains("exceeds the available context size")
        || m.contains("context size") && m.contains("exceed")
    {
        UserError::context_full().with_detail(message.to_string())
    } else if kind == Some("unavailable_error") || m.contains("loading model") {
        UserError::server_start().with_detail(message.to_string())
    } else {
        UserError::internal(message.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(user: &str, answer: &str, status: TurnStatus) -> Turn {
        Turn {
            id: 0,
            user: user.into(),
            reasoning: "secret thoughts".into(),
            answer: answer.into(),
            model: "m".into(),
            status,
            thought_ms: None,
            error: None,
        }
    }

    #[test]
    fn history_sends_answers_only_never_reasoning() {
        let turns = vec![turn("Q1", "A1", TurnStatus::Done)];
        let h = history("Be brief.", &turns, "Q2");
        let roles: Vec<&str> = h.iter().map(|m| m.role).collect();
        assert_eq!(roles, vec!["system", "user", "assistant", "user"]);
        assert!(h.iter().all(|m| !m.content.contains("secret")));
        assert_eq!(h[2].content, "A1");
    }

    #[test]
    fn empty_system_prompt_sends_nothing() {
        let h = history("   ", &[], "Hi");
        assert_eq!(
            h,
            vec![Message {
                role: "user",
                content: "Hi".into()
            }]
        );
    }

    #[test]
    fn turns_without_an_answer_are_dropped_with_their_question() {
        let turns = vec![
            turn("Q1", "A1", TurnStatus::Done),
            turn("Q2", "", TurnStatus::Stopped),
            turn("Q3", "", TurnStatus::Error),
            turn("Q4", "partial", TurnStatus::Stopped),
            turn("Q5", "cut off", TurnStatus::Cut),
        ];
        let h = history("", &turns, "Q6");
        let users: Vec<&str> = h
            .iter()
            .filter(|m| m.role == "user")
            .map(|m| m.content.as_str())
            .collect();
        assert_eq!(users, vec!["Q1", "Q4", "Q5", "Q6"]);
        // Always alternates.
        for w in h.windows(2) {
            assert_ne!(w[0].role, w[1].role);
        }
    }

    #[test]
    fn context_errors_map_to_the_course_message() {
        let body = r#"{"error":{"code":400,"message":"request (609 tokens) exceeds the available context size (256 tokens), try increasing it","type":"exceed_context_size_error"}}"#;
        assert_eq!(
            error_from_body(400, body).code,
            crate::errors::ErrorCode::ContextFull
        );
        assert_eq!(
            error_from_body(500, "not json").code,
            crate::errors::ErrorCode::Internal
        );
    }
}
