//! Text helpers for the terminal: word wrapping by display width, a small
//! Markdown renderer (headings, bold, italics, code, lists, code blocks), and a
//! multi-line input editor.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

/// Wrap styled spans to `width` columns, prefixing every line with `prefix`.
pub fn wrap(spans: Vec<Span<'static>>, width: usize, prefix: Span<'static>) -> Vec<Line<'static>> {
    let avail = width.saturating_sub(prefix.content.width()).max(8);
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut cur: Vec<Span<'static>> = vec![prefix.clone()];
    let mut cur_w = 0usize;
    let flush =
        |cur: &mut Vec<Span<'static>>, cur_w: &mut usize, lines: &mut Vec<Line<'static>>| {
            lines.push(Line::from(std::mem::replace(cur, vec![prefix.clone()])));
            *cur_w = 0;
        };
    for span in spans {
        let style = span.style;
        // Split into words keeping the spaces attached, so wrapping happens at spaces.
        let mut word = String::new();
        let mut pieces: Vec<String> = Vec::new();
        for ch in span.content.chars() {
            word.push(ch);
            if ch == ' ' {
                pieces.push(std::mem::take(&mut word));
            }
        }
        if !word.is_empty() {
            pieces.push(word);
        }
        for p in pieces {
            let w = p.width();
            if cur_w + w.saturating_sub(trailing_spaces(&p)) > avail && cur_w > 0 {
                flush(&mut cur, &mut cur_w, &mut lines);
            }
            if w > avail {
                // A single very long word (a URL): hard-break it.
                let mut chunk = String::new();
                for ch in p.chars() {
                    let cw = UnicodeWidthStr::width(ch.encode_utf8(&mut [0; 4]) as &str);
                    if cur_w + chunk.width() + cw > avail {
                        cur.push(Span::styled(std::mem::take(&mut chunk), style));
                        flush(&mut cur, &mut cur_w, &mut lines);
                    }
                    chunk.push(ch);
                }
                cur_w += chunk.width();
                cur.push(Span::styled(chunk, style));
                continue;
            }
            let p = if cur_w == 0 {
                p.trim_start().to_string()
            } else {
                p
            };
            cur_w += p.width();
            cur.push(Span::styled(p, style));
        }
    }
    if cur.len() > 1 || lines.is_empty() {
        lines.push(Line::from(cur));
    }
    lines
}

fn trailing_spaces(s: &str) -> usize {
    s.len() - s.trim_end_matches(' ').len()
}

/// Plain text (keeps line breaks) wrapped with one style.
pub fn wrap_plain(
    text: &str,
    width: usize,
    style: Style,
    prefix: Span<'static>,
) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    for l in text.split('\n') {
        out.extend(wrap(
            vec![Span::styled(l.to_string(), style)],
            width,
            prefix.clone(),
        ));
    }
    out
}

/// Inline Markdown: **bold**, *italic* / _italic_, `code`.
pub fn inline(text: &str, base: Style) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut buf = String::new();
    let (mut bold, mut italic, mut code) = (false, false, false);
    let style = |b: bool, i: bool, c: bool| {
        let mut s = base;
        if b {
            s = s.add_modifier(Modifier::BOLD);
        }
        if i {
            s = s.add_modifier(Modifier::ITALIC);
        }
        if c {
            s = s.fg(Color::Yellow);
        }
        s
    };
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '`' {
            spans.push(Span::styled(
                std::mem::take(&mut buf),
                style(bold, italic, code),
            ));
            code = !code;
            i += 1;
            continue;
        }
        if !code && c == '*' && chars.get(i + 1) == Some(&'*') {
            spans.push(Span::styled(
                std::mem::take(&mut buf),
                style(bold, italic, code),
            ));
            bold = !bold;
            i += 2;
            continue;
        }
        let word_edge = |j: usize| j == 0 || !chars[j - 1].is_alphanumeric();
        if !code
            && (c == '*' || (c == '_' && word_edge(i)))
            && chars
                .get(i + 1)
                .map(|n| !n.is_whitespace())
                .unwrap_or(false)
            && !italic
        {
            spans.push(Span::styled(
                std::mem::take(&mut buf),
                style(bold, italic, code),
            ));
            italic = true;
            i += 1;
            continue;
        }
        if !code && italic && (c == '*' || c == '_') {
            spans.push(Span::styled(
                std::mem::take(&mut buf),
                style(bold, italic, code),
            ));
            italic = false;
            i += 1;
            continue;
        }
        buf.push(c);
        i += 1;
    }
    spans.push(Span::styled(buf, style(bold, italic, code)));
    spans.retain(|s| !s.content.is_empty());
    spans
}

