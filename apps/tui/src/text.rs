//! Text helpers for the terminal: word wrapping by display width, a small
//! Markdown renderer (headings, bold, italics, code, lists, code blocks), and a
//! multi-line input editor.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

/// Wrap styled spans to `width` columns, prefixing every line with `prefix`.
/// Math written by models in LaTeX ($\frac{1}{2}$, \boxed{42}), as plain text a
/// learner can read: 1/2, 42. Same rules as apps/desktop/ui/plainmath.js; both
/// are checked against contracts/plainmath-cases.json. Code is never touched.
pub fn plain_math(text: &str) -> String {
    if !text.contains('$') && !text.contains('\\') {
        return text.to_string();
    }
    // Split out code (``` blocks and `spans`), convert only the rest.
    let mut out = String::new();
    let mut rest = text;
    while !rest.is_empty() {
        let fence = rest.find("```");
        let tick = rest.find('`');
        match tick {
            None => {
                out.push_str(&math_prose(rest));
                break;
            }
            Some(t) => {
                out.push_str(&math_prose(&rest[..t]));
                let code_end = if fence == Some(t) {
                    rest[t + 3..]
                        .find("```")
                        .map_or(rest.len(), |e| t + 3 + e + 3)
                } else {
                    match rest[t + 1..].find(['`', '\n']) {
                        Some(e) if rest.as_bytes()[t + 1 + e] == b'`' => t + 1 + e + 1,
                        _ => t + 1,
                    }
                };
                out.push_str(&rest[t..code_end]);
                rest = &rest[code_end..];
            }
        }
    }
    out
}

