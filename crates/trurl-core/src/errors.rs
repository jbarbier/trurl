//! Every failure a learner can see, in plain language.
//!
//! Each error has a short title (what happened) and a hint (what to do next).
//! `detail` keeps the technical message for the log and the "Details" toggle,
//! but is never the main text. The copy follows the voice rules in CLAUDE.md
//! and is checked by `scripts/check_copy.py`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    NoModel,
    ModelTooBig,
    ModelCorrupt,
    ModelUnsupported,
    ModelMissing,
    ServerMissing,
    ServerStart,
    ServerCrashed,
    ContextFull,
    Busy,
    Offline,
    HubFailed,
    HubGated,
    DownloadFailed,
    /// The server answered, but not with the file (a filter page, a proxy that
    /// mangles ranges). Retried a few times, then reported.
    DownloadBlocked,
    /// Writing the download to disk failed (not "disk full").
    DownloadSave,
    DownloadCorrupt,
    DiskFull,
    ImportFailed,
    NotGguf,
    DeleteFailed,
    UpdateFailed,
    UpdateCheckFailed,
    UpdateBusy,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{title} {hint}")]
pub struct UserError {
    pub code: ErrorCode,
    pub title: String,
    pub hint: String,
    /// Technical detail for the log and the "Details" toggle. Never the headline.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl UserError {
    pub fn new(code: ErrorCode, title: impl Into<String>, hint: impl Into<String>) -> Self {
        Self {
            code,
            title: title.into(),
            hint: hint.into(),
            detail: None,
        }
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        let d = detail.into();
        if !d.trim().is_empty() {
            self.detail = Some(d);
        }
        self
    }

    pub fn no_model() -> Self {
        Self::new(
            ErrorCode::NoModel,
            "No model is loaded.",
            "Pick a model in the list on the left. If the list is empty, import a model file or download one.",
        )
    }

    pub fn model_too_big(model: &str) -> Self {
        Self::new(
            ErrorCode::ModelTooBig,
            format!("{model} is too big for this computer's memory."),
            "Close other apps and try again, or pick a smaller model (green dot) and a smaller context size.",
        )
    }

    pub fn model_corrupt(model: &str) -> Self {
        Self::new(
            ErrorCode::ModelCorrupt,
            format!("{model} looks damaged or incomplete."),
            "Delete it and copy or download it again. Files copied from a USB stick break if the stick is removed too early.",
        )
    }

    pub fn model_unsupported(model: &str) -> Self {
        Self::new(
            ErrorCode::ModelUnsupported,
            format!("{model} uses a kind of model this version of Trurl cannot run."),
            "Pick another model. A newer Trurl version may support it.",
        )
    }

    pub fn model_missing(file: &str) -> Self {
        Self::new(
            ErrorCode::ModelMissing,
            format!("The file {file} is not in the models folder any more."),
            "Pick another model, or import the file again.",
        )
    }

    pub fn server_missing() -> Self {
        Self::new(
            ErrorCode::ServerMissing,
            "Part of Trurl is missing: the program that runs the models.",
            "Install Trurl again. Your models stay where they are.",
        )
    }

    pub fn server_start() -> Self {
        Self::new(
            ErrorCode::ServerStart,
            "The model could not start.",
            "Try again. If it keeps failing, restart the computer, or pick a smaller model.",
        )
    }

    pub fn server_crashed() -> Self {
        Self::new(
            ErrorCode::ServerCrashed,
            "The model stopped working.",
            "Trurl will load it again. Send your message again when it says Ready. If this repeats, the model may be too big for this computer.",
        )
    }

    pub fn context_full() -> Self {
        Self::new(
            ErrorCode::ContextFull,
            "The conversation ran out of room.",
            "Start a new chat or increase the context size.",
        )
    }

    pub fn busy() -> Self {
        Self::new(
            ErrorCode::Busy,
            "Trurl is still busy with the last request.",
            "Wait for it to finish, or press Stop.",
        )
    }

    pub fn offline() -> Self {
        Self::new(
            ErrorCode::Offline,
            "Connect to the internet to browse Hugging Face.",
            "Everything else in Trurl works without internet.",
        )
    }

    pub fn hub_failed() -> Self {
        Self::new(
            ErrorCode::HubFailed,
            "Hugging Face did not answer.",
            "Check the internet connection and try again in a minute.",
        )
    }

    pub fn hub_gated(repo: &str) -> Self {
        Self::new(
            ErrorCode::HubGated,
            format!("{repo} needs a Hugging Face account."),
            "Accept the model's terms on huggingface.co, then add your access token in Trurl's hidden settings (Ctrl+Shift+S).",
        )
    }

    pub fn download_failed() -> Self {
        Self::new(
            ErrorCode::DownloadFailed,
            "The download stopped.",
            "Press Resume when the internet is back. Trurl keeps what was already downloaded.",
        )
    }

    pub fn download_blocked() -> Self {
        Self::new(
            ErrorCode::DownloadBlocked,
            "Hugging Face sent something that is not the model file.",
            "A school or office network may block downloads. Try another connection, or download at home and import the file.",
        )
    }

    pub fn download_save_failed() -> Self {
        Self::new(
            ErrorCode::DownloadSave,
            "Trurl could not save the download.",
            "Check that the disk or USB stick is still there and has room, then press Try again.",
        )
    }

    /// The secure connection failed: on these laptops, most often a wrong clock.
    pub fn hub_tls() -> Self {
        Self::new(
            ErrorCode::HubFailed,
            "Trurl could not make a secure connection to Hugging Face.",
            "Check that the computer's date and time are right. A school network may also block it.",
        )
    }

