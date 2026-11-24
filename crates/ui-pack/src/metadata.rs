//! The `ui` keys in `Cargo.toml`: `[package] name` and `[package.metadata.ui]` (`backend`,
//! `dev_command`, `dev_url`). Packaging is configured in `[package.metadata.packager]` and read
//! by packager.

use std::fs;
use std::path::{Path, PathBuf};

use packager::format::toml::{self, Toml};

/// The webview backend the app is built for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BackendTarget {
    /// Whatever the platform defaults to.
    #[default]
    Auto,
    /// The system WebKit (macOS).
    WebKit,
    /// Chromium Embedded Framework: packages carry the framework and its helper apps.
    Cef,
}

/// What `ui-pack` needs to know about the app besides its packaging.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiMetadata {
    /// `[package] name`: the binary `run`, `gen-ts` and `pack` build.
    pub crate_name: String,
    /// `backend`.
    pub backend: BackendTarget,
    /// `dev_command`: starts the web dev server for `ui-pack run --dev`.
    pub dev_command: Option<String>,
    /// `dev_url`: the dev server's URL for `ui-pack run --dev` (default `http://localhost:5173`).
    pub dev_url: Option<String>,
    /// The folder holding the manifest.
    pub manifest_dir: PathBuf,
}

/// Packaging keys `[package.metadata.ui]` used to take, and where they live now.
const MOVED: [(&str, &str); 10] = [
    ("name", "[package.metadata.packager.app] name"),
    ("bundle_id", "[package.metadata.packager.app] identifier"),
    ("icon", "[package.metadata.packager.app] icon"),
    ("url_schemes", "[package.metadata.packager.app] url_schemes"),
    (
        "document_types",
        "[package.metadata.packager.app] file_types",
    ),
    (
        "category",
        "[package.metadata.packager.package.app] category",
    ),
    (
        "min_macos",
        "[package.metadata.packager.package.app] min_macos",
    ),
    (
        "entitlements",
        "[package.metadata.packager.sign.apple] entitlements",
    ),
    (
        "microphone_usage",
        "[package.metadata.packager.package.app.info_plist] NSMicrophoneUsageDescription",
    ),
    (
        "speech_usage",
        "[package.metadata.packager.package.app.info_plist] NSSpeechRecognitionUsageDescription",
    ),
];

impl UiMetadata {
    /// Reads a `Cargo.toml`.
    pub fn from_manifest(path: &Path) -> Result<Self, String> {
        let src = fs::read_to_string(path)
            .map_err(|e| format!("failed to read {}: {e}", path.display()))?;
        let mut meta = Self::parse(&src).map_err(|e| format!("{}: {e}", path.display()))?;
        meta.manifest_dir = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        Ok(meta)
    }

    /// Parses a `Cargo.toml`'s text.
    pub fn parse(src: &str) -> Result<Self, String> {
        let root = toml::parse(src)?;
        let package = root.get("package").ok_or("missing [package] table")?;
        let crate_name = package
            .get("name")
            .and_then(Toml::as_str)
            .ok_or("missing `[package] name`")?
            .to_owned();
        let empty = Toml::Table(Default::default());
        let ui = package
            .get("metadata")
            .and_then(|m| m.get("ui"))
            .unwrap_or(&empty);
        for (key, now) in MOVED {
            if ui.get(key).is_some() {
                return Err(format!(
                    "`[package.metadata.ui] {key}` is packaging, which packager does now: \
                     move it to {now}"
                ));
            }
        }
        let text = |k: &str| ui.get(k).and_then(Toml::as_str).map(String::from);
        let backend = match text("backend").as_deref() {
            None | Some("auto") => BackendTarget::Auto,
            Some("webkit") => BackendTarget::WebKit,
            Some("cef") => BackendTarget::Cef,
            Some(other) => {
                return Err(format!(
                    "`[package.metadata.ui] backend = \"{other}\"`: expected webkit or cef"
                ));
            }
        };
        Ok(Self {
            crate_name,
            backend,
            dev_command: text("dev_command"),
            dev_url: text("dev_url"),
            manifest_dir: PathBuf::from("."),
        })
    }
}
