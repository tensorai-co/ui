//! The dev loop: `ui-pack run` (build, sign, launch) and `ui-pack gen-ts`.

use std::path::{Path, PathBuf};
use std::process::{Child, Command};

use packager::sign::apple::{CodeSignOptions, sign_macho};

use crate::metadata::UiMetadata;

/// Options for [`run`].
#[derive(Debug, Clone, Default)]
pub struct RunOptions {
    /// The app's `Cargo.toml`.
    pub manifest: PathBuf,
    /// Build with `--release`.
    pub release: bool,
    /// Start `dev_command` and point the app at `dev_url` via `UI_DEV_URL`.
    pub dev: bool,
    /// Extra arguments passed to the app.
    pub app_args: Vec<String>,
}

/// Builds the package's binary, signs it ad hoc with the entitlements from
/// `[package.metadata.packager.sign.apple]` (macOS), and runs it, returning its exit code.
/// With `dev`, the web dev server runs alongside and `UI_DEV_URL` points the app at it.
pub fn run(meta: &UiMetadata, opts: &RunOptions) -> Result<i32, String> {
    let bin = cargo_build(&opts.manifest, &meta.crate_name, opts.release)?;
    // The packaging config is optional for running; it supplies entitlements and the icon.
    let packaging = packager::config::from_cargo_manifest(&opts.manifest).ok();
    if cfg!(target_os = "macos")
        && let Some(cfg) = &packaging
        && let Some(entitlements) = cfg.apple.as_ref().and_then(|a| a.entitlements.clone())
    {
        sign_ad_hoc(&bin, &cfg.app.identifier, entitlements)?;
    }

    let mut dev_server: Option<Child> = None;
    let mut cmd = Command::new(&bin);
    cmd.args(&opts.app_args);
    // Unbundled binaries show a generic Dock icon; point them at the app's.
    if let Some(icon) = packaging.as_ref().and_then(|c| c.app.icon.as_ref()) {
        cmd.env("UI_DOCK_ICON", icon);
    }
    if opts.dev {
        let url = meta
            .dev_url
            .clone()
            .unwrap_or_else(|| "http://localhost:5173".into());
        if let Some(dev_cmd) = &meta.dev_command {
            eprintln!("ui-pack: starting `{dev_cmd}`");
            dev_server = Some(shell(dev_cmd, &meta.manifest_dir)?);
        }
        cmd.env("UI_DEV_URL", &url);
    }
    let status = cmd
        .status()
        .map_err(|e| format!("launch {}: {e}", bin.display()));
    if let Some(mut child) = dev_server {
        let _ = child.kill();
        let _ = child.wait();
    }
    Ok(status?.code().unwrap_or(1))
}

/// Signs a binary ad hoc with entitlements, in place (what `codesign -s - --entitlements` does).
fn sign_ad_hoc(bin: &Path, identifier: &str, entitlements: Vec<u8>) -> Result<(), String> {
    let data = std::fs::read(bin).map_err(|e| format!("{}: {e}", bin.display()))?;
    let mut opts = CodeSignOptions::ad_hoc(identifier);
    opts.entitlements = Some(entitlements);
    let signed = sign_macho(&data, &opts).map_err(|e| format!("sign {}: {e}", bin.display()))?;
    std::fs::write(bin, signed.data).map_err(|e| format!("{}: {e}", bin.display()))
}

/// Builds the app and runs it with `UI_GEN_TS=<out>`, so its
/// `ui::write_typescript_if_requested(..)` writes the `ipc_types!` declarations.
pub fn gen_ts(meta: &UiMetadata, manifest: &Path, out: &Path) -> Result<PathBuf, String> {
    let bin = cargo_build(manifest, &meta.crate_name, false)?;
    let out = if out.is_absolute() {
        out.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join(out)
    };
    let _ = std::fs::remove_file(&out);
    let status = Command::new(&bin)
        .env("UI_GEN_TS", &out)
        .status()
        .map_err(|e| format!("run {}: {e}", bin.display()))?;
    if !status.success() || !out.exists() {
        return Err(format!(
            "{} didn't write {}: call `ui::write_typescript_if_requested(..)` at the start of main",
            bin.display(),
            out.display()
        ));
    }
    Ok(out)
}

fn shell(cmd: &str, cwd: &Path) -> Result<Child, String> {
    let mut c = if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.args(["/C", cmd]);
        c
    } else {
        let mut c = Command::new("sh");
        c.args(["-c", cmd]);
        c
    };
    c.current_dir(cwd)
        .spawn()
        .map_err(|e| format!("start `{cmd}`: {e}"))
}

/// Runs `cargo build` for the package and returns the built binary's path.
pub fn cargo_build(manifest: &Path, bin_name: &str, release: bool) -> Result<PathBuf, String> {
    let mut cmd = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    cmd.args([
        "build",
        "--message-format=json-render-diagnostics",
        "--manifest-path",
    ])
    .arg(manifest)
    .args(["--bin", bin_name]);
    if release {
        cmd.arg("--release");
    }
    let out = cmd
        .stderr(std::process::Stdio::inherit())
        .output()
        .map_err(|e| format!("run cargo: {e}"))?;
    if !out.status.success() {
        return Err("cargo build failed".into());
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| l.contains(r#""reason":"compiler-artifact""#))
        .filter_map(executable_of)
        .next_back()
        .ok_or_else(|| format!("cargo built no binary named `{bin_name}`"))
}

/// The `"executable"` path in one line of cargo's JSON output, if any.
fn executable_of(line: &str) -> Option<PathBuf> {
    let start = line.find(r#""executable":""#)? + r#""executable":""#.len();
    let mut out = String::new();
    let mut chars = line[start..].chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(PathBuf::from(out)),
            '\\' => match chars.next()? {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                other => out.push(other),
            },
            c => out.push(c),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_executable_from_cargo_json() {
        let line = r#"{"reason":"compiler-artifact","target":{"name":"app"},"executable":"/w/target/debug/my\"app","fresh":true}"#;
        assert_eq!(
            executable_of(line),
            Some(PathBuf::from("/w/target/debug/my\"app"))
        );
        assert_eq!(
            executable_of(r#"{"reason":"compiler-artifact","executable":null}"#),
            None
        );
    }
}
