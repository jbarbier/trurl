//! Hugging Face Hub: online check, search, and file list. Only used when the
//! learner opens the browser; nothing here runs on the chat path.

use crate::errors::UserError;
use crate::hardware::{RamFit, ram_fit};
use serde::{Deserialize, Serialize};
use std::time::Duration;

pub const HF_BASE: &str = "https://huggingface.co";

#[derive(Debug, Clone, Serialize)]
pub struct RepoHit {
    pub id: String,
    pub downloads: u64,
    pub likes: u64,
    pub gated: bool,
    pub updated: Option<String>,
    /// Some(false): this model does not chat (embeddings, speech, rerankers):
    /// downloading it would waste a learner's data. None: cannot tell.
    pub chat: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RepoFile {
    pub path: String,
    pub size: u64,
    pub sha256: Option<String>,
    pub quant: Option<String>,
    pub fit: RamFit,
    /// Part of a model split over several files: Trurl cannot download those.
    pub split: bool,
}

/// Everything the details view shows about one repository.
#[derive(Debug, Clone, Serialize)]
pub struct RepoFiles {
    pub id: String,
    pub author: Option<String>,
    pub revision: String,
    pub gated: bool,
    pub license: Option<String>,
    pub downloads: u64,
    pub likes: u64,
    pub updated: Option<String>,
    pub base_model: Option<String>,
    pub architecture: Option<String>,
    pub context_length: Option<u64>,
    /// Parameter count from the GGUF header, e.g. 751632384.
    pub parameters: Option<u64>,
    pub tags: Vec<String>,
    /// See `RepoHit::chat`.
    pub chat: Option<bool>,
    /// The model card (README.md) as Markdown, front matter removed. Filled by `details`.
    pub readme: Option<String>,
    pub files: Vec<RepoFile>,
}

#[derive(Deserialize)]
struct ApiHit {
    id: String,
    #[serde(default)]
    downloads: u64,
    #[serde(default)]
    likes: u64,
    #[serde(default)]
    gated: serde_json::Value,
    #[serde(default, rename = "lastModified")]
    last_modified: Option<String>,
    #[serde(default)]
    pipeline_tag: Option<String>,
}

#[derive(Deserialize)]
struct ApiRepo {
    id: String,
    sha: String,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    gated: serde_json::Value,
    #[serde(default)]
    downloads: u64,
    #[serde(default)]
    likes: u64,
    #[serde(default, rename = "lastModified")]
    last_modified: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    pipeline_tag: Option<String>,
    #[serde(default, rename = "cardData")]
    card: Option<serde_json::Value>,
    #[serde(default)]
    gguf: Option<serde_json::Value>,
    #[serde(default)]
    siblings: Vec<ApiSibling>,
}

#[derive(Deserialize)]
struct ApiSibling {
    rfilename: String,
    #[serde(default)]
    size: Option<u64>,
    #[serde(default)]
    lfs: Option<ApiLfs>,
}

#[derive(Deserialize)]
struct ApiLfs {
    sha256: String,
    #[serde(default)]
    size: Option<u64>,
}

fn is_gated(v: &serde_json::Value) -> bool {
    match v {
        serde_json::Value::Bool(b) => *b,
        serde_json::Value::String(s) => !s.is_empty() && s != "false",
        _ => false,
    }
}

#[derive(Clone)]
pub struct Hub {
    http: reqwest::Client,
    base: String,
    token: Option<String>,
}

impl Hub {
    pub fn new(http: reqwest::Client, base: impl Into<String>, token: Option<String>) -> Self {
        Self {
            http,
            base: base.into().trim_end_matches('/').to_string(),
            token: token.filter(|t| !t.trim().is_empty()),
        }
    }

    fn get(&self, url: String) -> reqwest::RequestBuilder {
        let rb = self.http.get(url);
        match &self.token {
            Some(t) => rb.bearer_auth(t),
            None => rb,
        }
    }

    /// GET a tiny API route and require real JSON: a captive portal (hotel or
    /// hotspot login page) answers 200 with HTML and must count as offline.
    pub async fn online(&self) -> bool {
        let r = self
            .http
            .get(format!("{}/api/models?limit=1", self.base))
            .timeout(Duration::from_secs(5))
            .send()
            .await;
        match r {
            Ok(resp) if resp.status().is_success() => {
                resp.json::<Vec<serde_json::Value>>().await.is_ok()
            }
            _ => false,
        }
    }