/// Block-level Markdown to wrapped lines.
pub fn markdown(text: &str, width: usize, base: Style) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    let mut in_code = false;
    for raw in text.split('\n') {
        let line = raw.trim_end();
        if line.trim_start().starts_with("```") {
            in_code = !in_code;
            continue;
        }
        if in_code {
            out.extend(wrap(
                vec![Span::styled(
                    line.to_string(),
                    Style::default().fg(Color::Yellow),
                )],
                width,
                Span::raw("  "),
            ));
            continue;
        }
        let t = line.trim_start();
        if let Some(h) = t
            .strip_prefix("### ")
            .or_else(|| t.strip_prefix("## "))
            .or_else(|| t.strip_prefix("# "))
        {
            out.extend(wrap(
                inline(h, base.add_modifier(Modifier::BOLD)),
                width,
                Span::raw(""),
            ));
            continue;
        }
        let indent = (line.len() - t.len()) / 2;
        let pad = "  ".repeat(indent);
        if let Some(item) = t
            .strip_prefix("- ")
            .or_else(|| t.strip_prefix("* "))
            .or_else(|| t.strip_prefix("+ "))
        {
            let first = wrap(
                inline(item, base),
                width.saturating_sub(2),
                Span::raw(format!("{pad}• ")),
            );
            for (k, l) in first.into_iter().enumerate() {
                if k == 0 {
                    out.push(l);
                } else {
                    let mut spans = vec![Span::raw(format!("{pad}  "))];
                    spans.extend(l.spans.into_iter().skip(1));
                    out.push(Line::from(spans));
                }
            }
            continue;
        }
        if let Some((num, rest)) = t.split_once(". ") {
            if !num.is_empty() && num.len() <= 3 && num.chars().all(|c| c.is_ascii_digit()) {
                out.extend(wrap(
                    inline(rest, base),
                    width,
                    Span::raw(format!("{pad}{num}. ")),
                ));
                continue;
            }
        }
        if t.starts_with('|') {
            out.push(Line::from(Span::styled(line.to_string(), base)));
            continue;
        }
        if t.is_empty() {
            out.push(Line::from(""));
            continue;
        }
        out.extend(wrap(inline(t, base), width, Span::raw(pad)));
    }
    out
}

/// A small multi-line text editor (the chat box, the system prompt, search fields).
#[derive(Debug, Default, Clone)]
pub struct Editor {
    pub text: String,
    /// Byte offset of the cursor, always on a char boundary.
    pub cursor: usize,
}

impl Editor {
    pub fn with(text: &str) -> Self {
        Self {
            text: text.to_string(),
            cursor: text.len(),
        }
    }
    pub fn insert(&mut self, c: char) {
        self.text.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }
    pub fn insert_str(&mut self, s: &str) {
        let s = s.replace("\r\n", "\n").replace('\r', "\n");
        self.text.insert_str(self.cursor, &s);
        self.cursor += s.len();
    }
    pub fn backspace(&mut self) {
        if let Some(c) = self.text[..self.cursor].chars().next_back() {
            self.cursor -= c.len_utf8();
            self.text.remove(self.cursor);
        }
    }
    pub fn delete(&mut self) {
        if self.cursor < self.text.len() {
            self.text.remove(self.cursor);
        }
    }
    pub fn left(&mut self) {
        if let Some(c) = self.text[..self.cursor].chars().next_back() {
            self.cursor -= c.len_utf8();
        }
    }
    pub fn right(&mut self) {
        if let Some(c) = self.text[self.cursor..].chars().next() {
            self.cursor += c.len_utf8();
        }
    }
    pub fn home(&mut self) {
        self.cursor = self.text[..self.cursor]
            .rfind('\n')
            .map(|i| i + 1)
            .unwrap_or(0);
    }
    pub fn end(&mut self) {
        self.cursor = self.text[self.cursor..]
            .find('\n')
            .map(|i| self.cursor + i)
            .unwrap_or(self.text.len());
    }
    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
    }
    pub fn take(&mut self) -> String {
        self.cursor = 0;
        std::mem::take(&mut self.text)
    }

    /// Lines to draw and the (column, row) of the cursor, wrapping at `width`.
    pub fn layout(&self, width: usize) -> (Vec<String>, (u16, u16)) {
        let width = width.max(4);
        let mut rows: Vec<String> = vec![String::new()];
        let mut cur = (0u16, 0u16);
        let mut col = 0usize;
        for (i, ch) in self.text.char_indices() {
            if i == self.cursor {
                cur = (col as u16, (rows.len() - 1) as u16);
            }
            if ch == '\n' {
                rows.push(String::new());
                col = 0;
                continue;
            }
            let w = UnicodeWidthStr::width(ch.encode_utf8(&mut [0; 4]) as &str);
            if col + w > width {
                rows.push(String::new());
                col = 0;
            }
            rows.last_mut().unwrap().push(ch);
            col += w;
        }
        if self.cursor >= self.text.len() {
            if col >= width {
                rows.push(String::new());
                col = 0;
            }
            cur = (col as u16, (rows.len() - 1) as u16);
        }
        (rows, cur)
    }
}

