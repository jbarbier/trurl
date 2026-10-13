//! Drawing. Only the 16 basic terminal colours are used, so Trurl looks right
//! in every terminal, including the old Windows console.

use crate::state::*;
use crate::text::{human_duration, human_size, markdown, wrap, wrap_plain};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap};
use trurl_core::chat::TurnStatus;
use trurl_core::downloads::DownloadStatus;
use trurl_core::events::EngineState;
use trurl_core::hardware::{CpuVariant, RamFit};

pub const ACCENT: Color = Color::LightBlue;
const DIM: Color = Color::DarkGray;

fn fit_span(f: RamFit) -> Vec<Span<'static>> {
    let (c, label) = match f {
        RamFit::Green => (Color::Green, "Fits easily"),
        RamFit::Orange => (Color::Yellow, "Tight fit"),
        RamFit::Red => (Color::Red, "Too big"),
    };
    vec![
        Span::styled("● ", Style::default().fg(c)),
        Span::styled(label, Style::default().fg(c)),
    ]
}

fn status_style(kind: &str) -> Style {
    match kind {
        "ready" => Style::default().fg(Color::Green),
        "loading" | "thinking" | "answering" | "starting" => Style::default().fg(Color::Yellow),
        "failed" => Style::default().fg(Color::Red),
        _ => Style::default().fg(DIM),
    }
}

fn status_word(kind: &str) -> &'static str {
    match kind {
        "ready" => "Ready",
        "loading" => "Loading",
        "thinking" => "Thinking",
        "answering" => "Answering",
        "starting" => "Starting",
        "failed" => "Failed",
        _ => "No model",
    }
}

fn block(title: &str) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(DIM))
        .title(Span::styled(
            format!(" {title} "),
            Style::default().add_modifier(Modifier::BOLD),
        ))
}

fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width.saturating_sub(2));
    let h = h.min(area.height.saturating_sub(2));
    Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    }
}

pub fn draw(f: &mut Frame, ui: &Ui) {
    let area = f.area();
    let mut constraints = vec![Constraint::Length(1)];
    let banner = banner_lines(ui, area.width as usize);
    if !banner.is_empty() {
        constraints.push(Constraint::Length(banner.len() as u16));
    }
    constraints.push(Constraint::Min(5));
    constraints.push(Constraint::Length(1));
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);
    draw_top(f, ui, rows[0]);
    let mut i = 1;
    if !banner.is_empty() {
        f.render_widget(Paragraph::new(banner), rows[1]);
        i = 2;
    }
    let main = rows[i];
    match ui.screen {
        Screen::Chat => draw_chat(f, ui, main),
        Screen::Settings => draw_settings(f, ui, main),
        Screen::Models => draw_models(f, ui, main),
        Screen::Hub => draw_hub(f, ui, main),
        Screen::About => draw_about(f, ui, main),
        Screen::Token => draw_token(f, ui, main),
    }
    draw_footer(f, ui, rows[i + 1]);
    if let Some(c) = &ui.confirm {
        draw_confirm(f, c, area);
    }
    if let Some((kind, ed)) = &ui.prompt {
        draw_prompt(f, *kind, ed, area);
    }
}

