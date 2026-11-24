//! The `ui-pack` / `cargo ui` command line.

use std::path::PathBuf;

use crate::metadata::UiMetadata;
use crate::pack::{PackOptions, pack};
use crate::run::{RunOptions, gen_ts, run};

const USAGE: &str = "\
Usage:
  ui-pack pack [--manifest Cargo.toml] [--only app,dmg,pkg,msi,msix,msixbundle,deb,snap]
               [--out dir] [--bin path]... [--no-build] [--no-notarize]
  ui-pack run [--manifest Cargo.toml] [--release] [--dev] [-- app args…]
  ui-pack gen-ts [--manifest Cargo.toml] [--out ipc.d.ts]

`pack` builds the release binary, then the packages in [package.metadata.packager] (packager's
configuration, in Cargo.toml). `cargo ui <command>` runs the same commands (`cargo install` puts
`cargo-ui` on PATH).";

fn fail(msg: impl std::fmt::Display) -> ! {
    eprintln!("error: {msg}");
    std::process::exit(1);
}

/// Where commands `ui-pack` no longer has went.
fn moved(command: &str) -> Option<&'static str> {
    Some(match command {
        "macos" => "`ui-pack pack --only app` (or dmg, pkg)",
        "msix" => "`ui-pack pack --only msix`",
        "linux" => "`ui-pack pack --only deb,snap`",
        "mas" | "mac-app-store" => {
            "nothing yet: Mac App Store packages are on packager's list; `ui-pack pack --only app` \
             builds a .app"
        }
        "icons" => {
            "packager, which makes every icon from the PNG in [package.metadata.packager.app] icon (`packager icns` writes an .icns)"
        }
        "update-key" => "`packager update-key`",
        "update-feed" => "`packager update-feed`",
        _ => return None,
    })
}

/// Runs the `ui-pack` command line with `args` (including the program name).
pub fn main_with(args: Vec<String>) {
    if args.iter().any(|a| a == "--help" || a == "-h") || args.len() < 2 {
        eprintln!("{USAGE}");
        return;
    }
    let command = args[1].as_str();
    if let Some(now) = moved(command) {
        fail(format!("`ui-pack {command}` moved to {now}"));
    }
    let mut manifest = PathBuf::from("Cargo.toml");
    let mut out = None;
    let mut only = None;
    let mut binaries = Vec::new();
    let (mut release, mut dev, mut build, mut notarize) = (false, false, true, true);
    let mut app_args = Vec::new();

    let mut i = 2;
    while i < args.len() {
        let value = || {
            args.get(i + 1)
                .cloned()
                .unwrap_or_else(|| fail(format!("{} needs a value", args[i])))
        };
        match args[i].as_str() {
            "--" => {
                app_args = args[i + 1..].to_vec();
                break;
            }
            "--manifest" => manifest = PathBuf::from(value()),
            "--out" => out = Some(PathBuf::from(value())),
            "--only" => {
                only = Some(
                    value()
                        .split(',')
                        .map(|k| k.trim().to_owned())
                        .collect::<Vec<_>>(),
                )
            }
            "--bin" => binaries.push(PathBuf::from(value())),
            flag => {
                match flag {
                    "--release" => release = true,
                    "--dev" => dev = true,
                    "--no-build" => build = false,
                    "--no-notarize" => notarize = false,
                    other => fail(format!("unknown argument `{other}`\n{USAGE}")),
                }
                i += 1;
                continue;
            }
        }
        i += 2;
    }

    let meta = UiMetadata::from_manifest(&manifest).unwrap_or_else(|e| fail(e));
    match command {
        "pack" => {
            let opts = PackOptions {
                manifest,
                only,
                out_dir: out,
                binaries,
                build,
                notarize,
            };
            match pack(&meta, &opts) {
                Ok(packages) => {
                    for p in packages {
                        println!(
                            "{:<10} {:<16} {}  {}",
                            p.kind(),
                            p.arch(),
                            p.sha256_hex(),
                            p.path().display()
                        );
                    }
                }
                Err(e) => fail(e),
            }
        }
        "run" => {
            let opts = RunOptions {
                manifest,
                release,
                dev,
                app_args,
            };
            let code = run(&meta, &opts).unwrap_or_else(|e| fail(e));
            std::process::exit(code);
        }
        "gen-ts" => {
            let out = out.unwrap_or_else(|| meta.manifest_dir.join("ipc.d.ts"));
            match gen_ts(&meta, &manifest, &out) {
                Ok(path) => println!("Wrote {}", path.display()),
                Err(e) => fail(e),
            }
        }
        other => fail(format!("unknown command `{other}`\n{USAGE}")),
    }
}