    pub async fn search(&self, query: &str) -> Result<Vec<RepoHit>, UserError> {
        let q = query.trim();
        let url = format!(
            "{}/api/models?search={}&filter=gguf&sort=downloads&direction=-1&limit=30",
            self.base,
            urlencode(q)
        );
        let resp = self
            .get(url)
            .timeout(Duration::from_secs(20))
            .send()
            .await
            .map_err(hub_error)?;
        if !resp.status().is_success() {
            return Err(UserError::hub_failed().with_detail(format!("HTTP {}", resp.status())));
        }
        let hits: Vec<ApiHit> = crate::net::json_limited(resp, MAX_JSON)
            .await
            .map_err(|e| UserError::hub_failed().with_detail(e))?;
        Ok(hits
            .into_iter()
            .map(|h| RepoHit {
                chat: chat_kind(h.pipeline_tag.as_deref(), &h.id),
                id: h.id,
                downloads: h.downloads,
                likes: h.likes,
                gated: is_gated(&h.gated),
                updated: h.last_modified,
            })
            .collect())
    }

    pub async fn files(&self, repo: &str, total_ram: u64) -> Result<RepoFiles, UserError> {
        let url = format!(
            "{}/api/models/{}?blobs=true",
            self.base,
            repo.split('/').map(urlencode).collect::<Vec<_>>().join("/")
        );
        let resp = self
            .get(url)
            .timeout(Duration::from_secs(20))
            .send()
            .await
            .map_err(hub_error)?;
        match resp.status().as_u16() {
            200 => {}
            401 | 403 => return Err(UserError::hub_gated(repo)),
            s => return Err(UserError::hub_failed().with_detail(format!("HTTP {s}"))),
        }
        let r: ApiRepo = crate::net::json_limited(resp, MAX_JSON)
            .await
            .map_err(|e| UserError::hub_failed().with_detail(e))?;
        Ok(repo_files(r, total_ram))
    }

