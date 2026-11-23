//! Minimal pure-Rust parser for `[package]` and `[package.metadata.ui]` in `Cargo.toml`.

use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum BackendTarget {
    #[default]
    Auto,
    WebKit,
    Cef,
}

/// Application packaging metadata read from `[package]` and `[package.metadata.ui]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppMetadata {
    pub crate_name: String,
    pub display_name: String,
    pub version: String,
    pub bundle_id: String,
    pub icon: Option<String>,
    pub backend: BackendTarget,
    pub category: String,
    pub min_macos: String,
    /// URL schemes the app handles, e.g. `["myapp"]` for `myapp://…` links.
    pub url_schemes: Vec<String>,
    /// File extensions the app opens, e.g. `["md", "txt"]`.
    pub document_types: Vec<String>,
    /// Entitlements plist (relative to the manifest) used when signing.
    pub entitlements: Option<String>,
    /// Command that starts the web dev server for `ui-pack run --dev`.
    pub dev_command: Option<String>,
    /// The dev server's URL for `ui-pack run --dev` (default `http://localhost:5173`).
    pub dev_url: Option<String>,
    /// Folder holding the manifest; relative paths above resolve against it.
    pub manifest_dir: PathBuf,
}

/// Parses a one-line TOML string array (`["a", "b"]`) or a single string.
fn parse_list(raw: &str) -> Vec<String> {
    let raw = raw.trim();
    let inner = raw
        .strip_prefix('[')
        .and_then(|r| r.strip_suffix(']'))
        .unwrap_or(raw);
    inner
        .split(',')
        .map(|v| v.trim().trim_matches('"').trim().to_owned())
        .filter(|v| !v.is_empty())
        .collect()
}

impl AppMetadata {
    pub fn from_manifest(path: &Path) -> Result<Self, String> {
        let src = fs::read_to_string(path)
            .map_err(|e| format!("failed to read {}: {e}", path.display()))?;
        let mut meta = Self::parse(&src)?;
        meta.manifest_dir = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        Ok(meta)
    }

    pub fn parse(toml_src: &str) -> Result<Self, String> {
        let mut crate_name = String::new();
        let mut version = String::from("0.1.0");
        let mut display_name = None;
        let mut bundle_id = None;
        let mut icon = None;
        let mut backend = BackendTarget::Auto;
        let mut category = String::from("public.app-category.developer-tools");
        let mut min_macos = String::from("13.0");
        let mut url_schemes = Vec::new();
        let mut document_types = Vec::new();
        let mut entitlements = None;
        let mut dev_command = None;
        let mut dev_url = None;

        let mut section = String::new();
        for raw_line in toml_src.lines() {
            let line = raw_line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            if line.starts_with('[') && line.ends_with(']') {
                section = line[1..line.len() - 1].trim().to_owned();
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                let key = k.trim();
                let raw = v.trim();
                let val = raw.trim_matches('"').to_owned();
                match section.as_str() {
                    "package" => match key {
                        "name" => crate_name = val,
                        "version" if !val.starts_with('{') => version = val,
                        _ => {}
                    },
                    "package.metadata.ui" => match key {
                        "name" => display_name = Some(val),
                        "bundle_id" => bundle_id = Some(val),
                        "icon" => icon = Some(val),
                        "category" => category = val,
                        "min_macos" => min_macos = val,
                        "url_schemes" => url_schemes = parse_list(raw),
                        "document_types" => document_types = parse_list(raw),
                        "entitlements" => entitlements = Some(val),
                        "dev_command" => dev_command = Some(val),
                        "dev_url" => dev_url = Some(val),
                        "backend" => {
                            backend = match val.as_str() {
                                "webkit" => BackendTarget::WebKit,
                                "cef" => BackendTarget::Cef,
                                _ => BackendTarget::Auto,
                            };
                        }
                        _ => {}
                    },
                    _ => {}
                }
            }
        }

        if crate_name.is_empty() {
            return Err("missing `[package] name` in Cargo.toml".into());
        }
        let disp = display_name.unwrap_or_else(|| crate_name.clone());
        let default_bid = format!(
            "com.example.{}",
            crate_name
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() {
                    c.to_ascii_lowercase()
                } else {
                    '-'
                })
                .collect::<String>()
        );
        Ok(Self {
            crate_name,
            display_name: disp,
            version,
            bundle_id: bundle_id.unwrap_or(default_bid),
            icon,
            backend,
            category,
            min_macos,
            url_schemes,
            document_types,
            entitlements,
            dev_command,
            dev_url,
            manifest_dir: PathBuf::from("."),
        })
    }
}
