//! Where Trurl keeps things.
//!
//! The data folder is deliberately NOT the Tauri app-data folder (which the
//! uninstaller's "delete app data" box removes) and NOT the install folder
//! (which updates replace). Models in it survive updates and uninstalls.
//!
//! - Windows: `%LOCALAPPDATA%\CyberiadLabs\Trurl`
//! - Linux:   `$XDG_DATA_HOME/trurl` (default `~/.local/share/trurl`)
//! - `TRURL_HOME` overrides both (tests, portable installs).

use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Paths {
    pub home: PathBuf,
}

impl Paths {
    pub fn from_env() -> Self {
        if let Some(h) = std::env::var_os("TRURL_HOME").filter(|v| !v.is_empty()) {
            return Self {
                home: PathBuf::from(h),
            };
        }
        #[cfg(windows)]
        let home = dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("CyberiadLabs")
            .join("Trurl");
        #[cfg(not(windows))]
        let home = dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("trurl");
        Self { home }
    }

    pub fn at(home: impl Into<PathBuf>) -> Self {
        Self { home: home.into() }
    }

    pub fn models(&self) -> PathBuf {
        self.home.join("models")
    }
    pub fn downloads(&self) -> PathBuf {
        self.home.join("downloads")
    }
    pub fn settings(&self) -> PathBuf {
        self.home.join("settings.json")
    }
    pub fn logs(&self) -> PathBuf {
        self.home.join("logs")
    }
    pub fn verified_cache(&self) -> PathBuf {
        self.home.join("verified.json")
    }

    pub fn ensure(&self) -> std::io::Result<()> {
        for d in [self.models(), self.downloads(), self.logs()] {
            std::fs::create_dir_all(d)?;
        }
        Ok(())
    }
}

/// Folder holding the llama-server builds (`avx2/`, `avx/`, `compat/`).
///
/// Looked up in order: `TRURL_LLAMA_DIR`, `<exe dir>/llama`, `<exe dir>/../llama`
/// (Tauri puts resources next to the exe on Windows), and the repo's
/// `vendor/llama/<os>` for development.
pub fn llama_root() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("TRURL_LLAMA_DIR").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(d));
    }
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let mut candidates = vec![dir.join("llama"), dir.join("resources").join("llama")];
    if let Some(up) = dir.parent() {
        candidates.push(up.join("llama"));
        candidates.push(up.join("lib").join("trurl").join("llama"));
    }
    // Development: target/{debug,release}/<exe> -> repo/vendor/llama/<os>
    for anc in dir.ancestors().take(5) {
        candidates.push(anc.join("vendor").join("llama").join(std::env::consts::OS));
    }
    candidates
        .into_iter()
        .find(|c| c.join("compat").is_dir() || c.join("avx2").is_dir())
}

pub fn server_exe_name() -> &'static str {
    if cfg!(windows) {
        "llama-server.exe"
    } else {
        "llama-server"
    }
}

/// A file name that is safe on every OS: no separators, no `..`, no reserved
/// Windows names, no control characters, ASCII only (llama.cpp and some Windows
/// tools still mishandle non-ASCII paths). Keeps the `.gguf` extension.
pub fn safe_model_file_name(original: &str) -> Option<String> {
    let base = original.rsplit(['/', '\\']).next()?.trim();
    let stem = base
        .strip_suffix(".gguf")
        .or_else(|| base.strip_suffix(".GGUF"))?;
    let mut out: String = stem
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '+') {
                c
            } else {
                '_'
            }
        })
        .collect();
    while out.contains("..") {
        out = out.replace("..", ".");
    }
    while out.contains("__") {
        out = out.replace("__", "_");
    }
    let out = out.trim_matches(['.', ' ', '_']).to_string();
    if out.is_empty() {
        return None;
    }
    // Windows treats "NUL", "CON", "COM1"... as devices even with an extension
    // ("nul.Q4_K_M.gguf"), so check the part before the first dot.
    let first = out.split('.').next().unwrap_or("").to_ascii_uppercase();
    let reserved = ["CON", "PRN", "AUX", "NUL"];
    let is_reserved = reserved.contains(&first.as_str())
        || ((first.starts_with("COM") || first.starts_with("LPT"))
            && first.len() == 4
            && first.as_bytes()[3].is_ascii_digit());
    let out = if is_reserved {
        format!("model_{out}")
    } else {
        out
    };
    let out: String = out.chars().take(150).collect();
    Some(format!("{out}.gguf"))
}

/// True when `p` sits directly inside `dir` (no traversal).
pub fn is_direct_child(dir: &Path, p: &Path) -> bool {
    p.parent().map(|parent| parent == dir).unwrap_or(false)
        && p.file_name()
            .map(|n| n != ".." && n != ".")
            .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout() {
        let p = Paths::at("/x");
        assert_eq!(p.models(), PathBuf::from("/x/models"));
        assert_eq!(p.downloads(), PathBuf::from("/x/downloads"));
    }

    #[test]
    fn safe_names() {
        assert_eq!(
            safe_model_file_name("gemma-3-1b-it-Q4_K_M.gguf").as_deref(),
            Some("gemma-3-1b-it-Q4_K_M.gguf")
        );
        assert_eq!(
            safe_model_file_name("sub/dir/Model.gguf").as_deref(),
            Some("Model.gguf")
        );
        assert_eq!(
            safe_model_file_name("..\\..\\evil.gguf").as_deref(),
            Some("evil.gguf")
        );
        assert_eq!(
            safe_model_file_name("Modèle façon été.gguf").as_deref(),
            Some("Mod_le_fa_on_t.gguf")
        );
        assert_eq!(
            safe_model_file_name("CON.gguf").as_deref(),
            Some("model_CON.gguf")
        );
        assert_eq!(
            safe_model_file_name("nul.Q4_K_M.gguf").as_deref(),
            Some("model_nul.Q4_K_M.gguf")
        );
        assert_eq!(
            safe_model_file_name("lpt3.gguf").as_deref(),
            Some("model_lpt3.gguf")
        );
        assert_eq!(
            safe_model_file_name("console.gguf").as_deref(),
            Some("console.gguf")
        );

        assert_eq!(
            safe_model_file_name("a..b.gguf").as_deref(),
            Some("a.b.gguf")
        );
        assert_eq!(safe_model_file_name("notes.txt"), None);
        assert_eq!(safe_model_file_name("...gguf"), None);
        assert_eq!(safe_model_file_name("X.GGUF").as_deref(), Some("X.gguf"));
    }

    #[test]
    fn direct_child() {
        let d = Path::new("/m");
        assert!(is_direct_child(d, Path::new("/m/a.gguf")));
        assert!(!is_direct_child(d, Path::new("/m/sub/a.gguf")));
        assert!(!is_direct_child(d, Path::new("/other/a.gguf")));
    }
}
