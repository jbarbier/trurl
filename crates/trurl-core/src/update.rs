//! Self-update for the terminal app (the desktop app uses Tauri's updater).
//!
//! Both read the same `latest.json` on GitHub Releases and trust the same
//! minisign key: the desktop app reads the `windows-x86_64` entry, the terminal
//! app the `linux-x86_64` one. Nothing is installed unless the signature checks.

use crate::errors::UserError;
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;

pub const MANIFEST_URL: &str =
    "https://github.com/jbarbier/trurl/releases/latest/download/latest.json";
/// The same public key as `plugins.updater.pubkey` in tauri.conf.json (checked by a test).
pub const PUBKEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDY2QjFGRTA4RUNGNUZGRUIKUldUci8vWHNDUDZ4WnJpQVNvaitQZXZEMmlFcnc1RlNla3p3RmxNQ2hoMHdKNm9BQi9GUm85enAK";

#[derive(Debug, Clone, Deserialize)]
pub struct Manifest {
    pub version: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub pub_date: Option<String>,
    #[serde(default)]
    pub platforms: HashMap<String, Platform>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Platform {
    pub signature: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct UpdateInfo {
    pub version: String,
    pub notes: String,
    pub date: Option<String>,
    #[serde(skip)]
    pub url: String,
    #[serde(skip)]
    pub signature: String,
}

/// "0.10.0" > "0.9.3". Pre-release suffixes are ignored, missing parts are 0.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    let parse = |v: &str| -> Vec<u64> {
        v.trim_start_matches('v')
            .split(['-', '+'])
            .next()
            .unwrap_or("")
            .split('.')
            .map(|p| p.parse().unwrap_or(0))
            .collect()
    };
    let (a, b) = (parse(candidate), parse(current));
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (
            a.get(i).copied().unwrap_or(0),
            b.get(i).copied().unwrap_or(0),
        );
        if x != y {
            return x > y;
        }
    }
    false
}

pub fn platform_key() -> &'static str {
    if cfg!(windows) {
        "windows-x86_64"
    } else {
        "linux-x86_64"
    }
}

