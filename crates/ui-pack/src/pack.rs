//! `ui-pack pack`: native packages through packager, from `[package.metadata.packager]`, plus
//! what a `ui` app adds: with `backend = "cef"`, the Chromium Embedded Framework and its helper
//! apps go into the `.app`.

use std::path::{Path, PathBuf};

use packager::Package;
use packager::config::Config;

use crate::metadata::{BackendTarget, UiMetadata};
use crate::run::cargo_build;

/// The helper apps CEF runs its processes in, next to the framework.
pub const CEF_HELPERS: [&str; 5] = [
    "ui Helper.app",
    "ui Helper (Alerts).app",
    "ui Helper (GPU).app",
    "ui Helper (Plugin).app",
    "ui Helper (Renderer).app",
];

/// Options for [`pack`].
#[derive(Debug, Clone, Default)]
pub struct PackOptions {
    /// The app's `Cargo.toml`.
    pub manifest: PathBuf,
    /// Only these package kinds (`app`, `dmg`, `pkg`, `msi`, `msix`, `msixbundle`, `deb`, `snap`).
    pub only: Option<Vec<String>>,
    /// Where packages go (default `[build] out_dir`, else `<target>/dist`).
    pub out_dir: Option<PathBuf>,
    /// Binaries to package instead of the configured ones; each one's architecture is read
    /// from the file.
    pub binaries: Vec<PathBuf>,
    /// Run `cargo build --release` first (when no `binaries` are given).
    pub build: bool,
    /// Notarize when `[package.metadata.packager.notarize]` is set.
    pub notarize: bool,
}

/// Builds the release binary if asked, then the configured packages.
pub fn pack(meta: &UiMetadata, opts: &PackOptions) -> Result<Vec<Package>, String> {
    if opts.build && opts.binaries.is_empty() {
        cargo_build(&opts.manifest, &meta.crate_name, true)?;
    }
    let mut cfg =
        packager::config::from_cargo_manifest(&opts.manifest).map_err(|e| e.to_string())?;
    if !opts.binaries.is_empty() {
        cfg.app.binaries.clear();
        for b in &opts.binaries {
            cfg.app = cfg.app.clone().binary_file(b).map_err(|e| e.to_string())?;
        }
    }
    if meta.backend == BackendTarget::Cef {
        add_cef(&mut cfg)?;
    }
    let only: Option<Vec<&str>> = opts
        .only
        .as_ref()
        .map(|o| o.iter().map(String::as_str).collect());
    let out = cfg
        .run(only.as_deref(), opts.out_dir.as_deref(), opts.notarize)
        .map_err(|e| e.to_string())?;
    Ok(out.packages().to_vec())
}

/// Adds `Chromium Embedded Framework.framework` and the `ui Helper*.app`s found next to the
/// macOS binary (where `ui`'s `cef` build puts them), with the helpers taking the app's identifier.
pub fn add_cef(cfg: &mut Config) -> Result<(), String> {
    let Some(bin) = cfg
        .app
        .macos_binaries()
        .first()
        .map(|(_, p)| p.to_path_buf())
    else {
        return Ok(());
    };
    let framework = find_cef_framework(&bin).ok_or_else(|| {
        format!(
            "backend = \"cef\", but there's no Chromium Embedded Framework.framework near {}: \
             build with `--features cef`",
            bin.display()
        )
    })?;
    let mut bundle = cfg.app_bundle.clone().framework(&framework);
    if let Some(dir) = framework.parent() {
        for helper in CEF_HELPERS {
            let path = dir.join(helper);
            if path.exists() {
                bundle = bundle.helper(path, Some(cfg.app.identifier.clone()));
            }
        }
    }
    cfg.app_bundle = bundle;
    Ok(())
}

/// The framework in the binary's directory or up to two levels above it.
pub fn find_cef_framework(binary: &Path) -> Option<PathBuf> {
    binary
        .ancestors()
        .skip(1)
        .take(3)
        .map(|d| d.join("Chromium Embedded Framework.framework"))
        .find(|p| p.exists())
}
