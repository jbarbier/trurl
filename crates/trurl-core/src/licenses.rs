//! License texts shown in the About screen, compiled in so they are always
//! there, offline, whatever happens to the install folder.

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct License {
    pub name: &'static str,
    pub text: std::borrow::Cow<'static, str>,
}

/// The full list of Rust crates is large (about 1.7 MB), so it ships as a file
/// next to the app instead of inside the binary.
fn third_party() -> String {
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("licenses").join("THIRD-PARTY.txt"));
            candidates.push(dir.join("..").join("licenses").join("THIRD-PARTY.txt"));
            candidates.push(
                dir.join("..")
                    .join("share")
                    .join("trurl")
                    .join("licenses")
                    .join("THIRD-PARTY.txt"),
            );
        }
    }
    candidates.push(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../licenses/THIRD-PARTY.txt"),
    );
    candidates
        .iter()
        .find_map(|p| std::fs::read_to_string(p).ok())
        .unwrap_or_else(|| "The licenses of the open-source components inside Trurl are in the file licenses/THIRD-PARTY.txt in Trurl's install folder, and at https://github.com/jbarbier/trurl/blob/main/licenses/THIRD-PARTY.txt".to_string())
}

pub fn all() -> Vec<License> {
    vec![
        License {
            name: "Trurl (MIT License)",
            text: include_str!("../../../LICENSE").into(),
        },
        License {
            name: "Gemma 3 models: Gemma Terms of Use",
            text: include_str!("../../../licenses/Gemma-Terms-of-Use.txt").into(),
        },
        License {
            name: "Qwen3 0.6B (Apache License 2.0)",
            text: include_str!("../../../licenses/Qwen3-Apache-2.0.txt").into(),
        },
        License {
            name: "llama.cpp (MIT License)",
            text: include_str!("../../../licenses/llama.cpp-MIT.txt").into(),
        },
        License {
            name: "Sora typeface (SIL Open Font License 1.1)",
            text: include_str!("../../../licenses/Sora-OFL.txt").into(),
        },
        License {
            name: "Other open-source components",
            text: third_party().into(),
        },
    ]
}

#[cfg(test)]
mod tests {
    #[test]
    fn gemma_notice_is_first_line() {
        let l = super::all();
        let g = l.iter().find(|x| x.name.contains("Gemma")).unwrap();
        assert!(g.text.starts_with("Gemma is provided under and subject to the Gemma Terms of Use found at ai.google.dev/gemma/terms"));
        assert!(
            l.iter().all(|x| x.text.len() > 200),
            "every license has its text"
        );
    }
}
