//! `build.rs` helper that embeds a web build (`dist/`) into a `ui` app.
//!
//! ```ignore
//! // build.rs
//! fn main() {
//!     ui_build::web("dist").before("npm run build").emit();
//! }
//!
//! // main.rs
//! WebView::new().bundle("app", ui::embedded!().spa("index.html").immutable("assets/"))
//! ```
//!
//! `emit` writes `$OUT_DIR/ui_embedded.rs`, a table of `include_bytes!` entries
//! that `ui::embedded!()` serves, and tells Cargo to rebuild when the folder changes.
//!
//! The `before` command (run with `sh -c`, or `cmd /C` on Windows, from the
//! package root) runs when the folder is missing, in release builds, or when
//! `UI_BUILD_WEB=1`. `UI_BUILD_WEB=0` never runs it.

use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Starts configuring the embedded web build at `dir` (relative to the package root).
pub fn web(dir: impl Into<PathBuf>) -> Web {
    Web {
        dir: dir.into(),
        before: None,
    }
}

/// A web build folder to embed; see [`web`].
#[derive(Debug, Clone)]
pub struct Web {
    dir: PathBuf,
    before: Option<String>,
}

impl Web {
    /// A shell command that produces the folder, such as `"npm run build"`.
    pub fn before(mut self, command: impl Into<String>) -> Self {
        self.before = Some(command.into());
        self
    }

    /// Runs `before` if needed, then writes the file table for `ui::embedded!()`.
    ///
    /// # Panics
    ///
    /// Panics (failing the build) when the `before` command fails or `OUT_DIR`
    /// can't be written. A folder that is still missing only warns, and embeds
    /// nothing, so `cargo check` works before the first web build.
    pub fn emit(self) {
        let manifest_dir = PathBuf::from(
            env::var_os("CARGO_MANIFEST_DIR").expect("ui_build::web must run from build.rs"),
        );
        let out_dir =
            PathBuf::from(env::var_os("OUT_DIR").expect("ui_build::web must run from build.rs"));
        let dir = manifest_dir.join(&self.dir);

        println!("cargo:rerun-if-changed={}", dir.display());
        println!("cargo:rerun-if-env-changed=UI_BUILD_WEB");

        if let Some(cmd) = &self.before {
            let release = env::var("PROFILE").is_ok_and(|p| p == "release");
            let flag = env::var("UI_BUILD_WEB").ok();
            if should_build(dir.is_dir(), release, flag.as_deref()) {
                run_before(cmd, &manifest_dir);
            }
        }

        let files = if dir.is_dir() {
            collect(&dir).unwrap_or_else(|e| panic!("ui-build: reading {}: {e}", dir.display()))
        } else {
            println!(
                "cargo:warning=ui-build: {} does not exist; embedding no files",
                dir.display()
            );
            Vec::new()
        };
        let out = out_dir.join("ui_embedded.rs");
        fs::write(&out, generate(&files))
            .unwrap_or_else(|e| panic!("ui-build: writing {}: {e}", out.display()));
    }
}

/// Whether the `before` command should run.
fn should_build(dir_exists: bool, release: bool, flag: Option<&str>) -> bool {
    match flag.map(str::trim) {
        Some("0") => false,
        Some("1") => true,
        _ => !dir_exists || release,
    }
}

fn run_before(cmd: &str, cwd: &Path) {
    println!("cargo:warning=ui-build: running `{cmd}`");
    let status = if cfg!(windows) {
        Command::new("cmd").args(["/C", cmd]).current_dir(cwd).status()
    } else {
        Command::new("sh").args(["-c", cmd]).current_dir(cwd).status()
    };
    match status {
        Ok(s) if s.success() => {}
        Ok(s) => panic!("ui-build: `{cmd}` failed ({s})"),
        Err(e) => panic!("ui-build: could not run `{cmd}`: {e}"),
    }
}

/// Every file under `root` as (forward-slash relative path, absolute path), sorted.
/// Hidden files and folders (`.DS_Store`, `.vite`) are skipped.
fn collect(root: &Path) -> std::io::Result<Vec<(String, PathBuf)>> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let name = entry.file_name();
            if name.to_string_lossy().starts_with('.') {
                continue;
            }
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                stack.push(path);
            } else {
                let rel = path
                    .strip_prefix(root)
                    .expect("walked path is under root")
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/");
                let abs = fs::canonicalize(&path)?;
                out.push((rel, abs));
            }
        }
    }
    out.sort();
    Ok(out)
}

/// The Rust expression `ui::embedded!()` includes: a `&[(&str, &[u8])]` slice.
fn generate(files: &[(String, PathBuf)]) -> String {
    let mut s = String::from("// @generated by ui-build. Do not edit.\n&[\n");
    for (rel, abs) in files {
        let _ = writeln!(
            s,
            "    ({rel:?}, ::std::include_bytes!({:?}) as &[u8]),",
            abs.to_string_lossy()
        );
    }
    s.push(']');
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_runs_when_missing_in_release_or_forced() {
        assert!(should_build(false, false, None));
        assert!(!should_build(true, false, None));
        assert!(should_build(true, true, None));
        assert!(should_build(true, false, Some("1")));
        assert!(!should_build(false, true, Some("0")));
    }

    #[test]
    fn collects_sorted_relative_paths_and_skips_hidden() {
        let root = env::temp_dir().join(format!("ui-build-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("assets/img")).unwrap();
        fs::create_dir_all(root.join(".vite")).unwrap();
        fs::write(root.join("index.html"), "<html>").unwrap();
        fs::write(root.join("assets/app.js"), "export {}").unwrap();
        fs::write(root.join("assets/img/a \"b\".png"), [0u8, 1]).unwrap();
        fs::write(root.join(".DS_Store"), "x").unwrap();
        fs::write(root.join(".vite/manifest.json"), "{}").unwrap();

        let files = collect(&root).unwrap();
        let rels: Vec<&str> = files.iter().map(|(r, _)| r.as_str()).collect();
        assert_eq!(rels, ["assets/app.js", "assets/img/a \"b\".png", "index.html"]);

        let code = generate(&files);
        assert!(code.starts_with("// @generated"));
        assert!(code.contains(r#"("assets/img/a \"b\".png", ::std::include_bytes!("#));
        assert!(code.trim_end().ends_with(']'));
        fs::remove_dir_all(&root).unwrap();
    }
}