fn draw_top(f: &mut Frame, ui: &Ui, area: Rect) {
    let (name, kind) = ui.engine_label();
    let mut spans = vec![
        Span::styled(
            " Trurl ",
            Style::default()
                .fg(Color::Black)
                .bg(ACCENT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(name, Style::default().add_modifier(Modifier::BOLD)),
        Span::raw("  "),
        Span::styled(format!("● {}", status_word(kind)), status_style(kind)),
    ];
    if let Some(s) = &ui.settings {
        spans.push(Span::styled(
            format!(
                "   temperature {:.1} · context {}",
                s.temperature, s.context_size
            ),
            Style::default().fg(DIM),
        ));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn banner_lines(ui: &Ui, width: usize) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    match &ui.update {
        UpdateState::Available(u) => {
            let hint = if ui.generating {
                "F7 to see what is new and update when the answer is finished, F8 for later"
            } else {
                "F7 to see what is new and update, F8 for later"
            };
            out.push(Line::from(vec![
                Span::styled(
                    format!(" Update available: version {} ", u.version),
                    Style::default().fg(Color::Black).bg(Color::Cyan),
                ),
                Span::styled(
                    format!("  {}  ", u.notes.lines().next().unwrap_or("")),
                    Style::default(),
                ),
                Span::styled(hint, Style::default().fg(DIM)),
            ]));
        }
        UpdateState::Downloading(u, done, total) => {
            let pct = total
                .map(|t| (done * 100 / t.max(1)).to_string() + "%")
                .unwrap_or_else(|| human_size(*done));
            out.push(Line::from(Span::styled(
                format!(" Downloading version {}: {pct}", u.version),
                Style::default().fg(Color::Cyan),
            )));
        }
        UpdateState::Failed(e) => out.push(Line::from(Span::styled(
            format!(" {} {}", e.title, e.hint),
            Style::default().fg(Color::Yellow),
        ))),
        _ => {}
    }
    if let Some(m) = &ui.update_msg {
        out.push(Line::from(Span::styled(
            format!(" {m}"),
            Style::default().fg(Color::Cyan),
        )));
    }
    if let Some(n) = &ui.notice {
        out.extend(wrap(
            vec![
                Span::styled(
                    n.title.clone(),
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                ),
                Span::raw(format!(" {} ", n.hint)),
                Span::styled("(Esc to close)", Style::default().fg(DIM)),
            ],
            width,
            Span::raw(" "),
        ));
        if let Some(d) = &n.detail {
            let d: String = crate::text::clean(d).chars().take(200).collect();
            out.extend(wrap_plain(
                &format!("Details: {d}"),
                width,
                Style::default().fg(DIM),
                Span::raw(" "),
            ));
        }
    }
    out
}

fn draw_footer(f: &mut Frame, ui: &Ui, area: Rect) {
    // (key, what it does, always shown). Whatever does not fit is dropped from
    // the end of the optional ones, so F1 (all the keys) and quitting never are.
    let ran_out = ui.turns.last().is_some_and(|t| {
        t.error
            .as_ref()
            .is_some_and(|e| e.code == trurl_core::ErrorCode::ContextFull)
    });
    let keys: Vec<(&str, &str, bool)> = match ui.screen {
        Screen::Chat if ui.generating => vec![
            ("Esc", "stop", true),
            ("Ctrl+T", "thinking", false),
            ("PgUp/PgDn", "scroll", false),
            ("F1", "keys", true),
        ],
        Screen::Chat => {
            let mut k = vec![("Enter", "send", true)];
            if ran_out {
                k.push(("F5", "bigger context", true));
            }
            k.extend([
                ("F6", "copy answer", false),
                ("Ctrl+N", "new chat", false),
                ("F2", "models", false),
                ("F3", "settings", false),
                ("F4", "Hugging Face", false),
                ("Alt+Enter", "new line", false),
                ("F1", "keys", true),
                ("Ctrl+Q", "quit", true),
            ]);
            k
        }
        Screen::Settings => vec![
            ("↑↓", "choose", true),
            ("←→", "change", true),
            ("Enter", "edit or apply", true),
            ("R", "reset", false),
            ("Esc", "back", true),
        ],
        Screen::Models => vec![
            ("↑↓", "choose", true),
            ("Enter", "use", true),
            ("D", "delete", false),
            ("I", "import", false),
            ("F4", "Hugging Face", false),
            ("Esc", "back", true),
        ],
        Screen::Hub => match ui.hub_focus {
            HubFocus::Downloads => vec![
                ("P", "pause or resume", true),
                ("X", "cancel", true),
                ("Tab", "move", true),
                ("Esc", "back", true),
            ],
            HubFocus::Input => vec![
                ("Enter", "search", true),
                ("Tab", "go to the list", true),
                ("Esc", "back", true),
            ],
            _ => vec![
                ("Enter", "open / download", true),
                ("Tab", "move", true),
                ("Esc", "back", true),
            ],
        },
        Screen::About if ui.license_open.is_some() => {
            vec![("PgUp/PgDn", "scroll", true), ("Esc", "back", true)]
        }
        Screen::About => vec![
            ("↑↓", "license", true),
            ("Enter", "read", true),
            ("PgUp/PgDn", "scroll", false),
            ("U", "updates", false),
            ("Esc", "back", true),
        ],
        Screen::Token => vec![
            ("Enter", "save", true),
            ("Ctrl+D", "remove", true),
            ("Esc", "back", true),
        ],
    };
    let width = |ks: &[(&str, &str, bool)]| -> usize {
        ks.iter()
            .map(|(k, v, _)| {
                unicode_width::UnicodeWidthStr::width(*k)
                    + unicode_width::UnicodeWidthStr::width(*v)
                    + 4
            })
            .sum()
    };
    let mut keys = keys;
    while width(&keys) > area.width as usize {
        match keys.iter().rposition(|(_, _, keep)| !keep) {
            Some(i) => {
                keys.remove(i);
            }
            None => break,
        }
    }
    let mut spans = Vec::new();
    for (k, v, _) in &keys {
        spans.push(Span::styled(
            format!(" {k} "),
            Style::default().fg(Color::Black).bg(Color::Gray),
        ));
        spans.push(Span::raw(format!(" {v} ")));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

// ----- chat --------------------------------------------------------------------

fn draw_chat(f: &mut Frame, ui: &Ui, area: Rect) {
    let (side, main) = if area.width >= 100 {
        let c = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(34), Constraint::Min(40)])
            .split(area);
        (Some(c[0]), c[1])
    } else {
        (None, area)
    };
    if let Some(s) = side {
        draw_sidebar(f, ui, s);
    }
    let input_w = main.width.saturating_sub(4) as usize;
    let (rows, cur) = ui.input.layout(input_w);
    let input_h = (rows.len() as u16).clamp(1, 6) + 2;
    let parts = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(input_h)])
        .split(main);

    // Transcript
    let width = parts[0].width.saturating_sub(2) as usize;
    let lines = chat_lines(ui, width);
    let visible = parts[0].height as usize;
    let max_scroll = lines.len().saturating_sub(visible);
    ui.max_scroll.set(max_scroll);
    let scroll = ui.scroll.min(max_scroll);
    let top = lines.len().saturating_sub(visible + scroll);
    let shown: Vec<Line> = lines.into_iter().skip(top).take(visible).collect();
    f.render_widget(
        Paragraph::new(shown)
            .block(Block::default().padding(ratatui::widgets::Padding::horizontal(1))),
        parts[0],
    );
    if scroll > 0 {
        let tag = " ↓ more below: End to jump to latest ";
        let r = Rect {
            x: parts[0]
                .right()
                .saturating_sub(unicode_width::UnicodeWidthStr::width(tag) as u16 + 1),
            y: parts[0].bottom().saturating_sub(1),
            width: unicode_width::UnicodeWidthStr::width(tag) as u16,
            height: 1,
        };
        f.render_widget(
            Paragraph::new(Span::styled(
                tag,
                Style::default().fg(Color::Black).bg(Color::Yellow),
            )),
            r,
        );
    }

    // Input
    let ready = matches!(ui.engine, EngineState::Ready { .. });
    let title = if ui.generating {
        "Message (Esc stops the answer)".to_string()
    } else if !ready {
        match &ui.engine {
            EngineState::Loading { .. } if ui.queued => {
                "Message (sent as soon as the model is ready)".into()
            }
            EngineState::Loading { .. } => {
                "Message (type now: Enter sends it when the model is ready)".into()
            }
            EngineState::Failed { .. } => {
                "Message (the model did not load: F2 to pick another)".into()
            }
            _ => "Message (no model yet: F2 to add one)".into(),
        }
    } else {
        "Message".into()
    };
    let skip = (cur.1 as usize + 1).saturating_sub(6);
    let text: Vec<Line> = rows
        .iter()
        .skip(skip)
        .map(|r| Line::from(r.clone()))
        .collect();
    let b = block(&title).border_style(Style::default().fg(if ready && !ui.generating {
        ACCENT
    } else {
        DIM
    }));
    f.render_widget(Paragraph::new(text).block(b), parts[1]);
    if ui.screen == Screen::Chat && ui.confirm.is_none() && ui.prompt.is_none() {
        f.set_cursor_position((parts[1].x + 1 + cur.0, parts[1].y + 1 + cur.1 - skip as u16));
    }
}

fn draw_sidebar(f: &mut Frame, ui: &Ui, area: Rect) {
    let w = area.width.saturating_sub(4) as usize;
    let mut l: Vec<Line> = Vec::new();
    let label = |t: &str| {
        Line::from(Span::styled(
            t.to_string(),
            Style::default().fg(DIM).add_modifier(Modifier::BOLD),
        ))
    };
    l.push(label("MODEL"));
    match ui.current_model() {
        Some(m) => {
            l.extend(wrap(
                vec![Span::styled(
                    m.name.clone(),
                    Style::default().add_modifier(Modifier::BOLD),
                )],
                w,
                Span::raw(""),
            ));
            let mut s = vec![Span::raw(format!("{} · ", human_size(m.size)))];
            s.extend(fit_span(m.fit));
            l.push(Line::from(s));
        }
        None => l.push(Line::from("None yet (F2)")),
    }
    l.push(Line::from(""));
    if let Some(s) = &ui.settings {
        l.push(label("TEMPERATURE"));
        let filled = (s.temperature / 2.0 * 20.0).round() as usize;
        l.push(Line::from(vec![
            Span::styled("━".repeat(filled), Style::default().fg(ACCENT)),
            Span::styled("─".repeat(20 - filled.min(20)), Style::default().fg(DIM)),
            Span::raw(format!(" {:.1}", s.temperature)),
        ]));
        l.push(Line::from(Span::styled(
            "Low: focused. High: varied.",
            Style::default().fg(DIM),
        )));
        l.push(Line::from(""));
        l.push(label("CONTEXT"));
        l.push(Line::from(format!("{} tokens", s.context_size)));
        l.push(Line::from(""));
        l.push(label("SYSTEM PROMPT"));
        if s.system_prompt.trim().is_empty() {
            l.push(Line::from(Span::styled(
                "(empty)",
                Style::default().fg(DIM),
            )));
        } else {
            let mut p = wrap_plain(
                s.system_prompt.trim(),
                w,
                Style::default().add_modifier(Modifier::ITALIC),
                Span::raw(""),
            );
            if p.len() > 6 {
                p.truncate(6);
                p.push(Line::from(Span::styled("…", Style::default().fg(DIM))));
            }
            l.extend(p);
        }
        l.push(Line::from(""));
        l.push(Line::from(Span::styled(
            "F3 to change these",
            Style::default().fg(DIM),
        )));
    }
    f.render_widget(
        Paragraph::new(l)
            .wrap(Wrap { trim: false })
            .block(block("Settings")),
        area,
    );
}

pub fn chat_lines(ui: &Ui, width: usize) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();
    if ui.turns.is_empty() {
        let msg = match &ui.engine {
            EngineState::Idle if ui.models.is_empty() => {
                "No model yet.\n\nPress F2 to import a model file (from a USB stick or a folder), or F4 to download one from Hugging Face. Once a model is on this computer, Trurl works without internet."
            }
            EngineState::Idle => "Pick a model with F2.",
            EngineState::Loading { .. } => {
                "Loading the model. This can take a minute on older computers.\n\nYou can type your message now."
            }
            EngineState::Ready { .. } => {
                "Ready. Type a message and press Enter.\n\nTry: \"Explain what a language model is, in two sentences.\""
            }
            EngineState::Failed { .. } => "",
        };
        out.extend(wrap_plain(
            msg,
            width,
            Style::default().fg(DIM),
            Span::raw(""),
        ));
    }
    if let EngineState::Failed { error, .. } = &ui.engine {
        out.extend(wrap(
            vec![Span::styled(
                error.title.clone(),
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            )],
            width,
            Span::raw(""),
        ));
        out.extend(wrap(
            vec![Span::raw(error.hint.clone())],
            width,
            Span::raw(""),
        ));
        out.push(Line::from(Span::styled(
            "Press Ctrl+R to try again, or F2 to choose another model.",
            Style::default().fg(DIM),
        )));
        out.push(Line::from(""));
    }
    for t in &ui.turns {
        out.push(Line::from(Span::styled(
            "You",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        )));
        out.extend(wrap_plain(
            &t.user,
            width,
            Style::default(),
            Span::raw("  "),
        ));
        out.push(Line::from(""));
        out.push(Line::from(Span::styled(
            t.model.clone(),
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        )));
        // What this answer was written with: comparing answers is the lesson.
        let sp = t.system_prompt.trim();
        let settings = format!(
            "temperature {:.1} · {}",
            t.temperature,
            if sp.is_empty() {
                "no system prompt".to_string()
            } else {
                format!("system prompt: “{}”", crate::text::clean(sp))
            }
        );
        out.extend(wrap_plain(
            &settings,
            width,
            Style::default().fg(DIM),
            Span::raw("  "),
        ));
        if !t.reasoning.is_empty() {
            let open = ui.thinking_open(t);
            let still = t.status == TurnStatus::Streaming && t.answer.is_empty();
            let label = if still {
                "Thinking…".to_string()
            } else if let Some(ms) = t.thought_ms {
                format!("Thought for {}", human_duration(ms))
            } else if t.status == TurnStatus::Stopped && t.answer.is_empty() {
                "Stopped while thinking".to_string()
            } else if t.status == TurnStatus::Cut && t.answer.is_empty() {
                "Ran out of room while thinking".to_string()
            } else if t.status == TurnStatus::Error && t.answer.is_empty() {
                "Thinking (cut short)".to_string()
            } else {
                "Thinking".to_string()
            };
            let arrow = if open { "▾" } else { "▸" };
            let hint = if open { "" } else { "  (Ctrl+T to read it)" };
            out.push(Line::from(vec![
                Span::styled(
                    format!("  {arrow} {label}"),
                    Style::default().fg(DIM).add_modifier(Modifier::ITALIC),
                ),
                Span::styled(hint, Style::default().fg(DIM)),
            ]));
            if open {
                out.extend(wrap_plain(
                    t.reasoning.trim(),
                    width,
                    Style::default().fg(DIM).add_modifier(Modifier::ITALIC),
                    Span::styled("  │ ", Style::default().fg(DIM)),
                ));
            }
            out.push(Line::from(""));
        }
        if t.answer.is_empty() && t.status == TurnStatus::Streaming && t.reasoning.is_empty() {
            out.push(Line::from(Span::styled("  …", Style::default().fg(DIM))));
        } else {
            for l in markdown(
                &crate::text::plain_math(&t.answer),
                width.saturating_sub(2),
                Style::default(),
            ) {
                let mut s = vec![Span::raw("  ")];
                s.extend(l.spans);
                out.push(Line::from(s));
            }
        }
        match t.status {
            TurnStatus::Stopped => out.push(Line::from(Span::styled(
                "  Stopped.",
                Style::default().fg(DIM),
            ))),
            TurnStatus::Cut | TurnStatus::Error => {
                if let Some(e) = &t.error {
                    out.extend(wrap(
                        vec![
                            Span::styled(
                                e.title.clone(),
                                Style::default()
                                    .fg(Color::Yellow)
                                    .add_modifier(Modifier::BOLD),
                            ),
                            Span::raw(format!(" {}", e.hint)),
                        ],
                        width,
                        Span::raw("  "),
                    ));
                    if e.code == trurl_core::ErrorCode::ContextFull {
                        out.push(Line::from(Span::styled(
                            "  F5: a bigger context, and the same question back in the box.",
                            Style::default().fg(ACCENT),
                        )));
                    }
                    if t.answer.is_empty() && !t.reasoning.is_empty() {
                        out.extend(wrap_plain(
                            "Small thinking models sometimes think for a very long time: that is normal. To skip the thinking, start your message with /no_think.",
                            width,
                            Style::default().fg(DIM),
                            Span::raw("  "),
                        ));
                    }
                }
            }
            _ => {}
        }
        out.push(Line::from(""));
    }
    out
}