    /// Files plus the model card. The card is optional: a missing or slow
    /// README never blocks the file list.
    pub async fn details(&self, repo: &str, total_ram: u64) -> Result<RepoFiles, UserError> {
        let mut d = self.files(repo, total_ram).await?;
        let url = format!(
            "{}/{}/raw/{}/README.md",
            self.base,
            repo.split('/').map(urlencode).collect::<Vec<_>>().join("/"),
            d.revision
        );
        if let Ok(resp) = self.get(url).timeout(Duration::from_secs(15)).send().await
            && resp.status().is_success()
            && let Ok(bytes) = crate::net::read_limited(resp, MAX_README).await
        {
            d.readme = Some(clean_readme(&String::from_utf8_lossy(&bytes)));
        }
        Ok(d)
    }
}

/// Hub replies are a few hundred KB; a repo with thousands of files is a few MB.
const MAX_JSON: usize = 16 * 1024 * 1024;
/// Model cards are text; anything this big is not one worth showing.
const MAX_README: usize = 1024 * 1024;

fn hub_error(e: reqwest::Error) -> UserError {
    let detail = crate::errors::chain(&e);
    match crate::net::classify(&e) {
        crate::net::NetFailure::Offline => UserError::offline().with_detail(detail),
        crate::net::NetFailure::Tls => UserError::hub_tls().with_detail(detail),
        crate::net::NetFailure::Other => UserError::hub_failed().with_detail(detail),
    }
}

/// Can this repository's model chat? From Hugging Face's task tag, and, when a
/// repository has none, from words in its name that mean it cannot.
pub fn chat_kind(pipeline_tag: Option<&str>, id: &str) -> Option<bool> {
    const CHAT: &[&str] = &[
        "text-generation",
        "image-text-to-text",
        "conversational",
        "text2text-generation",
    ];
    const NOT_CHAT: &[&str] = &[
        "feature-extraction",
        "sentence-similarity",
        "text-ranking",
        "text-classification",
        "token-classification",
        "automatic-speech-recognition",
        "text-to-speech",
        "text-to-audio",
        "audio-classification",
        "text-to-image",
        "image-classification",
        "zero-shot-classification",
        "fill-mask",
        "translation",
    ];
    if let Some(t) = pipeline_tag {
        if CHAT.contains(&t) {
            return Some(true);
        }
        if NOT_CHAT.contains(&t) {
            return Some(false);
        }
    }
    let name = id.rsplit('/').next().unwrap_or(id).to_ascii_lowercase();
    let words = [
        "embed", "rerank", "-asr", "asr-", "tts", "whisper", "-clip", "bge-", "e5-", "minilm",
    ];
    if words.iter().any(|w| name.contains(w)) {
        return Some(false);
    }
    None
}

fn repo_files(r: ApiRepo, total_ram: u64) -> RepoFiles {
    let license = r
        .card
        .as_ref()
        .and_then(|c| c.get("license"))
        .and_then(|l| l.as_str())
        .map(str::to_string);
    let mut files: Vec<RepoFile> = r
        .siblings
        .into_iter()
        .filter(|s| s.rfilename.to_ascii_lowercase().ends_with(".gguf"))
        .filter(|s| {
            !s.rfilename
                .rsplit('/')
                .next()
                .unwrap_or("")
                .to_ascii_lowercase()
                .starts_with("mmproj")
        })
        .map(|s| {
            let size = s.lfs.as_ref().and_then(|l| l.size).or(s.size).unwrap_or(0);
            let name = s
                .rfilename
                .rsplit('/')
                .next()
                .unwrap_or(&s.rfilename)
                .to_string();
            RepoFile {
                quant: crate::models::quant_from_name(&name),
                fit: ram_fit(size, total_ram),
                split: crate::gguf::split_part(&name).is_some(),
                sha256: s.lfs.map(|l| l.sha256),
                size,
                path: s.rfilename,
            }
        })
        .collect();
    files.sort_by_key(|f| (f.split, f.size));
    let card_str = |k: &str| {
        r.card
            .as_ref()
            .and_then(|c| c.get(k))
            .and_then(|v| match v {
                serde_json::Value::String(s) => Some(s.clone()),
                serde_json::Value::Array(a) => {
                    a.first().and_then(|x| x.as_str()).map(str::to_string)
                }
                _ => None,
            })
    };
    let g = |k: &str| {
        r.gguf
            .as_ref()
            .and_then(|g| g.get(k))
            .and_then(|v| v.as_u64())
    };
    RepoFiles {
        author: r.author.clone(),
        revision: r.sha.clone(),
        gated: is_gated(&r.gated),
        license,
        downloads: r.downloads,
        likes: r.likes,
        updated: r.last_modified.clone(),
        base_model: card_str("base_model"),
        architecture: r
            .gguf
            .as_ref()
            .and_then(|g| g.get("architecture"))
            .and_then(|v| v.as_str())
            .map(str::to_string),
        context_length: g("context_length"),
        parameters: g("total"),
        chat: chat_kind(r.pipeline_tag.as_deref(), &r.id),
        tags: r
            .tags
            .iter()
            .filter(|t| !t.contains(':') && !t.starts_with("region"))
            .take(12)
            .cloned()
            .collect(),
        readme: None,
        id: r.id,
        files,
    }
}

/// Drop the YAML front matter and cap the size: some cards are huge.
pub fn clean_readme(text: &str) -> String {
    let t = text.trim_start_matches('\u{feff}');
    let body = if let Some(rest) = t.strip_prefix("---") {
        match rest.find("\n---") {
            Some(i) => rest[i + 4..].trim_start_matches(['\r', '\n']).to_string(),
            None => t.to_string(),
        }
    } else {
        t.to_string()
    };
    const MAX: usize = 60_000;
    if body.len() > MAX {
        let mut cut = MAX;
        while !body.is_char_boundary(cut) {
            cut -= 1;
        }
        format!("{}\n\n…", &body[..cut])
    } else {
        body
    }
}

pub fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn models_that_cannot_chat_are_marked() {
        assert_eq!(
            chat_kind(Some("text-generation"), "Qwen/Qwen3-0.6B-GGUF"),
            Some(true)
        );
        assert_eq!(
            chat_kind(None, "Qwen/Qwen3-Embedding-0.6B-GGUF"),
            Some(false)
        );
        assert_eq!(
            chat_kind(
                Some("automatic-speech-recognition"),
                "x/Qwen3-ASR-0.6B-gguf"
            ),
            Some(false)
        );
        assert_eq!(
            chat_kind(
                Some("text-ranking"),
                "ggml-org/Qwen3-Reranker-0.6B-Q8_0-GGUF"
            ),
            Some(false)
        );
        assert_eq!(
            chat_kind(None, "mradermacher/Qwen3-Reranker-0.6B-GGUF"),
            Some(false)
        );
        assert_eq!(
            chat_kind(None, "unslothai/Qwen3-ASR-0.6B-GGUF"),
            Some(false)
        );
        assert_eq!(
            chat_kind(
                Some("text-to-speech"),
                "cstr/qwen3-tts-0.6b-customvoice-GGUF"
            ),
            Some(false)
        );
        assert_eq!(
            chat_kind(None, "bartowski/Llama-3.2-1B-Instruct-GGUF"),
            None
        );
    }

