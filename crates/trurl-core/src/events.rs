//! What the core tells the front ends. This enum IS the contract: the desktop
//! UI receives these as JSON (see `contracts/events.md`), the terminal app
//! matches on them directly.

use crate::chat::Turn;
use crate::downloads::DownloadInfo;
use crate::errors::UserError;
use crate::hardware::{CpuVariant, RamFit};
use crate::models::ModelInfo;
use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum EngineState {
    /// No model selected (empty models folder, or the loaded one was deleted).
    Idle,
    Loading {
        file: String,
        name: String,
    },
    Ready {
        file: String,
        name: String,
        context: u32,
    },
    Failed {
        file: String,
        name: String,
        error: UserError,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct PublicSettings {
    pub system_prompt: String,
    pub temperature: f32,
    pub context_size: u32,
    pub model: Option<String>,
    pub has_hf_token: bool,
    pub default_temperature: f32,
    pub default_context: u32,
    pub context_choices: Vec<u32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HardwareSummary {
    pub total_ram: u64,
    pub physical_cores: usize,
    pub threads: usize,
    pub cpu_name: String,
    pub variant: CpuVariant,
}

#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    pub version: String,
    pub llama_build: String,
    pub models_dir: String,
    pub hardware: HardwareSummary,
    pub settings: PublicSettings,
    pub models: Vec<ModelInfo>,
    pub engine: EngineState,
    pub turns: Vec<Turn>,
    pub generating: bool,
    pub downloads: Vec<DownloadInfo>,
    /// The models the course uses, in course order. Fixed for a given Trurl version.
    pub course_models: Vec<CourseModel>,
}

/// A course model: one click downloads exactly this pinned file
/// (`download_course_model`), so nobody picks a look-alike on Hugging Face.
#[derive(Debug, Clone, Serialize)]
pub struct CourseModel {
    pub id: String,
    pub name: String,
    /// What the course uses it for, in one line.
    pub note: String,
    /// The file name it gets in the models folder.
    pub file: String,
    pub size: u64,
    pub fit: RamFit,
    /// The id its pinned download gets. A front end matches downloads on this,
    /// never on the file name: a search download of another commit can share it.
    pub download_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct LoadWarning {
    pub file: String,
    pub name: String,
    pub fit: RamFit,
    pub size: u64,
    pub total_ram: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    Snapshot {
        snapshot: Box<Snapshot>,
    },
    Engine {
        engine: EngineState,
    },
    Models {
        models: Vec<ModelInfo>,
    },
    Settings {
        settings: PublicSettings,
    },
    TurnStarted {
        turn: Turn,
    },
    /// Text to append. `reasoning_from_answer`: move the answer so far into the reasoning block first.
    TurnDelta {
        id: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        reasoning: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        answer: Option<String>,
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        reasoning_from_answer: bool,
    },
    /// The first answer token arrived after `ms` of reasoning: collapse the block.
    TurnThought {
        id: u64,
        ms: u64,
    },
    TurnFinished {
        turn: Turn,
    },
    ChatCleared,
    Generating {
        generating: bool,
    },
    ImportProgress {
        file: String,
        done: u64,
        total: u64,
    },
    ImportFinished {
        file: Option<String>,
        error: Option<UserError>,
    },
    /// Checking a bundled model's SHA-256 against pins.json.
    VerifyProgress {
        file: String,
        done: u64,
        total: u64,
    },
    Downloads {
        downloads: Vec<DownloadInfo>,
    },
    /// Something failed outside a chat turn (delete, import, download…).
    Notice {
        error: UserError,
    },
}
