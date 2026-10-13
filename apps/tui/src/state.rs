//! Everything the terminal app shows, and how core events change it.

use crate::text::Editor;
use std::collections::HashSet;
use trurl_core::chat::{Turn, TurnStatus};
use trurl_core::downloads::DownloadInfo;
use trurl_core::errors::UserError;
use trurl_core::events::{
    CourseModel, EngineState, Event, HardwareSummary, PublicSettings, Snapshot,
};
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
    /// Ctrl+N while the model is still answering.
    NewChat,
    /// Ctrl+C or Ctrl+Q with a chat on screen.
    Quit,
    /// R on the system prompt in Settings.
    ClearSystemPrompt,
    /// Esc in the system prompt editor, with changes not saved.
    DiscardSystemPrompt,
    /// "Update available": the version and its full release notes.
    Update {
        version: String,
        notes: String,
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
    /// The course models, above the search.
    Course,
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
    Hidden,
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
    /// The models the course uses (pins.json), first on the Hugging Face screen.
    pub course_models: Vec<CourseModel>,

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
    pub course_sel: usize,

    pub about_scroll: u16,
    pub license_sel: usize,
    pub license_open: Option<usize>,
    pub update: UpdateState,
    pub update_msg: Option<String>,
    pub token: Editor,
    pub quit: bool,
    /// The learner pressed Enter while the model was loading: the message goes
    /// out as soon as it is ready (it stays in the box if loading fails).
    pub queued: bool,
    /// When `update_msg` was set: short messages ("Token saved.") fade after a while.
    pub msg_at: Option<std::time::Instant>,
    /// How far the chat can scroll up, as the last drawing measured it.
    pub max_scroll: std::cell::Cell<usize>,
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
            course_models: Vec::new(),
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
            course_sel: 0,
            about_scroll: 0,
            license_sel: 0,
            license_open: None,
            update: UpdateState::None,
            update_msg: None,
            token: Editor::default(),
            quit: false,
            queued: false,
            msg_at: None,
            max_scroll: std::cell::Cell::new(0),
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
        self.course_models = s.course_models;
    }

    /// The model in the folder that is this course model's exact pinned file.
    pub fn course_model_here(&self, c: &CourseModel) -> Option<&ModelInfo> {
        self.models.iter().find(|m| m.file == c.file && m.course)
    }

    /// Its download, while one is under way.
    pub fn course_download(&self, c: &CourseModel) -> Option<&DownloadInfo> {
        // Its pinned download only: a search download of another commit can share the name.
        self.downloads.iter().find(|d| {
            d.id == c.download_id && d.status != trurl_core::downloads::DownloadStatus::Done
        })
    }

    /// A different file under a course model's name (imported, or from another repo).
    pub fn course_clash(&self, c: &CourseModel) -> bool {
        self.course_model_here(c).is_none() && self.models.iter().any(|m| m.file == c.file)
    }

    /// Where the Hugging Face screen starts: on the first missing course model,
    /// otherwise in the search box.
    pub fn hub_start_focus(&self) -> HubFocus {
        if self.first_missing_course().is_some() {
            HubFocus::Course
        } else {
            HubFocus::Input
        }
    }

    pub fn first_missing_course(&self) -> Option<usize> {
        self.course_models
            .iter()
            .position(|c| self.course_model_here(c).is_none())
    }

    /// What Enter does on the selected course model, for the key bar; None when
    /// it only explains (in use, damaged, another file with its name, downloading).
    pub fn course_enter(&self) -> Option<&'static str> {
        let c = self.course_models.get(self.course_sel)?;
        if let Some(m) = self.course_model_here(c) {
            return (!m.broken && self.running_file() != Some(m.file.as_str())).then_some("use");
        }
        if self.course_clash(c) {
            return None;
        }
        match self.course_download(c).map(|d| d.status) {
            Some(trurl_core::downloads::DownloadStatus::Failed) => Some("try again"),
            Some(_) => None,
            None => Some("download"),
        }
    }

    /// The file the engine runs or loads: what "in use" means on every screen.
    pub fn running_file(&self) -> Option<&str> {
        match &self.engine {
            EngineState::Ready { file, .. } | EngineState::Loading { file, .. } => Some(file),
            _ => None,
        }
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
            Event::ImportFinished { file, error } => {
                self.import = None;
                if let Some(e) = error {
                    self.notice = Some(e);
                } else if let Some(f) = file {
                    self.notice = None;
                    self.say(if self.screen == Screen::Models {
                        format!("Imported {f}. It is in the list: Enter uses it.")
                    } else {
                        format!("Imported {f}. Press F2 to use it.")
                    });
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
                // Gone after the check: it is in the models folder now.
                let finished: Vec<String> = self
                    .downloads
                    .iter()
                    .filter(|old| {
                        old.status == trurl_core::downloads::DownloadStatus::Verifying
                            && !downloads.iter().any(|d| d.id == old.id)
                    })
                    .map(|old| old.dest_file.clone())
                    .collect();
                for f in finished {
                    let f = self
                        .models
                        .iter()
                        .rev()
                        .find(|m| m.file.starts_with(f.trim_end_matches(".gguf")))
                        .map(|m| m.file.clone())
                        .unwrap_or(f);
                    self.say(format!("Downloaded {f}. Press F2 to use it."));
                }
                self.downloads = downloads;
                self.dl_sel = self.dl_sel.min(self.downloads.len().saturating_sub(1));
            }
            Event::Notice { error } => self.notice = Some(error),
        }
    }

    /// A short message under the chat, faded after 10 seconds.
    pub fn say(&mut self, m: impl Into<String>) {
        self.update_msg = Some(m.into());
        self.msg_at = Some(std::time::Instant::now());
    }

    /// Ctrl+T: open every thinking block in the chat, or close them all if
    /// they are all open already.
    pub fn toggle_all_thinking(&mut self) {
        let with: Vec<Turn> = self
            .turns
            .iter()
            .filter(|t| !t.reasoning.is_empty())
            .cloned()
            .collect();
        let open = !with.iter().all(|t| self.thinking_open(t));
        for t in &with {
            if self.thinking_open(t) != open && !self.toggled.remove(&t.id) {
                self.toggled.insert(t.id);
            }
        }
    }

    /// Is the thinking block of this turn open? Open while it streams, closed
    /// once the answer starts; the learner can flip any of them.
    pub fn thinking_open(&self, t: &Turn) -> bool {
        // Open while it streams, and when the context ran out while thinking:
        // then the reasoning is all there is to read.
        let ran_out = t.status == TurnStatus::Cut
            || t.error
                .as_ref()
                .is_some_and(|e| e.code == trurl_core::ErrorCode::ContextFull);
        let default = t.answer.is_empty() && (t.status == TurnStatus::Streaming || ran_out);
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
                    // Until the first answer word, the model is reading or thinking.
                    // Only a reasoning model thinks; the others are reading the question.
                    let kind = match self.turns.last() {
                        Some(t) if !t.answer.is_empty() => "answering",
                        Some(t) if !t.reasoning.is_empty() => "thinking",
                        _ => "starting",
                    };
                    (name.clone(), kind)
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
            system_prompt: String::new(),
            temperature: 0.7,
            context: 2048,
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