/// Some(update) if a newer version exists for this platform. Errors only for
/// manual checks; callers doing background checks ignore them.
pub async fn check(
    http: &reqwest::Client,
    url: &str,
    current: &str,
) -> Result<Option<UpdateInfo>, UserError> {
    let resp = http
        .get(url)
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .map_err(|e| UserError::update_check_failed().with_detail(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(UserError::update_check_failed().with_detail(format!("HTTP {}", resp.status())));
    }
    let m: Manifest = resp
        .json()
        .await
        .map_err(|e| UserError::update_check_failed().with_detail(e.to_string()))?;
    Ok(pick(&m, current, platform_key()))
}

pub fn pick(m: &Manifest, current: &str, platform: &str) -> Option<UpdateInfo> {
    if !is_newer(&m.version, current) {
        return None;
    }
    let p = m.platforms.get(platform)?;
    Some(UpdateInfo {
        version: m.version.trim_start_matches('v').to_string(),
        notes: m.notes.clone(),
        date: m.pub_date.clone(),
        url: p.url.clone(),
        signature: p.signature.clone(),
    })
}

/// Check `bytes` against a Tauri-style signature (base64 of a minisign signature
/// file) and public key (base64 of a minisign public key file). When `version`
/// is given, the signature must also name that version in its signed comment
/// (`tauri signer sign --app-version`): a tampered manifest cannot pair a new
/// version number with an older signed release.
pub fn verify(
    bytes: &[u8],
    signature_b64: &str,
    pubkey_b64: &str,
    version: Option<&str>,
) -> Result<(), String> {
    let b64 = base64::engine::general_purpose::STANDARD;
    let pk_text = String::from_utf8(b64.decode(pubkey_b64.trim()).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let sig_text = String::from_utf8(
        b64.decode(signature_b64.trim())
            .map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let pk = minisign_verify::PublicKey::decode(&pk_text).map_err(|e| e.to_string())?;
    let sig = minisign_verify::Signature::decode(&sig_text).map_err(|e| e.to_string())?;
    pk.verify(bytes, &sig, false).map_err(|e| e.to_string())?;
    if let Some(want) = version {
        let signed = sig
            .trusted_comment()
            .split('\t')
            .find_map(|f| f.strip_prefix("version:"))
            .ok_or("the signature does not say which version it is for")?;
        if signed.trim_start_matches('v') != want.trim_start_matches('v') {
            return Err(format!("signed for version {signed}, announced as {want}"));
        }
    }
    Ok(())
}

/// Download the update and verify it. Returns the verified bytes.
pub async fn download(
    http: &reqwest::Client,
    info: &UpdateInfo,
    mut progress: impl FnMut(u64, Option<u64>),
) -> Result<Vec<u8>, UserError> {
    use futures_util::StreamExt;
    let resp = http
        .get(&info.url)
        .timeout(Duration::from_secs(600))
        .send()
        .await
        .map_err(|e| UserError::update_failed().with_detail(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(UserError::update_failed().with_detail(format!("HTTP {}", resp.status())));
    }
    let total = resp.content_length();
    let mut buf = Vec::with_capacity(total.unwrap_or(0) as usize);
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| UserError::update_failed().with_detail(e.to_string()))?;
        buf.extend_from_slice(&chunk);
        progress(buf.len() as u64, total);
    }
    verify(&buf, &info.signature, PUBKEY, Some(&info.version)).map_err(|e| {
        UserError::update_failed().with_detail(format!("signature check failed: {e}"))
    })?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_order() {
        assert!(is_newer("0.2.0", "0.1.0"));
        assert!(is_newer("v0.10.0", "0.9.9"));
        assert!(is_newer("1.0", "0.99.99"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0-beta", "0.1.0"));
        assert!(!is_newer("0.0.9", "0.1.0"));
    }

    #[test]
    fn picks_this_platform_only_when_newer() {
        let m: Manifest = serde_json::from_str(
            r#"{"version":"0.2.0","notes":"Faster.","pub_date":"2026-11-02T00:00:00Z",
                "platforms":{"windows-x86_64":{"signature":"w","url":"https://github.com/jbarbier/trurl/releases/download/v0.2.0/Trurl_0.2.0_x64-setup.exe"},
                             "linux-x86_64":{"signature":"l","url":"https://github.com/jbarbier/trurl/releases/download/v0.2.0/trurl-0.2.0-linux-x64.tar.gz"}}}"#,
        )
        .unwrap();
        let u = pick(&m, "0.1.0", "linux-x86_64").unwrap();
        assert_eq!(
            (u.version.as_str(), u.url.as_str(), u.signature.as_str()),
            (
                "0.2.0",
                "https://github.com/jbarbier/trurl/releases/download/v0.2.0/trurl-0.2.0-linux-x64.tar.gz",
                "l"
            )
        );
        assert!(pick(&m, "0.2.0", "linux-x86_64").is_none());
        assert!(pick(&m, "0.1.0", "darwin-aarch64").is_none());
    }

    #[test]
    fn pubkey_matches_the_desktop_updater() {
        let conf = include_str!("../../../apps/desktop/src-tauri/tauri.conf.json");
        assert!(
            conf.contains(PUBKEY),
            "tauri.conf.json and update.rs must trust the same key"
        );
        // And it decodes as a minisign public key.
        let text = String::from_utf8(
            base64::engine::general_purpose::STANDARD
                .decode(PUBKEY)
                .unwrap(),
        )
        .unwrap();
        assert!(minisign_verify::PublicKey::decode(&text).is_ok());
    }

    #[test]
    fn rejects_bad_signatures() {
        assert!(verify(b"data", "bm90IGEgc2ln", PUBKEY, None).is_err());
        // A real signature from the release key, made with
        // `tauri signer sign --app-version 9.9.9 tests/fixtures/update-fixture.txt`.
        let data = include_bytes!("../tests/fixtures/update-fixture.txt");
        let sig = include_str!("../tests/fixtures/update-fixture.txt.sig");
        assert_eq!(verify(data, sig, PUBKEY, Some("9.9.9")), Ok(()));
        assert_eq!(verify(data, sig, PUBKEY, None), Ok(()));
        // Same signature, but the manifest claims another version: refused.
        assert!(verify(data, sig, PUBKEY, Some("10.0.0")).is_err());
        // One changed byte: refused.
        let mut bad = data.to_vec();
        bad[0] ^= 1;
        assert!(verify(&bad, sig, PUBKEY, Some("9.9.9")).is_err());
        assert!(verify(b"data", "!!!", PUBKEY, None).is_err());
    }
}
