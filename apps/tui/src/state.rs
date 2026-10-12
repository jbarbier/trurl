//! Everything the terminal app shows, and how core events change it.

use crate::text::Editor;
use std::collections::HashSet;
use trurl_core::chat::{Turn, TurnStatus};
use trurl_core::downloads::DownloadInfo;
use trurl_core::errors::UserError;
use trurl_core::events::{EngineState, Event, HardwareSummary, PublicSettings, Snapshot};
use trurl_core::hub::{RepoFiles, RepoHit};
use trurl_core::models::ModelInfo;
use trurl_core::update::UpdateInfo;

#[derive(Debug, Clone, PartialEq)]
pub enum Screen {
    Chat,
    Settings,
    Models,
    Hub,
    About,
    /// Hidden: Hugging Face token.
    Token,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Confirm {
    Delete {
        file: String,
        name: String,
        size: u64,
    },
    LoadRed {
        file: String,
        name: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SettingsField {
    Model,
    SystemPrompt,
    Temperature,
    Context,
}

impl SettingsField {
    pub const ALL: [SettingsField; 4] = [
        SettingsField::Model,
        SettingsField::SystemPrompt,
        SettingsField::Temperature,
        SettingsField::Context,
    ];
}

#[derive(Debug, Clone, PartialEq)]
pub enum HubView {
    Checking,
    Offline,
    Search,
    Details,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HubFocus {
    Input,
    Results,
    Files,
    Downloads,
}

#[derive(Debug, Clone)]
pub enum UpdateState {
    None,
    Available(UpdateInfo),
    Downloading(UpdateInfo, u64, Option<u64>),
    Failed(UserError),
    Hidden(UpdateInfo),
}

pub struct Ui {
    pub version: String,
    pub llama_build: String,
    pub models_dir: String,
    pub hardware: Option<HardwareSummary>,
    pub settings: Option<PublicSettings>,
    pub models: Vec<ModelInfo>,
    pub engine: EngineState,
    pub turns: Vec<Turn>,
    pub generating: bool,
    pub downloads: Vec<DownloadInfo>,

    pub screen: Screen,
    pub input: Editor,
    /// Lines scrolled up from the bottom of the transcript; 0 follows new text.
    pub scroll: usize,
    pub notice: Option<UserError>,
    pub confirm: Option<Confirm>,
    /// A text prompt in a popup: (title, editor). Used for import paths and the system prompt.
    pub prompt: Option<(PromptKind, Editor)>,

    pub models_sel: usize,
    pub settings_sel: usize,
    pub pending_context: Option<u32>,
    pub import: Option<(String, u64, u64)>,
    pub verify: Option<(String, u64, u64)>,

    pub hub_view: HubView,
    pub hub_focus: HubFocus,
    pub hub_query: Editor,
    pub hub_results: Vec<RepoHit>,
    pub hub_sel: usize,
    pub hub_busy: bool,
    pub hub_details: Option<RepoFiles>,
    pub hub_file_sel: usize,
    pub hub_readme_scroll: u16,
    pub dl_sel: usize,

    pub about_scroll: u16,
    pub license_sel: usize,
    pub license_open: Option<usize>,
    pub update: UpdateState,
    pub update_msg: Option<String>,
    pub token: Editor,
    pub quit: bool,
    /// Turn ids whose thinking block the learner toggled away from the default.
    pub toggled: HashSet<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PromptKind {
    ImportPath,
    SystemPrompt,
}

impl Ui {
    pub fn new() -> Self {
        Ui {
            version: String::new(),
            llama_build: String::new(),
            models_dir: String::new(),
            hardware: None,
            settings: None,
            models: Vec::new(),
            engine: EngineState::Idle,
            turns: Vec::new(),
            generating: false,
            downloads: Vec::new(),
            screen: Screen::Chat,
            input: Editor::default(),
            scroll: 0,
            notice: None,
            confirm: None,
            prompt: None,
            models_sel: 0,
            settings_sel: 0,
            pending_context: None,
            import: None,
            verify: None,
            hub_view: HubView::Checking,
            hub_focus: HubFocus::Input,
            hub_query: Editor::default(),
            hub_results: Vec::new(),
            hub_sel: 0,
            hub_busy: false,
            hub_details: None,
            hub_file_sel: 0,
            hub_readme_scroll: 0,
            dl_sel: 0,
            about_scroll: 0,
            license_sel: 0,
            license_open: None,
            update: UpdateState::None,
            update_msg: None,
            token: Editor::default(),
            quit: false,
            toggled: HashSet::new(),
        }
    }

    pub fn apply_snapshot(&mut self, s: Snapshot) {
        self.version = s.version;
        self.llama_build = s.llama_build;
        self.models_dir = s.models_dir;
        self.hardware = Some(s.hardware);
        self.settings = Some(s.settings);
        self.models = s.models;
        self.engine = s.engine;
        self.turns = s.turns;
        self.generating = s.generating;
        self.downloads = s.downloads;
    }

    pub fn apply(&mut self, e: Event) {
        match e {
            Event::Snapshot { snapshot } => self.apply_snapshot(*snapshot),
            Event::Engine { engine } => self.engine = engine,
            Event::Models { models } => {
                self.models = models;
                self.models_sel = self.models_sel.min(self.models.len().saturating_sub(1));
            }
            Event::Settings { settings } => self.settings = Some(settings),
            Event::Generating { generating } => self.generating = generating,
            Event::TurnStarted { turn } => {
                self.turns.push(turn);
                self.scroll = 0;
            }
            Event::TurnDelta {
                id,
                reasoning,
                answer,
                reasoning_from_answer,
            } => {
                if let Some(t) = self.turns.iter_mut().find(|t| t.id == id) {
                    if reasoning_from_answer {
                        let a = std::mem::take(&mut t.answer);
                        t.reasoning = a + &t.reasoning;
                    }
                    if let Some(r) = reasoning {
                        t.reasoning.push_str(&r);
                    }
                    if let Some(a) = answer {
                        t.answer.push_str(&a);
                    }
                }
            }
            Event::TurnThought { id, ms } => {
                if let Some(t) = self.turns.iter_mut().find(|t| t.id == id) {
                    t.thought_ms = Some(ms);
                }
            }
            Event::TurnFinished { turn } => {
                if let Some(t) = self.turns.iter_mut().find(|t| t.id == turn.id) {
                    *t = turn;
                }
            }
            Event::ChatCleared => {
                self.turns.clear();
                self.scroll = 0;
                self.toggled.clear();
            }
            Event::ImportProgress { file, done, total } => self.import = Some((file, done, total)),
            Event::ImportFinished { error, .. } => {
                self.import = None;
                if let Some(e) = error {
                    self.notice = Some(e);
                }
            }
            Event::VerifyProgress { file, done, total } => {
                self.verify = if done >= total {
                    None
                } else {
                    Some((file, done, total))
                };
            }
            Event::Downloads { downloads } => {
                self.downloads = downloads;
                self.dl_sel = self.dl_sel.min(self.downloads.len().saturating_sub(1));
            }
            Event::Notice { error } => self.notice = Some(error),
        }
    }

    /// Is the thinking block of this turn open? Open while it streams, closed
    /// once the answer starts; the learner can flip any of them.
    pub fn thinking_open(&self, t: &Turn) -> bool {
        let default = t.status == TurnStatus::Streaming && t.answer.is_empty();
        default ^ self.toggled.contains(&t.id)
    }

    pub fn current_model(&self) -> Option<&ModelInfo> {
        let f = self.settings.as_ref()?.model.as_ref()?;
        self.models.iter().find(|m| &m.file == f)
    }

    pub fn engine_label(&self) -> (String, &'static str) {
        match &self.engine {
            EngineState::Idle => ("No model".into(), "idle"),
            EngineState::Loading { name, .. } => (format!("Loading {name}…"), "loading"),
            EngineState::Ready { name, .. } => {
                if self.generating {
                    let thinking = self
                        .turns
                        .last()
                        .map(|t| {
                            t.status == TurnStatus::Streaming
                                && t.answer.is_empty()
                                && !t.reasoning.is_empty()
                        })
                        .unwrap_or(false);
                    (
                        name.clone(),
                        if thinking { "thinking" } else { "answering" },
                    )
                } else {
                    (name.clone(), "ready")
                }
            }
            EngineState::Failed { name, .. } => (name.clone(), "failed"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(id: u64) -> Turn {
        Turn {
            id,
            user: "q".into(),
            reasoning: String::new(),
            answer: String::new(),
            model: "m".into(),
            status: TurnStatus::Streaming,
            thought_ms: None,
            error: None,
        }
    }

    #[test]
    fn deltas_build_the_turn_and_move_answer_to_reasoning() {
        let mut ui = Ui::new();
        ui.apply(Event::TurnStarted { turn: turn(1) });
        ui.apply(Event::TurnDelta {
            id: 1,
            reasoning: None,
            answer: Some("hmm ".into()),
            reasoning_from_answer: false,
        });
        ui.apply(Event::TurnDelta {
            id: 1,
            reasoning: Some("more".into()),
            answer: None,
            reasoning_from_answer: true,
        });
        ui.apply(Event::TurnDelta {
            id: 1,
            reasoning: None,
            answer: Some("42".into()),
            reasoning_from_answer: false,
        });
        assert_eq!(ui.turns[0].reasoning, "hmm more");
        assert_eq!(ui.turns[0].answer, "42");
    }

    #[test]
    fn thinking_opens_while_thinking_and_closes_when_answer_starts() {
        let mut ui = Ui::new();
        let mut t = turn(1);
        t.reasoning = "x".into();
        assert!(ui.thinking_open(&t));
        t.answer = "y".into();
        assert!(!ui.thinking_open(&t));
        ui.toggled.insert(1);
        assert!(ui.thinking_open(&t));
    }

    #[test]
    fn clearing_the_chat_resets_scroll_and_toggles() {
        let mut ui = Ui::new();
        ui.apply(Event::TurnStarted { turn: turn(1) });
        ui.scroll = 10;
        ui.toggled.insert(1);
        ui.apply(Event::ChatCleared);
        assert!(ui.turns.is_empty() && ui.scroll == 0 && ui.toggled.is_empty());
    }
}
