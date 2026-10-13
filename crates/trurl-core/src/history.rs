//! Chat history: every chat is saved on this computer, one JSON file per chat
//! in `<data>/chats/`. Nothing here ever leaves the computer. A learner can
//! open an old chat and carry on, delete one, or delete them all.
//!
//! Files are written whole to a temporary name and renamed, so a power cut
//! leaves the old version or the new one, never half of each. A file that
//! cannot be read (damaged, or from a newer Trurl) is skipped when listing and
//! opening; only "Delete all chats" removes it.

use crate::chat::{Turn, TurnStatus};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// A saved chat.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Chat {
    pub id: String,
    /// Seconds since 1970, when the chat started and when it last changed.
    pub created: u64,
    pub updated: u64,
    /// The model file the last answer came from, to load it again on open.
    #[serde(default)]
    pub model_file: Option<String>,
    pub turns: Vec<Turn>,
}

/// One line of the history list.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ChatSummary {
    pub id: String,
    /// The first question, on one line and shortened.
    pub title: String,
    pub updated: u64,
    /// Questions asked in it.
    pub turns: usize,
    /// Display name of the model of the last answer.
    pub model: String,
}

pub const TITLE_CHARS: usize = 60;

impl Chat {
    pub fn summary(&self) -> ChatSummary {
        ChatSummary {
            id: self.id.clone(),
            title: title(self.turns.first().map(|t| t.user.as_str()).unwrap_or("")),
            updated: self.updated,
            turns: self.turns.len(),
            model: self
                .turns
                .last()
                .map(|t| t.model.clone())
                .unwrap_or_default(),
        }
    }
}

/// The first line of the first question, at most `TITLE_CHARS` characters.
pub fn title(first_question: &str) -> String {
    let line = first_question
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let line: String = line.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() <= TITLE_CHARS {
        return line;
    }
    let cut: String = line.chars().take(TITLE_CHARS - 1).collect();
    format!("{}…", cut.trim_end())
}

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// A new chat id: the time in milliseconds, then a counter, so two chats
/// started in the same millisecond still differ. Ids sort by age.
pub fn new_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    format!("{ms:013}-{:04}", N.fetch_add(1, Ordering::Relaxed) % 10_000)
}

/// Ids come back from the front ends: only digits and dashes, so no id can
/// name a file outside the chats folder.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 40 && id.bytes().all(|b| b.is_ascii_digit() || b == b'-')
}

#[derive(Debug, Clone)]
pub struct History {
    dir: PathBuf,
}

impl History {
    pub fn new(dir: PathBuf) -> Self {
        History { dir }
    }

    fn path(&self, id: &str) -> Option<PathBuf> {
        valid_id(id).then(|| self.dir.join(format!("{id}.json")))
    }

    pub fn exists(&self, id: &str) -> bool {
        self.path(id).is_some_and(|p| p.exists())
    }

    /// Every readable chat, newest first.
    pub fn list(&self) -> Vec<ChatSummary> {
        let Ok(rd) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut out: Vec<ChatSummary> = rd
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                let id = name.strip_suffix(".json")?;
                self.load(id).map(|c| c.summary())
            })
            .filter(|s| s.turns > 0)
            .collect();
        out.sort_by(|a, b| b.updated.cmp(&a.updated).then(b.id.cmp(&a.id)));
        out
    }

    /// A saved chat. An answer that was still being written when Trurl closed
    /// comes back as stopped.
    pub fn load(&self, id: &str) -> Option<Chat> {
        let p = self.path(id)?;
        let bytes = std::fs::read(&p).ok()?;
        let mut c: Chat = serde_json::from_slice(&bytes).ok()?;
        if c.id != id {
            return None;
        }
        for t in &mut c.turns {
            if t.status == TurnStatus::Streaming {
                t.status = TurnStatus::Stopped;
            }
        }
        Some(c)
    }

    pub fn save(&self, c: &Chat) -> std::io::Result<()> {
        let p = self
            .path(&c.id)
            .ok_or_else(|| std::io::Error::other("bad chat id"))?;
        std::fs::create_dir_all(&self.dir)?;
        let tmp = self.dir.join(format!(".{}.tmp", c.id));
        let data = serde_json::to_vec_pretty(c).map_err(std::io::Error::other)?;
        write_synced(&tmp, &data)?;
        std::fs::rename(&tmp, &p)
    }

    /// Delete one chat. A chat that is already gone is not an error.
    pub fn delete(&self, id: &str) -> std::io::Result<()> {
        let p = self
            .path(id)
            .ok_or_else(|| std::io::Error::other("bad chat id"))?;
        match std::fs::remove_file(p) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }

    /// Delete every chat file (and leftovers of interrupted saves). Other files
    /// in the folder are left alone.
    pub fn delete_all(&self) -> std::io::Result<()> {
        let Ok(rd) = std::fs::read_dir(&self.dir) else {
            return Ok(());
        };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let ours = name.strip_suffix(".json").is_some_and(valid_id)
                || name
                    .strip_prefix('.')
                    .and_then(|n| n.strip_suffix(".tmp"))
                    .is_some_and(valid_id);
            if ours {
                std::fs::remove_file(e.path())?;
            }
        }
        Ok(())
    }
}

