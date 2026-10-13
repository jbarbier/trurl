//! The models folder: listing, naming, import, delete, verification.

use crate::errors::{Result, UserError};
use crate::gguf::{self, GgufError, GgufInfo};
use crate::hardware::{RamFit, ram_fit};
use crate::pins::pinned_model;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

#[derive(Debug, Clone, Serialize)]
pub struct ModelInfo {
    /// File name inside the models folder: the model's id everywhere in Trurl.
    pub file: String,
    pub name: String,
    pub quant: Option<String>,
    pub size: u64,
    pub fit: RamFit,
    /// Shipped with Trurl and checked against its pinned SHA-256.
    pub bundled: bool,
    /// Some(true) once the SHA-256 matched the pin, Some(false) if it did not.
    pub verified: Option<bool>,
    /// The header could not be read: the file is damaged or not a model.
    pub broken: bool,
    pub has_chat_template: bool,
    pub reasoning_hint: bool,
    pub train_context: Option<u64>,
    #[serde(skip)]
    pub gguf: Option<GgufInfo>,
}

/// List every usable model in `dir`, sorted with the default model first, then by size.
pub fn scan(dir: &Path, total_ram: u64, verified: &VerifiedCache) -> Vec<ModelInfo> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for e in rd.flatten() {
        let p = e.path();
        let Some(file) = p.file_name().and_then(|n| n.to_str()).map(str::to_string) else {
            continue;
        };
        if !file.to_ascii_lowercase().ends_with(".gguf") || file.starts_with('.') {
            continue;
        }
        // Projector files for image input are not chat models; later split parts are not listed.
        if file.to_ascii_lowercase().starts_with("mmproj") || GgufInfo::is_secondary_split(&file) {
            continue;
        }
        // Follow symlinks: a teacher may link a model instead of copying it.
        let Ok(meta) = std::fs::metadata(&p) else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        out.push(describe(&p, &file, meta.len(), total_ram, verified));
    }
    out.sort_by(|a, b| {
        let da = pinned_model(&a.file).map(|m| !m.default).unwrap_or(true);
        let db = pinned_model(&b.file).map(|m| !m.default).unwrap_or(true);
        da.cmp(&db)
            .then(a.size.cmp(&b.size))
            .then(a.file.cmp(&b.file))
    });
    out
}

fn describe(
    path: &Path,
    file: &str,
    size: u64,
    total_ram: u64,
    verified: &VerifiedCache,
) -> ModelInfo {
    let header = gguf::read(path);
    let pin = pinned_model(file);
    let (gguf, broken) = match header {
        Ok(g) => (Some(g), false),
        Err(_) => (None, true),
    };
    let name = match (pin, &gguf) {
        (Some(p), _) => p.name.clone(),
        (None, Some(g)) => display_name(g, file),
        (None, None) => file.trim_end_matches(".gguf").to_string(),
    };
    let quant = quant_from_name(file);
    let bundled = pin.map(|p| p.size == size).unwrap_or(false);
    let v = if bundled { verified.get(path) } else { None };
    ModelInfo {
        file: file.to_string(),
        name,
        quant,
        size,
        fit: ram_fit(size, total_ram),
        bundled,
        verified: v,
        broken: broken || v == Some(false),
        has_chat_template: gguf.as_ref().map(|g| g.has_chat_template).unwrap_or(false),
        reasoning_hint: gguf
            .as_ref()
            .map(|g| g.template_mentions_thinking)
            .unwrap_or(false),
        train_context: gguf.as_ref().and_then(|g| g.context_length),
        gguf,
    }
}

/// "Gemma 3 1b It" style names are ugly; prefer general.name, fall back to the file name.
pub fn display_name(g: &GgufInfo, file: &str) -> String {
    let from_file = || file.trim_end_matches(".gguf").replace(['_', '-'], " ");
    let base = match g.name.as_deref().map(str::trim) {
        // A "name" that only repeats the architecture ("llama") says nothing.
        Some(n)
            if g.architecture
                .as_deref()
                .is_some_and(|a| a.eq_ignore_ascii_case(n)) =>
        {
            from_file()
        }
        // Some conversion scripts name the model after its folder: "Models Qwen Qwen3 0.6B".
        Some(n) if n.starts_with("Models ") && n.len() > 7 => n[7..].to_string(),
        Some(n) => n.to_string(),
        None => from_file(),
    };
    match &g.size_label {
        Some(l) if !base.to_ascii_lowercase().contains(&l.to_ascii_lowercase()) => {
            format!("{base} {l}")
        }
        _ => base,
    }
}

