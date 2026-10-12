//! Incremental parser for llama-server's OpenAI-style server-sent events.
//!
//! Bytes arrive in arbitrary chunks: a JSON line can be cut anywhere, including
//! inside a multi-byte character. The parser buffers raw bytes and only decodes
//! complete lines.

use serde::Deserialize;

#[derive(Debug, Clone, PartialEq)]
pub enum SseItem {
    Chunk(ChatChunk),
    Done,
    /// The server put an error object in the stream.
    Error(ServerError),
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ChatChunk {
    #[serde(default)]
    pub choices: Vec<Choice>,
    #[serde(default)]
    pub timings: Option<Timings>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Choice {
    #[serde(default)]
    pub delta: Delta,
    #[serde(default)]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct Delta {
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub reasoning_content: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Timings {
    #[serde(default)]
    pub predicted_n: Option<u64>,
    #[serde(default)]
    pub predicted_per_second: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ServerError {
    #[serde(default)]
    pub code: Option<i64>,
    #[serde(default)]
    pub message: String,
    #[serde(default, rename = "type")]
    pub kind: Option<String>,
}

#[derive(Deserialize)]
struct ErrorEnvelope {
    error: ServerError,
}

#[derive(Debug, Default)]
pub struct SseParser {
    buf: Vec<u8>,
}

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, bytes: &[u8]) -> Vec<SseItem> {
        self.buf.extend_from_slice(bytes);
        let mut out = Vec::new();
        while let Some(nl) = self.buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=nl).collect();
            if let Some(item) = parse_line(&line) {
                out.push(item);
            }
        }
        out
    }

    /// The stream ended: a last line without a newline still counts.
    pub fn finish(&mut self) -> Vec<SseItem> {
        let line = std::mem::take(&mut self.buf);
        parse_line(&line).into_iter().collect()
    }
}

fn parse_line(raw: &[u8]) -> Option<SseItem> {
    let line = String::from_utf8_lossy(raw);
    let line = line.trim_end_matches(['\r', '\n']);
    let data = line.strip_prefix("data:")?.trim_start();
    if data == "[DONE]" {
        return Some(SseItem::Done);
    }
    if let Ok(env) = serde_json::from_str::<ErrorEnvelope>(data) {
        return Some(SseItem::Error(env.error));
    }
    serde_json::from_str::<ChatChunk>(data)
        .ok()
        .map(SseItem::Chunk)
}

/// Parse a non-streamed error body (llama-server answers 4xx with JSON even when
/// the request asked for a stream).
pub fn parse_error_body(body: &str) -> Option<ServerError> {
    serde_json::from_str::<ErrorEnvelope>(body)
        .ok()
        .map(|e| e.error)
}

#[cfg(test)]
mod tests {
    use super::*;

    const REAL: &str = concat!(
        "data: {\"choices\":[{\"finish_reason\":null,\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":null}}],\"object\":\"chat.completion.chunk\"}\n\n",
        "data: {\"choices\":[{\"finish_reason\":null,\"index\":0,\"delta\":{\"reasoning_content\":\"Okay\"}}],\"object\":\"chat.completion.chunk\"}\n\n",
        "data: {\"choices\":[{\"finish_reason\":null,\"index\":0,\"delta\":{\"content\":\"5\"}}],\"object\":\"chat.completion.chunk\"}\n\n",
        "data: {\"choices\":[{\"finish_reason\":\"stop\",\"index\":0,\"delta\":{}}],\"timings\":{\"predicted_n\":16,\"predicted_per_second\":20.5}}\n\n",
        "data: [DONE]\n\n",
    );

    fn kinds(items: &[SseItem]) -> Vec<String> {
        items
            .iter()
            .map(|i| match i {
                SseItem::Done => "done".to_string(),
                SseItem::Error(e) => format!("error:{}", e.message),
                SseItem::Chunk(c) => {
                    let ch = &c.choices[0];
                    format!(
                        "r={:?} c={:?} f={:?}",
                        ch.delta.reasoning_content, ch.delta.content, ch.finish_reason
                    )
                }
            })
            .collect()
    }

    #[test]
    fn parses_a_real_llama_server_stream() {
        let mut p = SseParser::new();
        let items = p.push(REAL.as_bytes());
        assert_eq!(
            kinds(&items),
            vec![
                "r=None c=None f=None",
                "r=Some(\"Okay\") c=None f=None",
                "r=None c=Some(\"5\") f=None",
                "r=None c=None f=Some(\"stop\")",
                "done",
            ]
        );
        match &items[3] {
            SseItem::Chunk(c) => assert_eq!(c.timings.as_ref().unwrap().predicted_n, Some(16)),
            _ => panic!(),
        }
    }

    #[test]
    fn same_result_for_every_split_point() {
        let whole = kinds(&SseParser::new().push(REAL.as_bytes()));
        let bytes = REAL.as_bytes();
        for cut in 1..bytes.len() {
            let mut p = SseParser::new();
            let mut items = p.push(&bytes[..cut]);
            items.extend(p.push(&bytes[cut..]));
            items.extend(p.finish());
            assert_eq!(kinds(&items), whole, "cut at {cut}");
        }
    }

    #[test]
    fn multibyte_character_split_across_chunks() {
        let s = "data: {\"choices\":[{\"delta\":{\"content\":\"é😀\"}}]}\n";
        let b = s.as_bytes();
        let i = s.find('é').unwrap() + 1; // inside the 2-byte é
        let mut p = SseParser::new();
        let mut items = p.push(&b[..i]);
        items.extend(p.push(&b[i..]));
        assert_eq!(kinds(&items), vec!["r=None c=Some(\"é😀\") f=None"]);
    }

    #[test]
    fn crlf_and_missing_final_newline() {
        let mut p = SseParser::new();
        let mut items =
            p.push(b"data: {\"choices\":[{\"delta\":{\"content\":\"a\"}}]}\r\n\r\ndata: [DONE]");
        items.extend(p.finish());
        assert_eq!(kinds(&items), vec!["r=None c=Some(\"a\") f=None", "done"]);
    }

    #[test]
    fn error_object_in_stream() {
        let mut p = SseParser::new();
        let items = p.push(
            b"data: {\"error\":{\"code\":500,\"message\":\"boom\",\"type\":\"server_error\"}}\n",
        );
        assert_eq!(kinds(&items), vec!["error:boom"]);
    }

    #[test]
    fn non_stream_error_body_from_llama_server() {
        let body = r#"{"error":{"code":400,"message":"request (609 tokens) exceeds the available context size (256 tokens), try increasing it","type":"exceed_context_size_error","n_prompt_tokens":609,"n_ctx":256}}"#;
        let e = parse_error_body(body).unwrap();
        assert_eq!(e.kind.as_deref(), Some("exceed_context_size_error"));
        assert_eq!(e.code, Some(400));
    }

    #[test]
    fn ignores_comments_and_garbage() {
        let mut p = SseParser::new();
        assert!(
            p.push(b": keep-alive\n\nevent: x\nnot json\ndata: {not json}\n")
                .is_empty()
        );
    }
}