fn write_synced(p: &Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = std::fs::File::create(p)?;
    f.write_all(data)?;
    f.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(id: u64, user: &str, status: TurnStatus) -> Turn {
        Turn {
            id,
            user: user.into(),
            reasoning: "thinking".into(),
            answer: "answer".into(),
            model: "Gemma 3 1B Instruct".into(),
            status,
            thought_ms: Some(1200),
            error: None,
            system_prompt: "Be brief.".into(),
            temperature: 0.3,
            context: 4096,
        }
    }

    fn chat(id: &str, updated: u64, turns: Vec<Turn>) -> Chat {
        Chat {
            id: id.into(),
            created: updated,
            updated,
            model_file: Some("gemma-3-1b-it-Q4_K_M.gguf".into()),
            turns,
        }
    }

    #[test]
    fn saves_loads_and_lists_newest_first() {
        let d = tempfile::tempdir().unwrap();
        let h = History::new(d.path().join("chats"));
        assert!(h.list().is_empty(), "no folder yet: an empty list");
        let a = chat(
            "1000-0001",
            10,
            vec![turn(1, "Where is Accra?", TurnStatus::Done)],
        );
        let b = chat(
            "2000-0001",
            20,
            vec![turn(1, "What is 17 + 25?", TurnStatus::Done)],
        );
        h.save(&a).unwrap();
        h.save(&b).unwrap();
        assert_eq!(
            h.load("1000-0001").unwrap(),
            a,
            "everything comes back, settings included"
        );
        let l = h.list();
        assert_eq!(
            l.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            ["2000-0001", "1000-0001"]
        );
        assert_eq!(l[1].title, "Where is Accra?");
        assert_eq!(l[1].model, "Gemma 3 1B Instruct");
        assert_eq!(l[1].turns, 1);
        // No temporary file left behind.
        let names: Vec<_> = std::fs::read_dir(d.path().join("chats"))
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(names.len(), 2, "{names:?}");
    }

    #[test]
    fn an_answer_cut_off_by_closing_trurl_comes_back_stopped() {
        let d = tempfile::tempdir().unwrap();
        let h = History::new(d.path().into());
        h.save(&chat("5-1", 1, vec![turn(1, "Hi", TurnStatus::Streaming)]))
            .unwrap();
        assert_eq!(h.load("5-1").unwrap().turns[0].status, TurnStatus::Stopped);
    }

    #[test]
    fn ids_cannot_leave_the_folder_and_bad_files_are_skipped_not_deleted() {
        let d = tempfile::tempdir().unwrap();
        let h = History::new(d.path().join("chats"));
        for bad in ["../settings", "a/b", "", "..", "1.json", "x"] {
            assert!(!valid_id(bad), "{bad}");
            assert!(h.load(bad).is_none());
            assert!(h.delete(bad).is_err());
        }
        std::fs::create_dir_all(d.path().join("chats")).unwrap();
        std::fs::write(d.path().join("chats/123-4.json"), b"{ not json").unwrap();
        std::fs::write(d.path().join("chats/notes.txt"), b"teacher notes").unwrap();
        // A file whose id does not match its name is not trusted either.
        h.save(&chat("7-1", 1, vec![turn(1, "Hi", TurnStatus::Done)]))
            .unwrap();
        std::fs::rename(
            d.path().join("chats/7-1.json"),
            d.path().join("chats/8-1.json"),
        )
        .unwrap();
        assert!(h.list().is_empty());
        assert!(
            d.path().join("chats/123-4.json").exists(),
            "a damaged chat is kept"
        );
        h.delete_all().unwrap();
        assert!(
            d.path().join("chats/notes.txt").exists(),
            "only chat files are deleted"
        );
        assert!(!d.path().join("chats/123-4.json").exists());
    }

    #[test]
    fn delete_one_then_all() {
        let d = tempfile::tempdir().unwrap();
        let h = History::new(d.path().into());
        for i in 1..=3 {
            h.save(&chat(
                &format!("{i}-1"),
                i,
                vec![turn(1, "Q", TurnStatus::Done)],
            ))
            .unwrap();
        }
        h.delete("2-1").unwrap();
        h.delete("2-1").unwrap(); // already gone: fine
        assert_eq!(h.list().len(), 2);
        h.delete_all().unwrap();
        assert!(h.list().is_empty());
    }

    #[test]
    fn titles_are_one_short_line() {
        assert_eq!(title("\n  Where is   Accra?\nAnd why?"), "Where is Accra?");
        let long = "word ".repeat(40);
        let t = title(&long);
        assert!(t.chars().count() <= TITLE_CHARS && t.ends_with('…'), "{t}");
        assert_eq!(title(""), "");
    }

    #[test]
    fn new_ids_are_valid_distinct_and_sort_by_age() {
        let a = new_id();
        let b = new_id();
        assert!(valid_id(&a) && valid_id(&b));
        assert_ne!(a, b);
        assert!(a < b || a[..13] == b[..13]);
    }
}
