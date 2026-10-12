//! Fallback reasoning splitter for raw `<think>…</think>` tags.
//!
//! llama-server runs with `--reasoning-format deepseek`, so for every model whose
//! chat template it understands, reasoning arrives in `reasoning_content` and this
//! module sees nothing. Some templates leave the tags in `content` instead. This
//! splitter catches them, including when a tag is cut across two stream chunks
//! (`"<thi"` + `"nk>"`), and the case where the template opens the thought in the
//! prompt so the output starts mid-thought and only shows the closing tag.

const OPEN: &str = "<think>";
const CLOSE: &str = "</think>";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Piece {
    Reasoning(String),
    Answer(String),
    /// Everything emitted as answer so far was in fact reasoning: a closing tag
    /// arrived with no opening tag. The receiver moves its answer text into the
    /// reasoning block.
    AnswerWasReasoning,
}

#[derive(Debug, Default)]
pub struct ThinkSplitter {
    buf: String,
    in_think: bool,
    seen_open: bool,
    seen_close: bool,
    emitted_answer: bool,
}

impl ThinkSplitter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, chunk: &str) -> Vec<Piece> {
        self.buf.push_str(chunk);
        let mut out = Vec::new();
        loop {
            let tag = if self.in_think { CLOSE } else { OPEN };
            if let Some(i) = self.buf.find(tag) {
                let before: String = self.buf[..i].to_string();
                self.buf.drain(..i + tag.len());
                self.emit(&mut out, before);
                if self.in_think {
                    self.in_think = false;
                    self.seen_close = true;
                } else {
                    self.in_think = true;
                    self.seen_open = true;
                }
                continue;
            }
            // A closing tag with no opening tag: the prompt opened the thought.
            if !self.in_think && !self.seen_open && !self.seen_close {
                if let Some(i) = self.buf.find(CLOSE) {
                    let before: String = self.buf[..i].to_string();
                    self.buf.drain(..i + CLOSE.len());
                    if self.emitted_answer {
                        out.push(Piece::AnswerWasReasoning);
                        self.emitted_answer = false;
                    }
                    if !before.is_empty() {
                        out.push(Piece::Reasoning(before));
                    }
                    self.seen_close = true;
                    continue;
                }
            }
            // Keep back any tail that could be the start of a tag.
            let keep = partial_tag_suffix(&self.buf, self.in_think, !self.seen_open && !self.seen_close);
            let flush_to = self.buf.len() - keep;
            if flush_to > 0 {
                let text: String = self.buf[..flush_to].to_string();
                self.buf.drain(..flush_to);
                self.emit(&mut out, text);
            }
            break;
        }
        out
    }

    /// End of stream: whatever is buffered is plain text.
    pub fn finish(&mut self) -> Vec<Piece> {
        let mut out = Vec::new();
        let rest = std::mem::take(&mut self.buf);
        self.emit(&mut out, rest);
        out
    }

    fn emit(&mut self, out: &mut Vec<Piece>, text: String) {
        if text.is_empty() {
            return;
        }
        if self.in_think {
            out.push(Piece::Reasoning(text));
        } else {
            self.emitted_answer = true;
            out.push(Piece::Answer(text));
        }
    }
}

/// Length of the longest suffix of `s` that is a proper prefix of a tag we may see next.
fn partial_tag_suffix(s: &str, in_think: bool, close_possible: bool) -> usize {
    let mut tags: Vec<&str> = vec![if in_think { CLOSE } else { OPEN }];
    if !in_think && close_possible {
        tags.push(CLOSE);
    }
    let mut best = 0;
    for tag in tags {
        for n in (1..tag.len()).rev() {
            if n <= s.len() && s.is_char_boundary(s.len() - n) && s.ends_with(&tag[..n]) {
                best = best.max(n);
                break;
            }
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feed chunks, merge adjacent pieces of the same kind, return (reasoning, answer, retro).
    fn run(chunks: &[&str]) -> (String, String, bool) {
        let mut s = ThinkSplitter::new();
        let mut reasoning = String::new();
        let mut answer = String::new();
        let mut retro = false;
        let mut pieces = Vec::new();
        for c in chunks {
            pieces.extend(s.push(c));
        }
        pieces.extend(s.finish());
        for p in pieces {
            match p {
                Piece::Reasoning(t) => reasoning.push_str(&t),
                Piece::Answer(t) => answer.push_str(&t),
                Piece::AnswerWasReasoning => {
                    retro = true;
                    reasoning = std::mem::take(&mut answer) + &reasoning;
                }
            }
        }
        (reasoning, answer, retro)
    }

    #[test]
    fn plain_text_passes_through() {
        assert_eq!(run(&["Hello", " world"]), ("".into(), "Hello world".into(), false));
    }

    #[test]
    fn whole_tags_in_one_chunk() {
        assert_eq!(run(&["<think>hmm</think>Yes."]), ("hmm".into(), "Yes.".into(), false));
    }

    #[test]
    fn tags_split_at_every_position() {
        let text = "<think>a b</think>answer";
        for cut in 1..text.len() {
            let (r, a, _) = run(&[&text[..cut], &text[cut..]]);
            assert_eq!((r.as_str(), a.as_str()), ("a b", "answer"), "cut at {cut}");
        }
    }

    #[test]
    fn one_char_at_a_time() {
        let text = "<think>step 1\nstep 2</think>Final: 5";
        let chars: Vec<String> = text.chars().map(|c| c.to_string()).collect();
        let refs: Vec<&str> = chars.iter().map(|s| s.as_str()).collect();
        assert_eq!(run(&refs), ("step 1\nstep 2".into(), "Final: 5".into(), false));
    }

    #[test]
    fn closing_tag_only_means_prompt_opened_the_thought() {
        let (r, a, retro) = run(&["Let me think", " about it.</thi", "nk>The answer is 4."]);
        assert_eq!(r, "Let me think about it.");
        assert_eq!(a, "The answer is 4.");
        assert!(retro);
    }

    #[test]
    fn unterminated_thought_stays_reasoning() {
        assert_eq!(run(&["<think>still going"]), ("still going".into(), "".into(), false));
    }

    #[test]
    fn lookalike_text_is_not_held_back_forever() {
        assert_eq!(run(&["a < b and <th", "ings>"]), ("".into(), "a < b and <things>".into(), false));
    }

    #[test]
    fn multibyte_text_near_a_tag_boundary() {
        assert_eq!(run(&["<think>é</th", "ink>ñ"]), ("é".into(), "ñ".into(), false));
    }

    #[test]
    fn literal_tag_after_answer_is_text_once_closed() {
        // After a full think block, a later "<think>" in the answer opens a new block;
        // that matches how the models emit it and keeps the splitter simple.
        let (r, a, _) = run(&["<think>x</think>ok"]);
        assert_eq!((r.as_str(), a.as_str()), ("x", "ok"));
    }
}