/// Pull the quantization tag out of a GGUF file name: Q4_K_M, Q8_0, IQ4_XS, F16, BF16, MXFP4…
pub fn quant_from_name(file: &str) -> Option<String> {
    let stem = file.trim_end_matches(".gguf").trim_end_matches(".GGUF");
    let parts: Vec<&str> = stem.split(['-', '.']).collect();
    for part in parts.iter().rev() {
        let up = part.to_ascii_uppercase();
        // Tokens like "Q4_K_M" survive the split on '-' and '.'.
        if is_quant(&up) {
            return Some(up);
        }
        // "model_Q4_K_M" (underscore-joined)
        if let Some(i) = up
            .find("_Q")
            .or_else(|| up.find("_IQ"))
            .or_else(|| up.find("_F16"))
            .or_else(|| up.find("_BF16"))
        {
            let cand = &up[i + 1..];
            if is_quant(cand) {
                return Some(cand.to_string());
            }
        }
    }
    None
}

fn is_quant(s: &str) -> bool {
    matches!(s, "F32" | "F16" | "BF16" | "MXFP4" | "NVFP4")
        || ((s.starts_with('Q') || s.starts_with("IQ") || s.starts_with("TQ"))
            && s.trim_start_matches(['I', 'T', 'Q'])
                .chars()
                .next()
                .map(|c| c.is_ascii_digit())
                .unwrap_or(false)
            && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            && s.len() <= 8)
}

/// Find a free destination name: `name.gguf`, `name-2.gguf`, …
pub fn unique_dest(dir: &Path, file: &str) -> PathBuf {
    let p = dir.join(file);
    if !p.exists() {
        return p;
    }
    let stem = file.trim_end_matches(".gguf");
    (2..)
        .map(|i| dir.join(format!("{stem}-{i}.gguf")))
        .find(|p| !p.exists())
        .unwrap()
}

/// Copy `src` into `dir` without ever leaving a half-written model behind:
/// write to `.part`, flush, then rename. Calls `progress(done, total)` as it goes.
/// Returns the new file name, or the existing one if an identical-size copy is there.
pub fn import(src: &Path, dir: &Path, mut progress: impl FnMut(u64, u64)) -> Result<String> {
    let fname = src
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("model.gguf")
        .to_string();
    let safe =
        crate::paths::safe_model_file_name(&fname).ok_or_else(|| UserError::not_gguf(&fname))?;
    match gguf::read(src) {
        Ok(_) => {}
        Err(GgufError::NotGguf) => return Err(UserError::not_gguf(&fname)),
        Err(GgufError::Io(e)) => return Err(UserError::import_failed().with_detail(e)),
        Err(e) => return Err(UserError::model_corrupt(&fname).with_detail(e.to_string())),
    }
    let total = std::fs::metadata(src)
        .map_err(|e| UserError::import_failed().with_detail(e.to_string()))?
        .len();
    std::fs::create_dir_all(dir)
        .map_err(|e| UserError::from_io_write(&e, UserError::import_failed()))?;
    let existing = dir.join(&safe);
    if existing.exists()
        && std::fs::metadata(&existing)
            .map(|m| m.len() == total)
            .unwrap_or(false)
    {
        progress(total, total);
        return Ok(safe);
    }
    if let Some(free) = crate::hardware::free_space(dir)
        && free < total + total / 20
    {
        return Err(UserError::disk_full());
    }
    let dest = unique_dest(dir, &safe);
    let part = dest.with_extension("gguf.part");
    let r = (|| -> std::io::Result<()> {
        let mut input = std::fs::File::open(src)?;
        let mut out = std::fs::File::create(&part)?;
        let mut buf = vec![0u8; 4 * 1024 * 1024];
        let mut done = 0u64;
        loop {
            let n = input.read(&mut buf)?;
            if n == 0 {
                break;
            }
            out.write_all(&buf[..n])?;
            done += n as u64;
            progress(done, total);
        }
        out.sync_all()?;
        if done != total {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "source changed while copying",
            ));
        }
        Ok(())
    })();
    if let Err(e) = r {
        let _ = std::fs::remove_file(&part);
        return Err(UserError::from_io_write(&e, UserError::import_failed()));
    }
    std::fs::rename(&part, &dest).map_err(|e| {
        let _ = std::fs::remove_file(&part);
        UserError::import_failed().with_detail(e.to_string())
    })?;
    Ok(dest.file_name().unwrap().to_string_lossy().into_owned())
}

