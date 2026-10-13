//! Learner settings, saved as JSON next to the models folder.
//!
//! Saved settings survive restarts, so a temperature set in one class is still
//! there in the next. The UI shows a "Reset" next to each control that differs
//! from the default.

use serde::{Deserialize, Serialize};
use std::path::Path;

pub const DEFAULT_TEMPERATURE: f32 = 0.7;
pub const DEFAULT_CONTEXT: u32 = 2048;
/// llama.cpp rounds the context up to a multiple of 256, so these are the sizes
/// that actually happen. 256 is the smallest useful one.
pub const CONTEXT_CHOICES: [u32; 6] = [256, 512, 1024, 2048, 4096, 8192];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub system_prompt: String,
    pub temperature: f32,
    pub context_size: u32,
    /// File name inside the models folder.
    pub model: Option<String>,
    /// Optional Hugging Face token for gated models. Sent only to huggingface.co.
    pub hf_token: String,
    /// Highest llama-server build allowed, lowered after an illegal-instruction crash.
    pub max_variant: Option<crate::hardware::CpuVariant>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            system_prompt: String::new(),
            temperature: DEFAULT_TEMPERATURE,
            context_size: DEFAULT_CONTEXT,
            model: None,
            hf_token: String::new(),
            max_variant: None,
        }
    }
}

impl Settings {
    /// Read settings; a missing or damaged file gives defaults instead of an error,
    /// so a bad file can never stop the app from starting.
    pub fn load(path: &Path) -> Self {
        let s: Settings = std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        s.normalized()
    }

    /// Write atomically: a crash mid-write leaves the old file intact.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        let bytes = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        // The file may hold a Hugging Face token: only this account may read it.
        #[cfg(unix)]
        {
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            let _ = std::fs::remove_file(&tmp);
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&tmp)?;
            f.write_all(&bytes)?;
            f.sync_all()?;
        }
        #[cfg(not(unix))]
        std::fs::write(&tmp, &bytes)?;
        std::fs::rename(&tmp, path)
    }

    pub fn normalized(mut self) -> Self {
        self.temperature = clamp_temperature(self.temperature);
        self.context_size = clamp_context(self.context_size);
        self
    }
}

/// 0.0 to 2.0 in steps of 0.1. NaN becomes the default.
pub fn clamp_temperature(t: f32) -> f32 {
    if !t.is_finite() {
        return DEFAULT_TEMPERATURE;
    }
    (t.clamp(0.0, 2.0) * 10.0).round() / 10.0
}

/// Snap to the closest allowed size at or above the request.
pub fn clamp_context(c: u32) -> u32 {
    CONTEXT_CHOICES
        .iter()
        .copied()
        .find(|&x| x >= c)
        .unwrap_or(*CONTEXT_CHOICES.last().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn the_settings_file_is_private_on_linux() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        Settings::default().save(&path).unwrap();
        Settings::default().save(&path).unwrap(); // a second save replaces it, still private
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn defaults_match_the_spec() {
        let s = Settings::default();
        assert_eq!(s.temperature, 0.7);
        assert_eq!(s.context_size, 2048);
        assert!(s.system_prompt.is_empty());
    }

    #[test]
    fn temperature_is_clamped_and_snapped() {
        assert_eq!(clamp_temperature(-1.0), 0.0);
        assert_eq!(clamp_temperature(3.0), 2.0);
        assert_eq!(clamp_temperature(0.74), 0.7);
        assert_eq!(clamp_temperature(0.75), 0.8);
        assert_eq!(clamp_temperature(f32::NAN), 0.7);
    }

    #[test]
    fn context_snaps_up() {
        assert_eq!(clamp_context(200), 256);
        assert_eq!(clamp_context(2048), 2048);
        assert_eq!(clamp_context(3000), 4096);
        assert_eq!(clamp_context(1_000_000), 8192);
        assert_eq!(clamp_context(0), 256);
    }

    #[test]
    fn roundtrip_and_damaged_file() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("settings.json");
        assert_eq!(Settings::load(&p), Settings::default());
        let s = Settings {
            system_prompt: "Be kind.".into(),
            temperature: 0.0,
            model: Some("a.gguf".into()),
            ..Default::default()
        };
        s.save(&p).unwrap();
        assert_eq!(Settings::load(&p), s);
        std::fs::write(&p, "{not json").unwrap();
        assert_eq!(Settings::load(&p), Settings::default());
        std::fs::write(&p, r#"{"temperature": 9, "context_size": 100}"#).unwrap();
        let l = Settings::load(&p);
        assert_eq!((l.temperature, l.context_size), (2.0, 256));
    }
}