// ----- settings ------------------------------------------------------------------

fn draw_settings(f: &mut Frame, ui: &Ui, area: Rect) {
    let Some(s) = &ui.settings else { return };
    let w = area.width.saturating_sub(6) as usize;
    let mut l: Vec<Line> = Vec::new();
    for (i, field) in SettingsField::ALL.iter().enumerate() {
        let sel = i == ui.settings_sel;
        let mark = if sel { "› " } else { "  " };
        let st = if sel {
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
        } else {
            Style::default().add_modifier(Modifier::BOLD)
        };
        match field {
            SettingsField::Model => {
                l.push(Line::from(vec![Span::styled(format!("{mark}Model"), st)]));
                let name = ui
                    .current_model()
                    .map(|m| m.name.clone())
                    .unwrap_or_else(|| "None".into());
                l.push(Line::from(format!("    {name}   (Enter: choose another)")));
            }
            SettingsField::SystemPrompt => {
                l.push(Line::from(vec![Span::styled(format!("{mark}System prompt"), st), Span::styled("   Instructions the model reads before every message. Enter: edit. R: clear.", Style::default().fg(DIM))]));
                if s.system_prompt.trim().is_empty() {
                    l.push(Line::from(Span::styled(
                        "    (empty)",
                        Style::default().fg(DIM),
                    )));
                } else {
                    l.extend(wrap_plain(
                        s.system_prompt.trim(),
                        w,
                        Style::default().add_modifier(Modifier::ITALIC),
                        Span::raw("    "),
                    ));
                }
            }
            SettingsField::Temperature => {
                let changed = (s.temperature - s.default_temperature).abs() > 0.01;
                l.push(Line::from(vec![
                    Span::styled(format!("{mark}Temperature  "), st),
                    Span::raw(format!("◀ {:.1} ▶", s.temperature)),
                    Span::styled(
                        if changed {
                            format!("   default {:.1}, R to reset", s.default_temperature)
                        } else {
                            "   default".into()
                        },
                        Style::default().fg(DIM),
                    ),
                ]));
                l.push(Line::from(Span::styled("    Low: focused and repeatable. High: more varied. 0 gives the same answer every time.", Style::default().fg(DIM))));
            }
            SettingsField::Context => {
                let shown = ui.pending_context.unwrap_or(s.context_size);
                let pending = ui
                    .pending_context
                    .filter(|p| *p != s.context_size)
                    .is_some();
                l.push(Line::from(vec![
                    Span::styled(format!("{mark}Context size  "), st),
                    Span::raw(format!("◀ {shown} tokens ▶")),
                    Span::styled(
                        if pending {
                            "   Enter to apply (reloads the model)".to_string()
                        } else if s.context_size != s.default_context {
                            format!("   default {}, R to reset", s.default_context)
                        } else {
                            "   default".into()
                        },
                        Style::default().fg(if pending { Color::Yellow } else { DIM }),
                    ),
                ]));
                l.push(Line::from(Span::styled(
                    "    How much of the conversation the model can see. Bigger needs more memory.",
                    Style::default().fg(DIM),
                )));
            }
        }
        l.push(Line::from(""));
    }
    f.render_widget(
        Paragraph::new(l)
            .wrap(Wrap { trim: false })
            .block(block("Settings")),
        area,
    );
}

