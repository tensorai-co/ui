//! Signed update checks: fetch a small JSON feed, verify its ECDSA P-256
//! signature against the app's embedded public key, and compare versions.
//!
//! Feed format (written by `packager update-feed`):
//!
//! ```json
//! { "payload": "{\"version\":\"1.2.0\",\"url\":\"https://…/App.zip\",\"notes\":\"…\"}",
//!   "signature": "<base64 DER ECDSA-SHA256 signature of the payload string's bytes>" }
//! ```
//!
//! The signature covers the payload's exact bytes, so nothing needs to be
//! re-serialized to verify it. macOS only for now (Security framework).

use crate::ipc::{Value, decode_base64, parse};
use crate::{Error, Result};

/// A newer version announced by a verified feed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateInfo {
    pub version: String,
    /// Where to download it.
    pub url: String,
    pub notes: Option<String>,
}

/// Fetches `feed_url`, verifies it with `public_key`, and returns the update
/// if its version is newer than `current_version` (e.g. `env!("CARGO_PKG_VERSION")`).
///
/// `public_key` is the base64 printed by `packager update-key` (an uncompressed
/// P-256 point). Only `https://` feeds are fetched (`http://localhost` too, for
/// testing). Blocks while fetching: call it from a worker thread or `invoke_blocking`.
pub fn check(
    feed_url: &str,
    public_key: &str,
    current_version: &str,
) -> Result<Option<UpdateInfo>> {
    if !fetch_allowed(feed_url) {
        return Err(Error::InvalidState(format!(
            "update feeds must be https:// URLs, not \"{feed_url}\""
        )));
    }
    let body = fetch(feed_url)?;
    let info = verify_feed(&body, public_key)?;
    Ok(
        (compare_versions(&info.version, current_version) == std::cmp::Ordering::Greater)
            .then_some(info),
    )
}

/// Verifies a feed's signature and returns what it announces, whatever its version.
pub fn verify_feed(feed_json: &str, public_key: &str) -> Result<UpdateInfo> {
    let feed = parse(feed_json).map_err(Error::Ipc)?;
    let text = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).map(str::to_owned);
    let payload = text(&feed, "payload").ok_or_else(|| bad("feed has no \"payload\""))?;
    let signature = text(&feed, "signature").ok_or_else(|| bad("feed has no \"signature\""))?;
    let key = decode_base64(public_key.trim()).map_err(|_| bad("public key isn't base64"))?;
    let sig = decode_base64(signature.trim()).map_err(|_| bad("signature isn't base64"))?;
    if !verify_p256(&key, payload.as_bytes(), &sig)? {
        return Err(Error::InvalidState(
            "update feed signature doesn't match the app's key".into(),
        ));
    }
    let p = parse(&payload).map_err(Error::Ipc)?;
    Ok(UpdateInfo {
        version: text(&p, "version").ok_or_else(|| bad("payload has no \"version\""))?,
        url: text(&p, "url").ok_or_else(|| bad("payload has no \"url\""))?,
        notes: text(&p, "notes"),
    })
}

fn bad(msg: &str) -> Error {
    Error::InvalidState(format!("invalid update feed: {msg}"))
}

fn fetch_allowed(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    lower.starts_with("https://")
        || lower.starts_with("http://localhost")
        || lower.starts_with("http://127.0.0.1")
}

fn fetch(url: &str) -> Result<String> {
    let out = std::process::Command::new("/usr/bin/curl")
        .args([
            "-fsSL",
            "--max-time",
            "20",
            "--proto",
            "=https,http",
            "--",
            url,
        ])
        .output()
        .map_err(|e| Error::Io(format!("run curl: {e}")))?;
    if !out.status.success() {
        return Err(Error::Io(format!(
            "fetching {url} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    String::from_utf8(out.stdout).map_err(|_| Error::Io("update feed isn't UTF-8".into()))
}

/// Compares dotted versions numerically (`1.10.0` > `1.9.2`); a pre-release
/// suffix (`1.2.0-beta`) sorts before the plain version.
pub fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    fn split(v: &str) -> (Vec<u64>, bool) {
        let v = v.trim().trim_start_matches('v');
        let (core, pre) = match v.split_once('-') {
            Some((c, _)) => (c, true),
            None => (v, false),
        };
        (
            core.split('.').map(|p| p.parse().unwrap_or(0)).collect(),
            pre,
        )
    }
    let ((mut na, pa), (mut nb, pb)) = (split(a), split(b));
    let len = na.len().max(nb.len());
    na.resize(len, 0);
    nb.resize(len, 0);
    na.cmp(&nb).then(pb.cmp(&pa))
}

#[cfg(target_os = "macos")]
fn verify_p256(key: &[u8], message: &[u8], signature: &[u8]) -> Result<bool> {
    crate::backend::macos_security::verify_p256(key, message, signature)
}

#[cfg(not(target_os = "macos"))]
fn verify_p256(_key: &[u8], _message: &[u8], _signature: &[u8]) -> Result<bool> {
    Err(Error::Unsupported(
        "update signature checks are macOS-only for now",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering::*;

    #[test]
    fn version_order() {
        assert_eq!(compare_versions("1.10.0", "1.9.2"), Greater);
        assert_eq!(compare_versions("v2.0", "2.0.0"), Equal);
        assert_eq!(compare_versions("1.2.0-beta", "1.2.0"), Less);
        assert_eq!(compare_versions("0.1.0", "0.2.0"), Less);
    }

    #[test]
    fn only_https_feeds() {
        assert!(fetch_allowed("https://example.com/feed.json"));
        assert!(fetch_allowed("http://127.0.0.1:8080/feed.json"));
        assert!(!fetch_allowed("http://example.com/feed.json"));
        assert!(!fetch_allowed("file:///etc/passwd"));
    }
}