/// Delete with a short retry: on Windows a file stays locked for a moment after
/// llama-server exits, and antivirus scanners hold new files briefly.
pub fn delete(dir: &Path, file: &str) -> Result<()> {
    let p = dir.join(file);
    if !crate::paths::is_direct_child(dir, &p) {
        return Err(UserError::delete_failed(file));
    }
    let mut last = None;
    for i in 0..10 {
        match std::fs::remove_file(&p) {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => last = Some(e),
        }
        std::thread::sleep(std::time::Duration::from_millis(100 * (i + 1)));
    }
    Err(UserError::delete_failed(file).with_detail(last.map(|e| e.to_string()).unwrap_or_default()))
}

/// SHA-256 of a file, reporting progress.
pub fn sha256_file(path: &Path, mut progress: impl FnMut(u64)) -> std::io::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 4 * 1024 * 1024];
    let mut done = 0u64;
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
        done += n as u64;
        progress(done);
    }
    Ok(hex::encode(h.finalize()))
}

/// Remembers which bundled files already matched their pinned SHA-256, keyed by
/// name + size + modification time, so a 2.5 GB file is hashed once, not at every start.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct VerifiedCache {
    entries: HashMap<String, (u64, u64, bool)>,
    #[serde(skip)]
    path: Option<PathBuf>,
}

impl VerifiedCache {
    pub fn load(path: &Path) -> Self {
        let mut c: VerifiedCache = std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        c.path = Some(path.to_path_buf());
        c
    }

    fn key(p: &Path) -> Option<(String, u64, u64)> {
        let m = std::fs::metadata(p).ok()?;
        let mtime = m
            .modified()
            .ok()?
            .duration_since(UNIX_EPOCH)
            .ok()?
            .as_secs();
        Some((
            p.file_name()?.to_string_lossy().into_owned(),
            m.len(),
            mtime,
        ))
    }

    pub fn get(&self, p: &Path) -> Option<bool> {
        let (name, size, mtime) = Self::key(p)?;
        self.entries
            .get(&name)
            .filter(|(s, t, _)| *s == size && *t == mtime)
            .map(|(_, _, ok)| *ok)
    }

    pub fn set(&mut self, p: &Path, ok: bool) {
        if let Some((name, size, mtime)) = Self::key(p) {
            self.entries.insert(name, (size, mtime, ok));
            if let Some(path) = &self.path {
                let tmp = path.with_extension("json.tmp");
                if std::fs::write(&tmp, serde_json::to_vec(self).unwrap_or_default()).is_ok() {
                    let _ = std::fs::rename(tmp, path);
                }
            }
        }
    }

