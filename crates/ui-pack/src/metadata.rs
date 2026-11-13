//! Minimal pure-Rust parser for `[package]` and `[package.metadata.ui]` in `Cargo.toml`.

use std::fs;
use std::path::Path;

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
}

impl AppMetadata {
    pub fn from_manifest(path: &Path) -> Result<Self, String> {
        let src = fs::read_to_string(path)
            .map_err(|e| format!("failed to read {}: {e}", path.display()))?;
        Self::parse(&src)
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
                let val = v.trim().trim_matches('"').to_owned();
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
        })
    }
}
