//! The pinned versions, compiled in from `pins.json` at the repo root.

use serde::Deserialize;
use std::sync::OnceLock;

#[derive(Debug, Clone, Deserialize)]
pub struct Pins {
    pub app: AppPin,
    pub llama_cpp: LlamaPin,
    pub models: Vec<ModelPin>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AppPin {
    pub name: String,
    pub version: String,
    pub identifier: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LlamaPin {
    pub release: String,
    pub build: String,
    pub commit: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModelPin {
    pub id: String,
    pub name: String,
    pub repo: String,
    pub revision: String,
    pub file: String,
    pub size: u64,
    pub sha256: String,
    pub license: String,
    pub license_file: String,
    pub default: bool,
}

pub const PINS_JSON: &str = include_str!("../../../pins.json");

pub fn pins() -> &'static Pins {
    static P: OnceLock<Pins> = OnceLock::new();
    P.get_or_init(|| {
        serde_json::from_str(PINS_JSON)
            .expect("pins.json is valid (checked by scripts/check_pins.py)")
    })
}

pub fn pinned_model(file: &str) -> Option<&'static ModelPin> {
    pins().models.iter().find(|m| m.file == file)
}

pub fn default_model() -> Option<&'static ModelPin> {
    pins().models.iter().find(|m| m.default)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pins_parse_and_have_one_default() {
        let p = pins();
        assert_eq!(p.app.name, "Trurl");
        assert_eq!(p.models.iter().filter(|m| m.default).count(), 1);
        assert_eq!(default_model().unwrap().file, "gemma-3-1b-it-Q4_K_M.gguf");
        for m in &p.models {
            assert_eq!(m.sha256.len(), 64, "{}", m.file);
            assert_eq!(m.revision.len(), 40, "{}", m.file);
        }
    }

    #[test]
    fn app_version_matches_cargo() {
        assert_eq!(pins().app.version, env!("CARGO_PKG_VERSION"));
    }
}