// ----- models ----------------------------------------------------------------------

fn draw_models(f: &mut Frame, ui: &Ui, area: Rect) {
    let mut l: Vec<Line> = Vec::new();
    if ui.models.is_empty() {
        l.push(Line::from("No models yet. Press I to import a .gguf file (for example from a USB stick), or F4 to download one."));
    }
    let loaded = match &ui.engine {
        EngineState::Ready { file, .. } | EngineState::Loading { file, .. } => Some(file.clone()),
        _ => None,
    };
    for (i, m) in ui.models.iter().enumerate() {
        let sel = i == ui.models_sel;
        let st = if sel {
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        let mut s = vec![
            Span::styled(if sel { "› " } else { "  " }, st),
            Span::styled(format!("{:<30}", m.name), st),
        ];
        s.push(Span::raw(format!(
            " {:<8} {:>8}  ",
            m.quant.clone().unwrap_or_default(),
            human_size(m.size)
        )));
        s.extend(fit_span(m.fit));
        if m.bundled {
            s.push(Span::styled(
                "  included with Trurl",
                Style::default().fg(DIM),
            ));
        }
        if m.broken {
            s.push(Span::styled(
                "  damaged file",
                Style::default().fg(Color::Red),
            ));
        }
        if loaded.as_deref() == Some(m.file.as_str()) {
            s.push(Span::styled(
                "  ◆ in use",
                Style::default().fg(Color::Green),
            ));
        }
        l.push(Line::from(s));
    }
    l.push(Line::from(""));
    if let Some((file, done, total)) = &ui.import {
        l.push(Line::from(format!(
            "Importing {file}: {}%",
            done * 100 / (*total).max(1)
        )));
    }
    if let Some((file, done, total)) = &ui.verify {
        l.push(Line::from(Span::styled(
            format!("Checking {file}: {}%", done * 100 / (*total).max(1)),
            Style::default().fg(DIM),
        )));
    }
    downloads_lines(ui, &mut l, false);
    l.push(Line::from(""));
    l.push(Line::from(Span::styled(
        format!("Models folder: {}", ui.models_dir),
        Style::default().fg(DIM),
    )));
    if let Some(h) = &ui.hardware {
        l.push(Line::from(Span::styled(
            format!(
                "This computer has {} of memory. Green: fits easily. Orange: tight. Red: too big.",
                human_size(h.total_ram)
            ),
            Style::default().fg(DIM),
        )));
    }
    f.render_widget(
        Paragraph::new(l)
            .block(block("Models on this computer"))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn downloads_lines(ui: &Ui, l: &mut Vec<Line<'static>>, selectable: bool) {
    if ui.downloads.is_empty() {
        return;
    }
    l.push(Line::from(Span::styled(
        "Downloads",
        Style::default().add_modifier(Modifier::BOLD),
    )));
    for (i, d) in ui.downloads.iter().enumerate() {
        let sel = selectable && i == ui.dl_sel && ui.hub_focus == HubFocus::Downloads;
        let pct = d.done * 100 / d.size.max(1);
        let bar_w = 20usize;
        let filled = (pct as usize * bar_w / 100).min(bar_w);
        let status = match d.status {
            DownloadStatus::Running => format!(
                "{} of {}  {}/s",
                human_size(d.done),
                human_size(d.size),
                human_size(d.rate)
            ),
            DownloadStatus::Retrying => match &d.error {
                Some(e) => format!("{} Trying again…", e.title),
                None => "Connection lost. Trying again…".into(),
            },
            DownloadStatus::Paused => format!("Paused at {}", human_size(d.done)),
            DownloadStatus::Verifying => "Checking the file…".into(),
            DownloadStatus::Failed => d
                .error
                .as_ref()
                .map(|e| format!("{} {}", e.title, e.hint))
                .unwrap_or_else(|| "Failed".into()),
            DownloadStatus::Done => "Done".into(),
        };
        l.push(Line::from(vec![
            Span::styled(if sel { "› " } else { "  " }, Style::default().fg(ACCENT)),
            Span::styled(
                format!("{:<34} ", d.dest_file),
                if sel {
                    Style::default().fg(ACCENT)
                } else {
                    Style::default()
                },
            ),
            Span::styled("█".repeat(filled), Style::default().fg(ACCENT)),
            Span::styled("░".repeat(bar_w - filled), Style::default().fg(DIM)),
            Span::raw(format!(" {pct:>3}%  ")),
            Span::styled(
                status,
                Style::default().fg(if d.status == DownloadStatus::Failed {
                    Color::Red
                } else {
                    DIM
                }),
            ),
        ]));
    }
}

// ----- Hugging Face ------------------------------------------------------------

fn draw_hub(f: &mut Frame, ui: &Ui, area: Rect) {
    match ui.hub_view {
        HubView::Checking => {
            f.render_widget(
                Paragraph::new("Checking the internet connection…").block(block("Hugging Face")),
                area,
            );
        }
        HubView::Offline => {
            let l = vec![
                Line::from(Span::styled(
                    "Connect to the internet to browse Hugging Face",
                    Style::default().add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
                Line::from("Everything else in Trurl works without internet."),
                Line::from(Span::styled(
                    "Press Enter to check again.",
                    Style::default().fg(DIM),
                )),
            ];
            let mut l = l;
            l.push(Line::from(""));
            downloads_lines(ui, &mut l, true);
            f.render_widget(Paragraph::new(l).block(block("Hugging Face")), area);
        }
        HubView::Search => {
            let rows = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(3), Constraint::Min(3)])
                .split(area);
            let focused = ui.hub_focus == HubFocus::Input;
            let b = block("Search Hugging Face for GGUF models")
                .border_style(Style::default().fg(if focused { ACCENT } else { DIM }));
            f.render_widget(Paragraph::new(ui.hub_query.text.clone()).block(b), rows[0]);
            if focused && ui.prompt.is_none() {
                f.set_cursor_position((
                    rows[0].x + 1 + ui.hub_query.layout(200).1.0,
                    rows[0].y + 1,
                ));
            }
            let mut l: Vec<Line> = Vec::new();
            if ui.hub_busy {
                l.push(Line::from("Searching…"));
            } else if ui.hub_results.is_empty() {
                l.push(Line::from(Span::styled("Type a name, for example: qwen3 0.6b, gemma 3 1b, llama 3.2 1b. Then press Enter.", Style::default().fg(DIM))));
            }
            for (i, h) in ui.hub_results.iter().enumerate() {
                let sel = i == ui.hub_sel && ui.hub_focus == HubFocus::Results;
                let st = if sel {
                    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                l.push(Line::from(vec![
                    Span::styled(if sel { "› " } else { "  " }, st),
                    Span::styled(format!("{:<48}", h.id), st),
                    Span::styled(
                        format!(
                            " {:>9} downloads  {:>5} likes  {}",
                            h.downloads,
                            h.likes,
                            h.updated.as_deref().unwrap_or("").get(..10).unwrap_or("")
                        ),
                        Style::default().fg(DIM),
                    ),
                    Span::styled(
                        if h.gated { "  needs an account" } else { "" },
                        Style::default().fg(Color::Yellow),
                    ),
                ]));
            }
            l.push(Line::from(""));
            downloads_lines(ui, &mut l, true);
            f.render_widget(Paragraph::new(l).block(block("Results")), rows[1]);
        }
        HubView::Details => draw_hub_details(f, ui, area),
    }
}

fn draw_hub_details(f: &mut Frame, ui: &Ui, area: Rect) {
    let Some(d) = &ui.hub_details else {
        f.render_widget(Paragraph::new("Loading…").block(block("Model")), area);
        return;
    };
    let mut head: Vec<Line> = Vec::new();
    let mut facts = Vec::new();
    if let Some(a) = &d.author {
        facts.push(format!("by {a}"));
    }
    if let Some(p) = d.parameters {
        facts.push(format!("{} parameters", human_params(p)));
    }
    if let Some(a) = &d.architecture {
        facts.push(a.clone());
    }
    if let Some(c) = d.context_length {
        facts.push(format!("context {c}"));
    }
    if let Some(l) = &d.license {
        facts.push(format!("license {l}"));
    }
    facts.push(format!("{} downloads", thousands(d.downloads)));
    if let Some(u) = d.updated.as_deref().and_then(|u| u.get(..10)) {
        facts.push(format!("updated {u}"));
    }
    head.push(Line::from(Span::styled(
        facts.join(" · "),
        Style::default().fg(DIM),
    )));
    if let Some(b) = &d.base_model {
        head.push(Line::from(Span::styled(
            format!("Based on {b}"),
            Style::default().fg(DIM),
        )));
    }
    if d.gated {
        head.push(Line::from(Span::styled(
            "Needs a Hugging Face account and token (F12).",
            Style::default().fg(Color::Yellow),
        )));
    }
    if d.chat == Some(false) {
        head.push(Line::from(Span::styled(
            "This model does not chat: it turns text into numbers, or works with sound. Look for a model whose name ends in Instruct or Chat.",
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        )));
    }
    head.push(Line::from(Span::styled(
        "Each file is the same model at a different size. Q4_K_M is small and good: the one to pick. Q8_0 is twice the size, a little more exact.",
        Style::default().fg(DIM),
    )));
    head.push(Line::from(""));
    for (i, file) in d.files.iter().enumerate() {
        let sel = i == ui.hub_file_sel && ui.hub_focus == HubFocus::Files;
        let st = if sel {
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        let mut s = vec![
            Span::styled(if sel { "› " } else { "  " }, st),
            Span::styled(
                format!("{:<8} ", file.quant.clone().unwrap_or_else(|| "?".into())),
                st,
            ),
            Span::styled(format!("{:>8}  ", human_size(file.size)), st),
        ];
        let dest = file.path.rsplit('/').next().unwrap_or(&file.path);
        if file.split {
            s.push(Span::styled(
                "split file, not supported",
                Style::default().fg(DIM),
            ));
        } else {
            s.extend(fit_span(file.fit));
            if file.quant.as_deref() == Some("Q4_K_M") {
                s.push(Span::styled(
                    "  Recommended",
                    Style::default().fg(Color::Green),
                ));
            }
            if ui.models.iter().any(|m| m.file == dest) {
                s.push(Span::styled(
                    "  On this computer",
                    Style::default().fg(Color::Green),
                ));
            } else if ui.downloads.iter().any(|x| x.dest_file == dest) {
                s.push(Span::styled("  Downloading", Style::default().fg(ACCENT)));
            }
        }
        s.push(Span::styled(
            format!("   {}", file.path),
            Style::default().fg(DIM),
        ));
        head.push(Line::from(s));
    }
    head.push(Line::from(""));
    downloads_lines(ui, &mut head, true);
    let head_h = (head.len() as u16 + 2).min(area.height / 2 + 4);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(head_h), Constraint::Min(3)])
        .split(area);
    f.render_widget(Paragraph::new(head).block(block(&d.id)), rows[0]);
    let w = rows[1].width.saturating_sub(2) as usize;
    let readme = d
        .readme
        .as_deref()
        .unwrap_or("This model has no description.");
    let lines = markdown(&crate::text::readable_card(readme), w, Style::default());
    f.render_widget(
        Paragraph::new(lines)
            .scroll((ui.hub_readme_scroll, 0))
            .block(block("Model card (PgUp/PgDn)")),
        rows[1],
    );
}

/// 324976 -> "324,976".
pub fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

pub fn human_params(p: u64) -> String {
    if p >= 1_000_000_000 {
        format!("{:.1}B", p as f64 / 1e9)
    } else if p >= 1_000_000 {
        format!("{}M", p / 1_000_000)
    } else {
        format!("{}K", (p / 1_000).max(1))
    }
}

// ----- about -----------------------------------------------------------------------

/// The licenses of what the terminal app ships (the desktop's font is not part of it).
pub fn tui_licenses() -> Vec<trurl_core::licenses::License> {
    trurl_core::licenses::all()
        .into_iter()
        .filter(|l| !l.name.contains("Sora"))
        .collect()
}

fn draw_about(f: &mut Frame, ui: &Ui, area: Rect) {
    let licenses = tui_licenses();
    if let Some(i) = ui.license_open {
        let lic = &licenses[i.min(licenses.len() - 1)];
        f.render_widget(
            Paragraph::new(lic.text.to_string())
                .wrap(Wrap { trim: false })
                .scroll((ui.about_scroll, 0))
                .block(block(&format!("{} (Esc to close, PgUp/PgDn)", lic.name))),
            area,
        );
        return;
    }
    let mut l: Vec<Line> = vec![
        Line::from(vec![
            Span::styled(
                "Trurl ",
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!("version {}", ui.version)),
        ]),
        Line::from(Span::styled(
            "by Cyberiad Labs",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(
            "Run open-weight language models on your own computer, without internet, and see how they work.",
        ),
        Line::from(""),
        Line::from(Span::styled(
            "Keys",
            Style::default().add_modifier(Modifier::BOLD),
        )),
    ];
    for (k, v) in [
        ("Enter", "send your message"),
        ("Alt+Enter, Shift+Enter, Ctrl+J", "new line"),
        ("Esc", "stop the answer"),
        ("Ctrl+N", "new chat"),
        ("Ctrl+T", "show or hide the model's thinking"),
        (
            "F5",
            "after running out of room: a bigger context, same question",
        ),
        ("F6", "copy the last answer (also saved in your notes file)"),
        ("Ctrl+R", "load the model again"),
        ("PgUp/PgDn", "scroll"),
        ("F2", "models: use, import, delete"),
        ("F3", "settings: system prompt, temperature, context size"),
        ("F4", "Hugging Face (needs internet)"),
        ("F12", "Hugging Face token (only for gated models)"),
        ("Ctrl+Q", "quit"),
    ] {
        l.push(Line::from(vec![
            Span::styled(format!("  {k:<32}"), Style::default().fg(ACCENT)),
            Span::raw(v),
        ]));
    }
    l.push(Line::from(""));
    if let Some(h) = &ui.hardware {
        let tier = match h.variant {
            CpuVariant::Avx2 => "AVX2",
            CpuVariant::Avx => "AVX",
            CpuVariant::Compat => "Compatible",
        };
        l.push(Line::from(format!(
            "This computer: {} of memory, {} cores, {} ({tier} build)",
            human_size(h.total_ram),
            h.physical_cores,
            h.cpu_name
        )));
    }
    l.push(Line::from(format!(
        "Model engine: llama.cpp {}",
        ui.llama_build
    )));
    l.push(Line::from(format!("Models folder: {}", ui.models_dir)));
    l.push(Line::from(""));
    l.push(Line::from(Span::styled(
        "Press U to check for updates.",
        Style::default().fg(DIM),
    )));
    l.push(Line::from(""));
    l.push(Line::from(Span::styled(
        "Licenses",
        Style::default().add_modifier(Modifier::BOLD),
    )));
    for (i, lic) in licenses.iter().enumerate() {
        let sel = i == ui.license_sel;
        l.push(Line::from(Span::styled(
            format!("{}{}", if sel { "› " } else { "  " }, lic.name),
            if sel {
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            },
        )));
    }
    f.render_widget(
        Paragraph::new(l)
            .wrap(Wrap { trim: false })
            .scroll((ui.about_scroll, 0))
            .block(block("About Trurl (PgUp/PgDn to scroll)")),
        area,
    );
}

fn draw_token(f: &mut Frame, ui: &Ui, area: Rect) {
    let has = ui
        .settings
        .as_ref()
        .map(|s| s.has_hf_token)
        .unwrap_or(false);
    let masked = "•".repeat(ui.token.text.chars().count());
    let l = vec![
        Line::from(Span::styled(
            "Hugging Face access token",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(
            "Only needed for models that ask you to accept their terms (gated models). It is sent only to huggingface.co.",
        ),
        Line::from(if has {
            "A token is saved."
        } else {
            "No token saved."
        }),
        Line::from(""),
        Line::from(vec![
            Span::raw("Token: "),
            Span::styled(masked, Style::default().fg(ACCENT)),
        ]),
    ];
    f.render_widget(
        Paragraph::new(l)
            .wrap(Wrap { trim: false })
            .block(block("Advanced settings")),
        area,
    );
}

fn draw_confirm(f: &mut Frame, c: &Confirm, area: Rect) {
    let (title, body, yes) = match c {
        Confirm::Delete { name, size, .. } => (
            "Delete model",
            format!(
                "Delete {name}? This frees {} on this computer. You can import or download it again later.",
                human_size(*size)
            ),
            "Enter: delete",
        ),
        Confirm::LoadRed { name, .. } => (
            "This model may be too big",
            format!(
                "{name} may be too big for this computer. It may be very slow or fail to load."
            ),
            "Enter: load anyway",
        ),
        Confirm::Update { version, notes } => {
            return draw_update(f, version, notes, area);
        }
        Confirm::NewChat => (
            "Start a new chat?",
            "The model is still answering. The answer so far will be lost.".to_string(),
            "Enter: start a new chat",
        ),
        Confirm::Quit => (
            "Quit Trurl?",
            "This chat will be lost. To keep an answer, press Esc, then F6 to copy it.".to_string(),
            "Enter: quit",
        ),
        Confirm::ClearSystemPrompt => (
            "Clear the system prompt?",
            "The model will get no instructions before your messages.".to_string(),
            "Enter: clear it",
        ),
        Confirm::DiscardSystemPrompt => (
            "Leave without saving?",
            "Your changes to the system prompt will be lost. Ctrl+S saves them.".to_string(),
            "Enter: leave without saving",
        ),
    };
    let r = centered(area, 64, 8);
    f.render_widget(Clear, r);
    let l = vec![
        Line::from(body),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                yes,
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            Span::raw("    Esc: cancel"),
        ]),
    ];
    f.render_widget(
        Paragraph::new(l)
            .wrap(Wrap { trim: false })
            .block(block(title).border_style(Style::default().fg(Color::Yellow))),
        r,
    );
}

fn draw_update(f: &mut Frame, version: &str, notes: &str, area: Rect) {
    let r = centered(area, 72, area.height.saturating_sub(4).clamp(8, 20));
    f.render_widget(Clear, r);
    let mut l: Vec<Line> = notes
        .lines()
        .map(|s| Line::from(crate::text::clean(s)))
        .collect();
    l.push(Line::from(""));
    l.push(Line::from(
        "Your models and settings are kept. Trurl starts again by itself.",
    ));
    l.push(Line::from(""));
    l.push(Line::from(vec![
        Span::styled(
            "Enter: update now",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::raw("    F8: later    Esc: close"),
    ]));
    f.render_widget(
        Paragraph::new(l).wrap(Wrap { trim: false }).block(
            block(&format!("Update available: version {version}"))
                .border_style(Style::default().fg(ACCENT)),
        ),
        r,
    );
}

fn draw_prompt(f: &mut Frame, kind: PromptKind, ed: &crate::text::Editor, area: Rect) {
    let (title, hint, h) = match kind {
        PromptKind::ImportPath => (
            "Import a model",
            "Type the full path of a .gguf file, for example /media/usb/models/model.gguf. Enter: import · Esc: cancel",
            7,
        ),
        PromptKind::SystemPrompt => (
            "System prompt",
            "The model reads this before every message. Ctrl+S: save · Enter: new line · Esc: cancel",
            14,
        ),
    };
    let r = centered(area, 80, h);
    f.render_widget(Clear, r);
    let inner_w = r.width.saturating_sub(2) as usize;
    let (rows, cur) = ed.layout(inner_w);
    let parts = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(2)])
        .split(Rect {
            x: r.x + 1,
            y: r.y + 1,
            width: r.width - 2,
            height: r.height - 2,
        });
    f.render_widget(block(title).border_style(Style::default().fg(ACCENT)), r);
    let visible = parts[0].height as usize;
    let skip = (cur.1 as usize + 1).saturating_sub(visible);
    f.render_widget(
        Paragraph::new(
            rows.iter()
                .skip(skip)
                .map(|s| Line::from(s.clone()))
                .collect::<Vec<_>>(),
        ),
        parts[0],
    );
    f.render_widget(
        Paragraph::new(Span::styled(hint, Style::default().fg(DIM)))
            .wrap(Wrap { trim: true })
            .alignment(Alignment::Left),
        parts[1],
    );
    f.set_cursor_position((parts[0].x + cur.0, parts[0].y + cur.1 - skip as u16));
}

#[cfg(test)]
mod tests {
    #[test]
    fn parameter_counts() {
        assert_eq!(super::human_params(751_632_384), "751M");
        assert_eq!(super::human_params(3_880_099_328), "3.9B");
        assert_eq!(super::human_params(292_096), "292K");
    }
}