fn math_prose(s: &str) -> String {
    let mut out = String::new();
    let b: Vec<char> = s.chars().collect();
    let mut i = 0;
    let text = |a: usize, z: usize| b[a..z].iter().collect::<String>();
    let find = |from: usize, pat: &str| {
        let p: Vec<char> = pat.chars().collect();
        (from..b.len().saturating_sub(p.len() - 1)).find(|&k| b[k..k + p.len()] == p[..])
    };
    while i < b.len() {
        // $$ … $$, \[ … \], \( … \)
        let pairs = [("$$", "$$"), ("\\[", "\\]"), ("\\(", "\\)")];
        let mut done = false;
        for (open, close) in pairs {
            let o: Vec<char> = open.chars().collect();
            if b[i..].starts_with(&o)
                && let Some(e) = find(i + o.len(), close)
            {
                out.push_str(&convert(&text(i + o.len(), e)));
                i = e + close.chars().count();
                done = true;
                break;
            }
        }
        if done {
            continue;
        }
        // Inline $…$: not money ("$5 and $10"): no space just inside, no digit right after.
        if b[i] == '$'
            && (i == 0 || !(b[i - 1].is_alphanumeric() || b[i - 1] == '\\' || b[i - 1] == '$'))
            && b.get(i + 1).is_some_and(|c| !c.is_whitespace())
            && let Some(e) = (i + 1..b.len())
                .find(|&k| b[k] == '$' || b[k] == '\n')
                .filter(|&k| b[k] == '$')
            && !b[e - 1].is_whitespace()
            && !b.get(e + 1).is_some_and(|c| c.is_ascii_digit())
        {
            out.push_str(&convert(&text(i + 1, e)));
            i = e + 1;
            continue;
        }
        // "$ B $" with spaces inside: math only if short and clearly math (a
        // command, or names of one or two letters), never "$ 5 and $ 10".
        if b[i] == '$'
            && (i == 0 || !(b[i - 1].is_alphanumeric() || b[i - 1] == '\\' || b[i - 1] == '$'))
            && b.get(i + 1) == Some(&' ')
            && let Some(e) = (i + 2..b.len().min(i + 44))
                .find(|&k| b[k] == '$' || b[k] == '\n')
                .filter(|&k| b[k] == '$')
            && e > i + 2
            && b[e - 1] == ' '
            && !b.get(e + 1).is_some_and(|c| c.is_ascii_digit())
        {
            let m = text(i + 2, e - 1);
            let words: Vec<&str> = m
                .split(|c: char| !c.is_ascii_alphabetic())
                .filter(|w| !w.is_empty())
                .collect();
            if m.contains('\\') || (!words.is_empty() && words.iter().all(|w| w.len() <= 2)) {
                out.push_str(&convert(&m));
                i = e + 1;
                continue;
            }
        }
        // The few commands models also write outside math.
        if b[i] == '\\' {
            let word: String = b[i + 1..]
                .iter()
                .take_while(|c| c.is_ascii_alphabetic())
                .collect();
            let after = i + 1 + word.len();
            if (word == "boxed" || word == "text" || word == "textbf") && b.get(after) == Some(&'{')
            {
                let (g, end) = group(&b, after);
                out.push_str(&convert(&g));
                i = end;
                continue;
            }
            if word == "quad" || word == "qquad" {
                out.push(' ');
                i = after;
                if b.get(i) == Some(&' ') {
                    i += 1;
                }
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

/// The content of the {group} at b[i] == '{', and the index after it.
fn group(b: &[char], i: usize) -> (String, usize) {
    let mut depth = 0;
    for (j, &c) in b.iter().enumerate().skip(i) {
        if c == '{' {
            depth += 1;
        } else if c == '}' {
            depth -= 1;
            if depth == 0 {
                return (b[i + 1..j].iter().collect(), j + 1);
            }
        }
    }
    (b[i + 1..].iter().collect(), b.len())
}

fn needs_parens(x: &str) -> bool {
    let t = x.trim();
    t.contains(['+', '-', '×', '÷', '·', '/', ' ']) && !(t.starts_with('(') && t.ends_with(')'))
}

fn sub(x: &str) -> String {
    let m = |c: char| {
        Some(match c {
            '0' => '₀',
            '1' => '₁',
            '2' => '₂',
            '3' => '₃',
            '4' => '₄',
            '5' => '₅',
            '6' => '₆',
            '7' => '₇',
            '8' => '₈',
            '9' => '₉',
            '+' => '₊',
            '-' => '₋',
            '=' => '₌',
            '(' => '₍',
            ')' => '₎',
            'a' => 'ₐ',
            'e' => 'ₑ',
            'o' => 'ₒ',
            'x' => 'ₓ',
            'h' => 'ₕ',
            'k' => 'ₖ',
            'l' => 'ₗ',
            'm' => 'ₘ',
            'n' => 'ₙ',
            'p' => 'ₚ',
            's' => 'ₛ',
            't' => 'ₜ',
            'i' => 'ᵢ',
            'j' => 'ⱼ',
            'r' => 'ᵣ',
            'u' => 'ᵤ',
            'v' => 'ᵥ',
            _ => return None,
        })
    };
    x.chars()
        .map(m)
        .collect::<Option<String>>()
        .unwrap_or_else(|| x.to_string())
}

/// A denominator like "2a" needs brackets too: 1/2a reads as half of a.
fn needs_parens_below(x: &str) -> bool {
    let t = x.trim();
    needs_parens(t) || (t.chars().count() > 1 && !t.chars().all(|c| c.is_ascii_digit() || c == '.'))
}

fn sup(x: &str) -> String {
    let m = |c: char| match c {
        '0' => Some('⁰'),
        '1' => Some('¹'),
        '2' => Some('²'),
        '3' => Some('³'),
        '4' => Some('⁴'),
        '5' => Some('⁵'),
        '6' => Some('⁶'),
        '7' => Some('⁷'),
        '8' => Some('⁸'),
        '9' => Some('⁹'),
        '+' => Some('⁺'),
        '-' => Some('⁻'),
        'n' => Some('ⁿ'),
        _ => None,
    };
    x.chars()
        .map(m)
        .collect::<Option<String>>()
        .unwrap_or_else(|| format!("^({x})"))
}

fn symbol(name: &str) -> Option<&'static str> {
    Some(match name {
        "times" => "×",
        "cdot" => "·",
        "div" => "÷",
        "pm" => "±",
        "mp" => "∓",
        "le" | "leq" => "≤",
        "ge" | "geq" => "≥",
        "neq" | "ne" => "≠",
        "approx" => "≈",
        "equiv" => "≡",
        "infty" => "∞",
        "pi" => "π",
        "alpha" => "α",
        "beta" => "β",
        "gamma" => "γ",
        "delta" => "δ",
        "theta" => "θ",
        "lambda" => "λ",
        "mu" => "μ",
        "sigma" => "σ",
        "Sigma" => "Σ",
        "Delta" => "Δ",
        "omega" => "ω",
        "to" | "rightarrow" => "→",
        "Rightarrow" | "implies" => "⇒",
        "leftarrow" => "←",
        "iff" => "⇔",
        "ldots" | "dots" | "cdots" => "…",
        "degree" | "circ" => "°",
        "percent" => "%",
        "sum" => "Σ",
        "prod" => "Π",
        "in" => "∈",
        "cup" => "∪",
        "cap" => "∩",
        "forall" => "∀",
        "exists" => "∃",
        "int" => "∫",
        "iint" => "∬",
        "oint" => "∮",
        "partial" => "∂",
        "nabla" => "∇",
        "lfloor" => "⌊",
        "rfloor" => "⌋",
        "lceil" => "⌈",
        "rceil" => "⌉",
        "angle" => "∠",
        "triangle" => "△",
        "perp" => "⊥",
        "parallel" => "∥",
        "therefore" => "∴",
        "notin" => "∉",
        "subset" => "⊂",
        "emptyset" => "∅",
        "quad" | "," | ";" | " " => " ",
        "qquad" => "  ",
        "!" => "",
        _ => return None,
    })
}

/// One piece of LaTeX math, as text.
fn convert(src: &str) -> String {
    let b: Vec<char> = src.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    const TAKES_GROUP: &[&str] = &[
        "frac",
        "dfrac",
        "tfrac",
        "sqrt",
        "boxed",
        "text",
        "textbf",
        "mathrm",
        "mathbf",
        "operatorname",
        "mbox",
        "textit",
        "mathit",
        "left",
        "right",
        "binom",
        "mathbb",
        "mathcal",
        "vec",
        "hat",
        "bar",
        "overline",
        "begin",
        "end",
    ];
    while i < b.len() {
        let c = b[i];
        if c == '\\' {
            let name: String = if b.get(i + 1).is_some_and(|c| c.is_ascii_alphabetic()) {
                b[i + 1..]
                    .iter()
                    .take_while(|c| c.is_ascii_alphabetic())
                    .collect()
            } else {
                b.get(i + 1).map(|c| c.to_string()).unwrap_or_default()
            };
            i += 1 + name.chars().count();
            if TAKES_GROUP.contains(&name.as_str()) {
                while b.get(i) == Some(&' ') {
                    i += 1;
                }
            }
            let arg = |i: &mut usize| -> String {
                if b.get(*i) == Some(&'{') {
                    let (g, e) = group(&b, *i);
                    *i = e;
                    g
                } else {
                    String::new()
                }
            };
            match name.as_str() {
                "frac" | "dfrac" | "tfrac" => {
                    let a = convert(&arg(&mut i));
                    let d = convert(&arg(&mut i));
                    let p = |x: &str| {
                        if needs_parens(x) {
                            format!("({x})")
                        } else {
                            x.to_string()
                        }
                    };
                    let below = if needs_parens_below(&d) {
                        format!("({d})")
                    } else {
                        d.clone()
                    };
                    out.push_str(&format!("{}/{}", p(&a), below));
                }
                "binom" => {
                    let a = convert(&arg(&mut i));
                    let d = convert(&arg(&mut i));
                    out.push_str(&format!("C({a}, {d})"));
                }
                // \begin{aligned} … \end{aligned}: the lines inside are what matters.
                "begin" | "end" => {
                    let _ = arg(&mut i);
                }
                "\\" => out.push('\n'),
                "sqrt" => {
                    let mut root = String::new();
                    if b.get(i) == Some(&'[')
                        && let Some(c) = (i + 1..b.len()).find(|&k| b[k] == ']')
                    {
                        root = b[i + 1..c].iter().collect::<String>().trim().to_string();
                        i = c + 1;
                    }
                    let a = convert(&arg(&mut i));
                    match root.as_str() {
                        "" => out.push('√'),
                        "3" => out.push('∛'),
                        "4" => out.push('∜'),
                        r => {
                            out.push_str(&sup(&convert(r)));
                            out.push('√');
                        }
                    }
                    out.push_str(&if needs_parens(&a) {
                        format!("({a})")
                    } else {
                        a
                    });
                }
                "text" | "mbox" => out.push_str(&arg(&mut i)),
                "boxed" | "textbf" | "mathrm" | "mathbf" | "mathbb" | "mathcal" | "vec" | "hat"
                | "bar" | "overline" | "operatorname" | "textit" | "mathit" => {
                    let a = arg(&mut i);
                    out.push_str(&convert(&a));
                }
                "left" | "right" | "big" | "Big" | "displaystyle" => {}
                "{" | "}" | "%" | "$" | "#" | "&" | "_" => out.push_str(&name),
                n => {
                    out.push_str(symbol(n).unwrap_or(n));
                    // ∂f, ∇f, ∫x, Σi: these read as one with what follows.
                    if ["partial", "nabla", "int", "iint", "oint", "sum", "prod"].contains(&n) {
                        while b.get(i) == Some(&' ') {
                            i += 1;
                        }
                    }
                }
            }
        } else if c == '^' || c == '_' {
            i += 1;
            let a = if b.get(i) == Some(&'{') {
                let (g, e) = group(&b, i);
                i = e;
                g
            } else if b.get(i) == Some(&'\\') {
                // ^\circ: the whole command, not just its backslash.
                let n: String = b[i + 1..]
                    .iter()
                    .take_while(|c| c.is_ascii_alphabetic())
                    .collect();
                let n = if n.is_empty() {
                    b.get(i + 1).map(|c| c.to_string()).unwrap_or_default()
                } else {
                    n
                };
                i += 1 + n.chars().count();
                format!("\\{n}")
            } else {
                let s = b.get(i).map(|c| c.to_string()).unwrap_or_default();
                i += 1;
                s
            };
            let a = convert(&a);
            out.push_str(&if c == '^' && a != "°" {
                sup(&a)
            } else if c == '_' {
                sub(&a)
            } else {
                a
            });
        } else if c == '&' {
            // Alignment: "x &= 2" is "x = 2"; in cases, "1 & x > 0" is "1, x > 0".
            let next = b[i + 1..].iter().find(|c| !c.is_whitespace());
            if !next.is_some_and(|c| "=<>≤≥".contains(*c)) {
                out.push_str(", ");
            }
            i += 1;
        } else if c == '{' || c == '}' {
            i += 1;
        } else {
            out.push(c);
            i += 1;
        }
    }
    // Tidy: no space just inside brackets or around line breaks, single spaces.
    let mut t = String::new();
    for ch in out.chars() {
        if ch == ' ' && (t.ends_with([' ', '\n']) || t.ends_with(['(', '[', '{', '⌊', '⌈'])) {
            continue;
        }
        if matches!(ch, ')' | ']' | '}' | '⌋' | '⌉' | '\n' | ',') && t.ends_with(' ') {
            t.pop();
        }
        t.push(ch);
    }
    t.trim().to_string()
}

/// Text from the network (release notes, model cards) without control
/// characters: an escape sequence must never reach the terminal.
pub fn clean(s: &str) -> String {
    s.chars()
        .map(|c| if c == '\t' { ' ' } else { c })
        .filter(|c| !c.is_control())
        .collect()
}

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
        if let Some((num, rest)) = t.split_once(". ")
            && !num.is_empty()
            && num.len() <= 3
            && num.chars().all(|c| c.is_ascii_digit())
        {
            out.extend(wrap(
                inline(rest, base),
                width,
                Span::raw(format!("{pad}{num}. ")),
            ));
            continue;
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

    /// Lines to draw and the (column, row) of the cursor, wrapping at `width`.
    /// Rows to draw at `width` columns, wrapped between words like the chat
    /// (a word longer than the row is cut), and the cursor's (column, row).
    pub fn layout(&self, width: usize) -> (Vec<String>, (u16, u16)) {
        let width = width.max(4);
        let cw = |c: char| UnicodeWidthStr::width(c.encode_utf8(&mut [0; 4]) as &str);
        // Each row keeps (byte index, char), so the cursor can be found after wrapping.
        let mut rows: Vec<Vec<(usize, char)>> = vec![Vec::new()];
        for (i, ch) in self.text.char_indices() {
            if ch == '\n' {
                rows.last_mut().unwrap().push((i, '\n'));
                rows.push(Vec::new());
                continue;
            }
            let row = rows.last_mut().unwrap();
            let used: usize = row.iter().map(|(_, c)| cw(*c)).sum();
            if used + cw(ch) > width {
                // Move the unfinished word down, if the row has a space before it.
                let tail = match row.iter().rposition(|(_, c)| *c == ' ') {
                    Some(sp) if sp + 1 < row.len() && ch != ' ' => row.split_off(sp + 1),
                    _ => Vec::new(),
                };
                rows.push(tail);
            }
            rows.last_mut().unwrap().push((i, ch));
        }
        let mut cur = None;
        for (r, row) in rows.iter().enumerate() {
            let mut col = 0;
            for (i, c) in row {
                if *i == self.cursor {
                    cur = Some((col as u16, r as u16));
                }
                col += cw(*c);
            }
        }
        let cur = cur.unwrap_or_else(|| {
            // At the very end of the text.
            let last = rows.len() - 1;
            let col: usize = rows[last]
                .iter()
                .filter(|(_, c)| *c != '\n')
                .map(|(_, c)| cw(*c))
                .sum();
            if col >= width {
                rows.push(Vec::new());
                (0, (last + 1) as u16)
            } else {
                (col as u16, last as u16)
            }
        });
        let rows = rows
            .into_iter()
            .map(|r| {
                r.into_iter()
                    .filter(|(_, c)| *c != '\n')
                    .map(|(_, c)| c)
                    .collect()
            })
            .collect();
        (rows, cur)
    }
}

/// A Hugging Face model card made readable in a terminal: no HTML tags, no
/// images, links shown as their text.
pub fn readable_card(md: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    let mut in_code = false;
    for line in md.lines() {
        if line.trim_start().starts_with("```") {
            in_code = !in_code;
        }
        if in_code {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        let mut l = String::new();
        for ch in line.chars() {
            match ch {
                '<' => in_tag = true,
                '>' if in_tag => in_tag = false,
                _ if !in_tag => l.push(ch),
                _ => {}
            }
        }
        // ![alt](url) -> nothing, [text](url) -> text
        let mut res = String::new();
        let b: Vec<char> = l.chars().collect();
        let mut i = 0;
        while i < b.len() {
            let image = b[i] == '!' && b.get(i + 1) == Some(&'[');
            let start = if image { i + 1 } else { i };
            if b[start] == '['
                && let Some(close) = (start + 1..b.len()).find(|&k| b[k] == ']')
                && b.get(close + 1) == Some(&'(')
                && let Some(end) = (close + 2..b.len()).find(|&k| b[k] == ')')
            {
                if !image {
                    res.extend(&b[start + 1..close]);
                }
                i = end + 1;
                continue;
            }
            res.push(b[i]);
            i += 1;
        }
        let res = res
            .replace("&nbsp;", " ")
            .replace("&amp;", "&")
            .replace("&lt;", "<")
            .replace("&gt;", ">");
        if res.trim().is_empty() && !line.trim().is_empty() {
            continue; // a line that was only HTML or images
        }
        out.push_str(&res);
        out.push('\n');
    }
    out
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

    #[test]
    fn the_editor_wraps_between_words() {
        let mut e = Editor::with("Explain what a language model is, in two short sentences.");
        e.cursor = e.text.len();
        let (rows, cur) = e.layout(20);
        assert!(
            rows.iter()
                .all(|r| UnicodeWidthStr::width(r.as_str()) <= 20),
            "{rows:?}"
        );
        assert_eq!(rows[0], "Explain what a ");
        assert!(rows.iter().all(|r| !r.starts_with(['.', ','])), "{rows:?}");
        let last = rows.len() - 1;
        assert_eq!(
            cur,
            (
                UnicodeWidthStr::width(rows[last].as_str()) as u16,
                last as u16
            )
        );
        // The cursor follows a word moved to the next row.
        let mut e = Editor::with("aaaa bbbbbb");
        e.cursor = 6; // inside "bbbbbb"
        let (rows, cur) = e.layout(8);
        assert_eq!(rows, vec!["aaaa ", "bbbbbb"]);
        assert_eq!(cur, (1, 1));
    }

    #[test]
    fn model_cards_lose_html_and_links_but_keep_code() {
        let card = "<p align=\"center\"><img src=\"x.png\"></p>\nSee [Qwen](https://huggingface.co/Qwen) for more. ![logo](l.png)\n```\n<keep> [a](b)\n```";
        let r = readable_card(card);
        assert!(!r.contains("<p") && !r.contains("img"), "{r}");
        assert!(r.contains("See Qwen for more."), "{r}");
        assert!(r.contains("<keep> [a](b)"), "code stays as written: {r}");
    }

    #[test]
    fn math_reads_as_plain_text_like_the_desktop() {
        let cases: Vec<(String, String)> =
            serde_json::from_str(include_str!("../../../contracts/plainmath-cases.json")).unwrap();
        let mut bad = Vec::new();
        for (input, want) in &cases {
            let got = plain_math(input);
            if &got != want {
                bad.push(format!("{input:?}\n  got  {got:?}\n  want {want:?}"));
            }
        }
        assert!(bad.is_empty(), "{}", bad.join("\n"));
    }

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
        e.clear();
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