    /// Bundled files in `dir` that still need hashing.
    pub fn pending(&self, dir: &Path) -> Vec<PathBuf> {
        crate::pins::pins()
            .models
            .iter()
            .map(|m| dir.join(&m.file))
            .filter(|p| {
                std::fs::metadata(p)
                    .map(|md| {
                        Some(md.len())
                            == pinned_model(&p.file_name().unwrap().to_string_lossy())
                                .map(|m| m.size)
                    })
                    .unwrap_or(false)
                    && self.get(p).is_none()
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_names_a_learner_can_read() {
        let g = |name: Option<&str>, arch: &str| GgufInfo {
            name: name.map(str::to_string),
            architecture: Some(arch.into()),
            ..Default::default()
        };
        assert_eq!(
            display_name(&g(Some("llama"), "llama"), "stories260K-f32.gguf"),
            "stories260K f32"
        );
        assert_eq!(
            display_name(
                &g(Some("Models Qwen Qwen3 0.6B"), "qwen3"),
                "Qwen_Qwen3-0.6B-Q2_K.gguf"
            ),
            "Qwen Qwen3 0.6B"
        );
        assert_eq!(
            display_name(&g(Some("Gemma 3 1B Instruct"), "gemma3"), "x.gguf"),
            "Gemma 3 1B Instruct"
        );
        assert_eq!(
            display_name(&g(None, "gemma3"), "my-model.gguf"),
            "my model"
        );
    }
    use crate::gguf::testfile;

    const GB: u64 = 1 << 30;

    #[test]
    fn quant_names() {
        let cases = [
            ("gemma-3-1b-it-Q4_K_M.gguf", Some("Q4_K_M")),
            ("Qwen_Qwen3-0.6B-Q4_K_M.gguf", Some("Q4_K_M")),
            ("gemma-3-1b-pt.Q4_K_M.gguf", Some("Q4_K_M")),
            ("model-Q8_0.gguf", Some("Q8_0")),
            ("Llama-3.2-1B-Instruct-IQ4_XS.gguf", Some("IQ4_XS")),
            ("phi-4-mini-f16.gguf", Some("F16")),
            ("model_bf16.gguf", Some("BF16")),
            ("gemma-3-1B-it-QAT-Q4_0.gguf", Some("Q4_0")),
            ("gpt-oss-20b-MXFP4.gguf", Some("MXFP4")),
            ("mystery.gguf", None),
            ("Qwen3-4B.gguf", None),
        ];
        for (f, q) in cases {
            assert_eq!(quant_from_name(f).as_deref(), q, "{f}");
        }
    }

    #[test]
    fn scan_lists_models_and_hides_the_rest() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(
            d.path().join("a-Q4_K_M.gguf"),
            testfile::chat_model("Alpha"),
        )
        .unwrap();
        std::fs::write(d.path().join("broken.gguf"), b"GGUFxx").unwrap();
        std::fs::write(d.path().join("mmproj-f16.gguf"), testfile::chat_model("P")).unwrap();
        std::fs::write(
            d.path().join("big-00002-of-00002.gguf"),
            testfile::chat_model("B"),
        )
        .unwrap();
        std::fs::write(d.path().join("notes.txt"), b"hi").unwrap();
        std::fs::write(d.path().join("x.gguf.part"), b"GGUF").unwrap();
        let list = scan(d.path(), 4 * GB, &VerifiedCache::default());
        let names: Vec<(&str, bool)> = list.iter().map(|m| (m.file.as_str(), m.broken)).collect();
        assert_eq!(names, vec![("broken.gguf", true), ("a-Q4_K_M.gguf", false)]);
        let a = &list[1];
        assert_eq!(a.name, "Alpha");
        assert_eq!(a.quant.as_deref(), Some("Q4_K_M"));
        assert!(a.has_chat_template);
        assert_eq!(a.fit, RamFit::Green);
    }

    #[test]
    fn import_copies_atomically_and_reports_progress() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        let s = src.path().join("Modèle Q4_K_M.gguf");
        std::fs::write(&s, testfile::chat_model("M")).unwrap();
        let mut last = (0, 0);
        let name = import(&s, dst.path(), |d, t| last = (d, t)).unwrap();
        assert_eq!(name, "Mod_le_Q4_K_M.gguf");
        assert_eq!(last.0, last.1);
        assert_eq!(
            std::fs::read(dst.path().join(&name)).unwrap(),
            std::fs::read(&s).unwrap()
        );
        assert!(
            std::fs::read_dir(dst.path()).unwrap().all(|e| !e
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".part"))
        );
        // Importing the same file again is a no-op.
        assert_eq!(import(&s, dst.path(), |_, _| {}).unwrap(), name);
        assert_eq!(std::fs::read_dir(dst.path()).unwrap().count(), 1);
    }

    #[test]
    fn import_rejects_non_models() {
        let d = tempfile::tempdir().unwrap();
        let s = d.path().join("photo.gguf");
        std::fs::write(&s, b"\x89PNG....").unwrap();
        assert_eq!(
            import(&s, d.path(), |_, _| {}).unwrap_err().code,
            crate::errors::ErrorCode::NotGguf
        );
        let t = d.path().join("notes.txt");
        std::fs::write(&t, b"x").unwrap();
        assert_eq!(
            import(&t, d.path(), |_, _| {}).unwrap_err().code,
            crate::errors::ErrorCode::NotGguf
        );
    }

    #[test]
    fn import_rejects_truncated_models() {
        let d = tempfile::tempdir().unwrap();
        let s = d.path().join("cut.gguf");
        let b = testfile::chat_model("M");
        std::fs::write(&s, &b[..b.len() - 100]).unwrap();
        let out = tempfile::tempdir().unwrap();
        assert_eq!(
            import(&s, out.path(), |_, _| {}).unwrap_err().code,
            crate::errors::ErrorCode::ModelCorrupt
        );
    }

    #[test]
    fn delete_refuses_traversal_and_tolerates_missing() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a.gguf"), b"x").unwrap();
        delete(d.path(), "a.gguf").unwrap();
        assert!(!d.path().join("a.gguf").exists());
        delete(d.path(), "a.gguf").unwrap();
        assert!(delete(d.path(), "../etc.gguf").is_err());
    }

    #[test]
    fn verified_cache_is_keyed_by_size_and_mtime() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("m.gguf");
        std::fs::write(&f, b"one").unwrap();
        let mut c = VerifiedCache::load(&d.path().join("v.json"));
        assert_eq!(c.get(&f), None);
        c.set(&f, true);
        assert_eq!(
            VerifiedCache::load(&d.path().join("v.json")).get(&f),
            Some(true)
        );
        std::fs::write(&f, b"three").unwrap();
        assert_eq!(c.get(&f), None);
    }

    #[test]
    fn sha256_known_value() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("x");
        std::fs::write(&f, b"abc").unwrap();
        assert_eq!(
            sha256_file(&f, |_| {}).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