pub fn human_size(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= 1e9 {
        format!("{:.1} GB", b / 1e9)
    } else if b >= 1e6 {
        format!("{:.0} MB", b / 1e6)
    } else if b >= 1e3 {
        format!("{:.0} KB", b / 1e3)
    } else {
        format!("{bytes} B")
    }
}

pub fn human_duration(ms: u64) -> String {
    let s = (ms as f64 / 1000.0).round() as u64;
    if s < 60 {
        format!("{}s", s.max(1))
    } else {
        format!("{} min {} s", s / 60, s % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(lines: &[Line]) -> Vec<String> {
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn wraps_at_word_boundaries() {
        let l = wrap(
            vec![Span::raw("the quick brown fox jumps over")],
            12,
            Span::raw(""),
        );
        assert_eq!(text(&l), vec!["the quick ", "brown fox ", "jumps over"]);
    }

    #[test]
    fn hard_breaks_long_words_and_keeps_prefix() {
        let l = wrap(
            vec![Span::raw("https://huggingface.co/abcdefghij")],
            20,
            Span::raw("│ "),
        );
        assert!(l.iter().all(|x| x.spans[0].content == "│ "));
        assert!(text(&l).iter().all(|s| s.width() <= 20));
        assert_eq!(
            text(&l).concat().replace("│ ", ""),
            "https://huggingface.co/abcdefghij"
        );
    }

    #[test]
    fn wide_characters_count_as_two() {
        let l = wrap(vec![Span::raw("日本語 日本語 日本語")], 8, Span::raw(""));
        assert!(text(&l).iter().all(|s| s.trim_end().width() <= 8));
    }

    #[test]
    fn inline_markdown() {
        let s = inline("a **b** `c` *d* snake_case", Style::default());
        let parts: Vec<(&str, bool, bool)> = s
            .iter()
            .map(|x| {
                (
                    x.content.as_ref(),
                    x.style.add_modifier.contains(Modifier::BOLD),
                    x.style.add_modifier.contains(Modifier::ITALIC),
                )
            })
            .collect();
        assert!(parts.contains(&("b", true, false)));
        assert!(parts.contains(&("d", false, true)));
        assert_eq!(
            s.iter().map(|x| x.content.as_ref()).collect::<String>(),
            "a b c d snake_case"
        );
    }

    #[test]
    fn block_markdown() {
        let md = "# Title\n\n- one\n- two\n\n1. first\n\n```\ncode here\n```";
        let t = text(&markdown(md, 40, Style::default()));
        assert_eq!(
            t,
            vec![
                "Title",
                "",
                "• one",
                "• two",
                "",
                "1. first",
                "",
                "  code here"
            ]
        );
    }

    #[test]
    fn editor_edits_and_lays_out() {
        let mut e = Editor::default();
        for c in "héllo".chars() {
            e.insert(c);
        }
        e.left();
        e.backspace();
        assert_eq!(e.text, "hélo");
        e.home();
        e.insert('>');
        assert_eq!(e.text, ">hélo");
        e.end();
        e.insert_str("\r\nnext");
        assert_eq!(e.text, ">hélo\nnext");
        let (rows, cur) = e.layout(10);
        assert_eq!(rows, vec![">hélo", "next"]);
        assert_eq!(cur, (4, 1));
        assert_eq!(e.take(), ">hélo\nnext");
        assert_eq!((e.text.as_str(), e.cursor), ("", 0));
    }

    #[test]
    fn sizes_and_durations() {
        assert_eq!(human_size(806_058_240), "806 MB");
        assert_eq!(human_size(2_489_757_856), "2.5 GB");
        assert_eq!(human_duration(12_400), "12s");
        assert_eq!(human_duration(65_000), "1 min 5 s");
        assert_eq!(human_duration(100), "1s");
    }
}