    #[test]
    fn parses_a_real_repo_listing() {
        // Trimmed from GET /api/models/ggml-org/gemma-3-4b-it-GGUF?blobs=true
        let j = r#"{"id":"ggml-org/gemma-3-4b-it-GGUF","sha":"d0976223747697cb51e056d85c532013931fe52e","gated":false,
          "author":"ggml-org","downloads":1234,"likes":56,"lastModified":"2025-05-21T15:38:43.000Z",
          "tags":["gguf","text-generation","base_model:google/gemma-3-4b-it","region:us"],
          "gguf":{"total":3880099328,"architecture":"gemma3","context_length":131072},
          "cardData":{"license":"gemma","base_model":"google/gemma-3-4b-it"},
          "siblings":[
            {"rfilename":".gitattributes","size":1700},
            {"rfilename":"README.md","size":200},
            {"rfilename":"gemma-3-4b-it-Q4_K_M.gguf","size":2489757856,"lfs":{"sha256":"882e8d2db44dc554fb0ea5077cb7e4bc49e7342a1f0da57901c0802ea21a0863","size":2489757856}},
            {"rfilename":"gemma-3-4b-it-Q8_0.gguf","size":4130226336,"lfs":{"sha256":"aa","size":4130226336}},
            {"rfilename":"mmproj-model-f16.gguf","size":851251104,"lfs":{"sha256":"bb","size":851251104}},
            {"rfilename":"big/x-00001-of-00002.gguf","size":10,"lfs":{"sha256":"cc","size":10}}
          ]}"#;
        let r: ApiRepo = serde_json::from_str(j).unwrap();
        let f = repo_files(r, 6 << 30);
        assert_eq!(f.revision.len(), 40);
        assert_eq!(f.license.as_deref(), Some("gemma"));
        let names: Vec<&str> = f.files.iter().map(|x| x.path.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "gemma-3-4b-it-Q4_K_M.gguf",
                "gemma-3-4b-it-Q8_0.gguf",
                "big/x-00001-of-00002.gguf"
            ]
        );
        assert_eq!(f.files[0].quant.as_deref(), Some("Q4_K_M"));
        assert_eq!(f.files[0].fit, RamFit::Green);
        assert_eq!(f.files[1].fit, RamFit::Orange);
        assert!(f.files[2].split);
        assert_eq!(f.parameters, Some(3880099328));
        assert_eq!(f.architecture.as_deref(), Some("gemma3"));
        assert_eq!(f.context_length, Some(131072));
        assert_eq!(f.base_model.as_deref(), Some("google/gemma-3-4b-it"));
        assert_eq!(f.tags, vec!["gguf", "text-generation"]);
        assert_eq!((f.downloads, f.likes), (1234, 56));
    }

    #[test]
    fn readme_front_matter_is_removed() {
        assert_eq!(
            clean_readme("---\nlicense: mit\n---\n\n# Title\nBody"),
            "# Title\nBody"
        );
        assert_eq!(clean_readme("# No front matter"), "# No front matter");
        assert!(clean_readme(&"é".repeat(40_000)).ends_with('…'));
    }

    #[test]
    fn gated_values() {
        assert!(!is_gated(&serde_json::json!(false)));
        assert!(is_gated(&serde_json::json!("auto")));
        assert!(is_gated(&serde_json::json!("manual")));
        assert!(!is_gated(&serde_json::Value::Null));
    }

    #[test]
    fn query_encoding() {
        assert_eq!(urlencode("gemma 3 1b"), "gemma%203%201b");
        assert_eq!(urlencode("a&b=c"), "a%26b%3Dc");
    }
}
