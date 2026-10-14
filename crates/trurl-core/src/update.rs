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

/// How often a running Trurl looks for a new version.
pub const CHECK_EVERY: Duration = Duration::from_secs(24 * 60 * 60);
/// The first check waits for the model to load: nothing touches the network
/// while the learner's first answer is on its way.
pub const FIRST_CHECK_AFTER: Duration = Duration::from_secs(15);
/// How often the schedule looks at the clock. Short, so a laptop that slept
/// through the 24 hours checks soon after it wakes up.
pub const SCHEDULE_TICK: Duration = Duration::from_secs(10 * 60);
/// A check that failed (no internet yet, GitHub down, a slow start) is tried
/// again after this, not 24 hours later. Each further failure in a row doubles
/// the wait, up to `CHECK_EVERY`: a broken manifest never means 48 requests a day.
pub const RETRY_AFTER: Duration = Duration::from_secs(30 * 60);

/// When to check for updates: once at start, then every 24 hours of wall-clock
/// time (a timer would stop while the laptop sleeps). Both apps use this, so
/// "Later" means the same everywhere: the offer comes back at the next start or
/// the next 24-hour check, never sooner. A failed check is retried after
/// `RETRY_AFTER` (then 1 h, 2 h, ... up to 24 h): one bad moment at start must
/// not hide an update for a day.
#[derive(Debug, Default, Clone)]
pub struct CheckSchedule {
    last: Option<(std::time::SystemTime, Duration)>,
    failures: u32,
}

impl CheckSchedule {
    pub fn due(&self, now: std::time::SystemTime) -> bool {
        match self.last {
            None => true,
            Some((last, wait)) => match now.duration_since(last) {
                Ok(d) => d >= wait,
                // The clock went back (a dead CMOS battery, a time zone fix): do
                // not wait for it to catch up again.
                Err(_) => true,
            },
        }
    }

    /// GitHub answered (with or without a newer version): next check in 24 hours.
    pub fn checked(&mut self, now: std::time::SystemTime) {
        self.last = Some((now, CHECK_EVERY));
        self.failures = 0;
    }

    /// The check failed: try again after 30 min, then 1 h, 2 h, ... at most 24 h.
    pub fn failed(&mut self, now: std::time::SystemTime) {
        let wait = RETRY_AFTER
            .saturating_mul(1 << self.failures.min(16))
            .min(CHECK_EVERY);
        self.failures = self.failures.saturating_add(1);
        self.last = Some((now, wait));
    }

    /// How long until the next check after the last outcome (for About's line).
    pub fn retry_wait(&self) -> Option<Duration> {
        self.last.map(|(_, wait)| wait)
    }

    /// Record the outcome of a check that ran at `now`.
    pub fn record<T, E>(&mut self, now: std::time::SystemTime, outcome: &Result<T, E>) {
        match outcome {
            Ok(_) => self.checked(now),
            Err(_) => self.failed(now),
        }
    }
}

/// What the last automatic check did, for About (`update_status` in
/// contracts/events.md). `outcome` is what the check returned: the update's
/// info (`version`, `notes`, `date`) when one is newer, `null` when not.
/// `retry_in` is the wait the schedule chose after it.
pub fn check_status(
    at: u64,
    outcome: &Result<serde_json::Value, UserError>,
    retry_in: Option<Duration>,
) -> serde_json::Value {
    let mut s = match outcome {
        Ok(serde_json::Value::Null) => serde_json::json!({ "outcome": "latest" }),
        Ok(info) => serde_json::json!({ "outcome": "newer", "update": info }),
        Err(e) => serde_json::json!({ "outcome": "failed", "error": e }),
    };
    s["at"] = at.into();
    if let Some(w) = retry_in {
        s["retry_in_mins"] = (w.as_secs() / 60).into();
    }
    s
}

