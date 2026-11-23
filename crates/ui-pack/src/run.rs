//! `ui-pack run` (build, sign, launch) and `ui-pack icons` (iconset + .icns).

use std::path::{Path, PathBuf};
use std::process::{Child, Command};

use crate::macos::{codesign, entitlements_path};
use crate::metadata::AppMetadata;

/// Options for [`run`].
#[derive(Debug, Clone, Default)]
pub struct RunOptions {
    pub manifest: PathBuf,
    pub release: bool,
    /// Start `dev_command` and point the app at `dev_url` via `UI_DEV_URL`.
    pub dev: bool,
    /// Extra arguments passed to the app.
    pub app_args: Vec<String>,
}

/// Builds the package's binary, signs it ad hoc with the manifest's
/// entitlements (macOS), and runs it, returning its exit code. With `dev`, the
/// web dev server runs alongside and `UI_DEV_URL` points the app at it.
pub fn run(meta: &AppMetadata, opts: &RunOptions) -> Result<i32, String> {
    let bin = cargo_build(&opts.manifest, &meta.crate_name, opts.release)?;
    if cfg!(target_os = "macos")
        && let Some(ent) = entitlements_path(meta)
    {
        codesign(&bin, "-", Some(&ent))?;
    }

    let mut dev_server: Option<Child> = None;
    let mut cmd = Command::new(&bin);
    cmd.args(&opts.app_args);
    if opts.dev {
        let url = meta.dev_url.clone().unwrap_or_else(|| "http://localhost:5173".into());
        if let Some(dev_cmd) = &meta.dev_command {
            eprintln!("ui-pack: starting `{dev_cmd}`");
            dev_server = Some(shell(dev_cmd, &meta.manifest_dir)?);
        }
        cmd.env("UI_DEV_URL", &url);
    }
    let status = cmd.status().map_err(|e| format!("launch {}: {e}", bin.display()));
    if let Some(mut child) = dev_server {
        let _ = child.kill();
        let _ = child.wait();
    }
    Ok(status?.code().unwrap_or(1))
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
    c.current_dir(cwd).spawn().map_err(|e| format!("start `{cmd}`: {e}"))
}

/// Runs `cargo build` for the package and returns the built binary's path.
fn cargo_build(manifest: &Path, bin_name: &str, release: bool) -> Result<PathBuf, String> {
    let mut cmd = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    cmd.args(["build", "--message-format=json-render-diagnostics", "--manifest-path"])
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

/// Icon sizes in a macOS `.iconset`: (file name, pixel size).
pub const ICONSET_SIZES: [(&str, u32); 10] = [
    ("icon_16x16.png", 16),
    ("icon_16x16@2x.png", 32),
    ("icon_32x32.png", 32),
    ("icon_32x32@2x.png", 64),
    ("icon_128x128.png", 128),
    ("icon_128x128@2x.png", 256),
    ("icon_256x256.png", 256),
    ("icon_256x256@2x.png", 512),
    ("icon_512x512.png", 512),
    ("icon_512x512@2x.png", 1024),
];

/// Makes `<out>/AppIcon.iconset/` and `<out>/AppIcon.icns` from one large PNG
/// (1024x1024 recommended) with the system `sips` and `iconutil` tools.
pub fn make_icons(source: &Path, out_dir: &Path) -> Result<PathBuf, String> {
    let iconset = out_dir.join("AppIcon.iconset");
    std::fs::create_dir_all(&iconset).map_err(|e| format!("create {}: {e}", iconset.display()))?;
    for (name, px) in ICONSET_SIZES {
        let px = px.to_string();
        let status = Command::new("sips")
            .args(["-s", "format", "png", "-z", &px, &px])
            .arg(source)
            .arg("--out")
            .arg(iconset.join(name))
            .stdout(std::process::Stdio::null())
            .status()
            .map_err(|e| format!("run sips: {e}"))?;
        if !status.success() {
            return Err(format!("sips could not resize {}", source.display()));
        }
    }
    let icns = out_dir.join("AppIcon.icns");
    let status = Command::new("iconutil")
        .args(["-c", "icns"])
        .arg(&iconset)
        .arg("-o")
        .arg(&icns)
        .status()
        .map_err(|e| format!("run iconutil: {e}"))?;
    if !status.success() {
        return Err("iconutil failed".into());
    }
    Ok(icns)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_executable_from_cargo_json() {
        let line = r#"{"reason":"compiler-artifact","target":{"name":"app"},"executable":"/w/target/debug/my\"app","fresh":true}"#;
        assert_eq!(executable_of(line), Some(PathBuf::from("/w/target/debug/my\"app")));
        assert_eq!(executable_of(r#"{"reason":"compiler-artifact","executable":null}"#), None);
    }
}