    pub fn download_corrupt(file: &str) -> Self {
        Self::new(
            ErrorCode::DownloadCorrupt,
            format!("{file} did not download correctly."),
            "Trurl deleted the broken copy. Start the download again.",
        )
    }

    pub fn disk_full() -> Self {
        Self::new(
            ErrorCode::DiskFull,
            "There is not enough free space on this computer.",
            "Delete a model you do not use, or free some space, then try again.",
        )
    }

    pub fn import_failed() -> Self {
        Self::new(
            ErrorCode::ImportFailed,
            "The model could not be copied.",
            "Check that the USB stick is still plugged in, then try again.",
        )
    }

    pub fn not_gguf(file: &str) -> Self {
        Self::new(
            ErrorCode::NotGguf,
            format!("{file} is not a model file Trurl can use."),
            "Trurl uses .gguf files. Pick a file whose name ends in .gguf.",
        )
    }

    pub fn delete_failed(file: &str) -> Self {
        Self::new(
            ErrorCode::DeleteFailed,
            format!("{file} could not be deleted."),
            "Pick another model first, then delete this one.",
        )
    }

    pub fn update_failed() -> Self {
        Self::new(
            ErrorCode::UpdateFailed,
            "The update did not finish.",
            "Nothing changed. Trurl will try again next time it checks, or use Check for updates in About.",
        )
    }

    pub fn update_check_failed() -> Self {
        Self::new(
            ErrorCode::UpdateCheckFailed,
            "Trurl could not check for updates.",
            "Connect to the internet and try again. Trurl works the same without updates.",
        )
    }

    pub fn update_wait_for_answer() -> Self {
        Self::new(
            ErrorCode::UpdateBusy,
            "Trurl is writing an answer.",
            "Wait until the answer is finished, or press Stop, then click Update now again.",
        )
    }

    pub fn installing_update() -> Self {
        Self::new(
            ErrorCode::UpdateBusy,
            "Trurl is installing an update.",
            "It starts again in a moment, with your models and settings.",
        )
    }

    pub fn internal(detail: impl Into<String>) -> Self {
        Self::new(
            ErrorCode::Internal,
            "Something went wrong inside Trurl.",
            "Try again. If it keeps happening, restart Trurl.",
        )
        .with_detail(detail)
    }

    /// Map an I/O error during a write to the most useful learner message.
    pub fn from_io_write(e: &std::io::Error, fallback: UserError) -> Self {
        if is_disk_full(e) {
            Self::disk_full().with_detail(e.to_string())
        } else {
            fallback.with_detail(e.to_string())
        }
    }
}

/// An error and all its causes on one line, for the "Details" a teacher may read.
pub fn chain(e: &(dyn std::error::Error + 'static)) -> String {
    let mut s = e.to_string();
    let mut cur = e.source();
    while let Some(c) = cur {
        s.push_str(": ");
        s.push_str(&c.to_string());
        cur = c.source();
    }
    s
}

/// ENOSPC on Unix (28), ERROR_DISK_FULL (112) and ERROR_HANDLE_DISK_FULL (39) on Windows.
pub fn is_disk_full(e: &std::io::Error) -> bool {
    if e.kind() == std::io::ErrorKind::StorageFull {
        return true;
    }
    match e.raw_os_error() {
        #[cfg(unix)]
        Some(28) => true,
        #[cfg(windows)]
        Some(112) | Some(39) => true,
        _ => false,
    }
}

pub type Result<T> = std::result::Result<T, UserError>;

#[cfg(test)]
mod tests {
    use super::*;

    fn all() -> Vec<UserError> {
        vec![
            UserError::no_model(),
            UserError::model_too_big("M"),
            UserError::model_corrupt("M"),
            UserError::model_unsupported("M"),
            UserError::model_missing("f.gguf"),
            UserError::server_missing(),
            UserError::server_start(),
            UserError::server_crashed(),
            UserError::context_full(),
            UserError::busy(),
            UserError::offline(),
            UserError::hub_failed(),
            UserError::hub_gated("a/b"),
            UserError::download_failed(),
            UserError::download_corrupt("f.gguf"),
            UserError::download_blocked(),
            UserError::download_save_failed(),
            UserError::hub_tls(),
            UserError::installing_update(),
            UserError::disk_full(),
            UserError::import_failed(),
            UserError::not_gguf("f.txt"),
            UserError::delete_failed("f.gguf"),
            UserError::update_failed(),
            UserError::update_check_failed(),
            UserError::update_wait_for_answer(),
            UserError::internal("x"),
        ]
    }

    #[test]
    fn every_error_has_a_title_and_a_next_step() {
        for e in all() {
            assert!(e.title.ends_with('.'), "{:?}", e.code);
            assert!(e.hint.len() > 10, "{:?} needs a real hint", e.code);
            assert!(
                !e.title.contains('—') && !e.hint.contains('—'),
                "no em dashes: {:?}",
                e.code
            );
        }
    }

    #[test]
    fn context_full_matches_the_course_wording() {
        let e = UserError::context_full();
        assert_eq!(
            format!("{} {}", e.title, e.hint),
            "The conversation ran out of room. Start a new chat or increase the context size."
        );
    }

    #[test]
    fn empty_detail_is_dropped() {
        assert_eq!(UserError::busy().with_detail("  ").detail, None);
        assert_eq!(
            UserError::busy().with_detail("x").detail.as_deref(),
            Some("x")
        );
    }

    #[test]
    fn serializes_code_in_snake_case() {
        let v = serde_json::to_value(UserError::context_full()).unwrap();
        assert_eq!(v["code"], "context_full");
        assert!(v.get("detail").is_none());
    }
}