/// A check that started at `at` and has not returned yet.
pub fn running_status(at: u64) -> serde_json::Value {
    serde_json::json!({ "at": at, "outcome": "running" })
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
    // No release published yet (or none for this platform): nothing newer exists.
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !resp.status().is_success() {
        return Err(UserError::update_check_failed().with_detail(format!("HTTP {}", resp.status())));
    }
    let m: Manifest = crate::net::json_limited(resp, 1024 * 1024)
        .await
        .map_err(|e| UserError::update_check_failed().with_detail(e))?;
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

/// No Trurl release comes near this (about 30 MB); a bigger reply is not one.
pub const MAX_UPDATE_BYTES: u64 = 300 * 1024 * 1024;

/// Download the update and verify it. Returns the verified bytes.
pub async fn download(
    http: &reqwest::Client,
    info: &UpdateInfo,
    mut progress: impl FnMut(u64, Option<u64>),
) -> Result<Vec<u8>, UserError> {
    use futures_util::StreamExt;
    // No limit on the whole download (a 30 MB update takes 10 minutes at 400
    // kbit/s): it fails only when nothing arrives for a minute.
    let quiet = Duration::from_secs(60);
    let failed = |d: String| UserError::update_failed().with_detail(d);
    let resp = tokio::time::timeout(quiet, http.get(&info.url).send())
        .await
        .map_err(|_| failed("no answer for a minute".into()))?
        .map_err(|e| failed(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(failed(format!("HTTP {}", resp.status())));
    }
    let total = resp.content_length();
    if total.is_some_and(|t| t > MAX_UPDATE_BYTES) {
        return Err(failed(format!(
            "the update is {total:?} bytes, more than any Trurl release"
        )));
    }
    let mut buf = Vec::with_capacity(total.unwrap_or(0) as usize);
    let mut stream = resp.bytes_stream();
    loop {
        let next = tokio::time::timeout(quiet, stream.next())
            .await
            .map_err(|_| failed("no data for a minute".into()))?;
        let Some(chunk) = next else { break };
        let chunk = chunk.map_err(|e| failed(e.to_string()))?;
        buf.extend_from_slice(&chunk);
        if buf.len() as u64 > MAX_UPDATE_BYTES {
            return Err(failed("the update is larger than any Trurl release".into()));
        }
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
    fn checks_at_start_then_every_24_hours_of_wall_clock() {
        use std::time::{Duration, SystemTime};
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let mut s = CheckSchedule::default();
        assert!(s.due(t0), "first check at start");
        s.checked(t0);
        assert!(!s.due(t0 + Duration::from_secs(60)));
        assert!(
            !s.due(t0 + CHECK_EVERY - Duration::from_secs(1)),
            "\"Later\" holds for 24 h"
        );
        assert!(s.due(t0 + CHECK_EVERY));
        // A laptop asleep for three days checks on the first tick after waking.
        assert!(s.due(t0 + CHECK_EVERY * 3));
        // A clock set backwards does not silence checks for days.
        assert!(s.due(t0 - Duration::from_secs(3600)));
        s.checked(t0 + CHECK_EVERY);
        assert!(!s.due(t0 + CHECK_EVERY + SCHEDULE_TICK));
    }

    #[test]
    fn a_failed_check_is_retried_after_30_minutes_not_24_hours() {
        use std::time::{Duration, SystemTime};
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let mut s = CheckSchedule::default();
        // The first check at start gets no answer (Wi-Fi not up yet, GitHub down).
        s.record::<(), ()>(t0, &Err(()));
        assert!(!s.due(t0 + SCHEDULE_TICK), "not hammering every tick");
        assert!(!s.due(t0 + RETRY_AFTER - Duration::from_secs(1)));
        assert!(s.due(t0 + RETRY_AFTER), "tried again after 30 minutes");
        // The retry gets an answer: back to once a day.
        let t1 = t0 + RETRY_AFTER;
        s.record::<(), ()>(t1, &Ok(()));
        assert!(!s.due(t1 + RETRY_AFTER));
        assert!(!s.due(t1 + CHECK_EVERY - Duration::from_secs(1)));
        assert!(s.due(t1 + CHECK_EVERY));
        // A clock set backwards after a failure still checks.
        s.failed(t1);
        assert!(s.due(t1 - Duration::from_secs(60)));
    }

    #[test]
    fn failures_in_a_row_back_off_to_once_a_day() {
        use std::time::{Duration, SystemTime};
        let t = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let mut s = CheckSchedule::default();
        let mins = |s: &CheckSchedule| s.retry_wait().unwrap().as_secs() / 60;
        let mut seen = vec![];
        for _ in 0..8 {
            s.failed(t);
            seen.push(mins(&s));
        }
        // A broken manifest costs a handful of requests, then one a day.
        assert_eq!(seen, [30, 60, 120, 240, 480, 960, 1440, 1440]);
        // One answer resets it: the next failure waits 30 minutes again.
        s.checked(t);
        assert_eq!(mins(&s), 1440);
        s.failed(t);
        assert_eq!(mins(&s), 30);
        // Thousands of failures never overflow.
        for _ in 0..5000 {
            s.failed(t);
        }
        assert_eq!(mins(&s), 1440);
    }

    #[test]
    fn about_gets_what_the_last_automatic_check_did() {
        let newer = serde_json::json!({ "version": "0.1.2", "notes": "Faster.", "date": null });
        let s = check_status(100, &Ok(newer.clone()), Some(CHECK_EVERY));
        assert_eq!(s["outcome"], "newer");
        assert_eq!(s["update"], newer, "the full update, so About can offer it");
        assert_eq!(s["at"], 100);
        assert_eq!(s["retry_in_mins"], 1440);

        let s = check_status(100, &Ok(serde_json::Value::Null), Some(CHECK_EVERY));
        assert_eq!(s["outcome"], "latest");
        assert!(s.get("update").is_none());

        let err = UserError::update_check_failed().with_detail("operation timed out");
        let s = check_status(100, &Err(err), Some(RETRY_AFTER));
        assert_eq!(s["outcome"], "failed");
        assert_eq!(s["error"]["detail"], "operation timed out");
        assert_eq!(s["error"]["title"], "Trurl could not check for updates.");
        assert_eq!(s["retry_in_mins"], 30);

        assert_eq!(
            running_status(7),
            serde_json::json!({ "at": 7, "outcome": "running" })
        );
    }

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
