//! The `ui-pack` / `cargo ui` command line.

use std::path::PathBuf;

use crate::linux::render_desktop_entry;
use crate::macos::{MacChannel, MacPackOptions, pack_macos_app};
use crate::metadata::AppMetadata;
use crate::run::{RunOptions, gen_ts, make_icons, run};
use crate::windows::render_appx_manifest;

const USAGE: &str = "\
Usage:
  ui-pack macos [--manifest Cargo.toml] [--bin path] [--out dir] [--sign identity] [--notarize keychain-profile]
  ui-pack mac-app-store [--manifest Cargo.toml] [--bin path] [--out dir]   (alias: mas)
  ui-pack msix | linux [--manifest Cargo.toml]
  ui-pack run [--manifest Cargo.toml] [--release] [--dev] [-- app args…]
  ui-pack icons <icon.png> [--out dir]
  ui-pack gen-ts [--manifest Cargo.toml] [--out ipc.d.ts]
  ui-pack update-key [--out dir]
  ui-pack update-feed <private-key.pem> <version> <url> [notes] > feed.json

`cargo ui <command>` runs the same commands (`cargo install` puts `cargo-ui` on PATH).";

fn fail(msg: impl std::fmt::Display) -> ! {
    eprintln!("error: {msg}");
    std::process::exit(1);
}

/// Runs the `ui-pack` command line with `args` (including the program name).
pub fn main_with(args: Vec<String>) {
    if args.iter().any(|a| a == "--help" || a == "-h") || args.len() < 2 {
        eprintln!("{USAGE}");
        return;
    }
    let target = args[1].as_str();
    let mut manifest = PathBuf::from("Cargo.toml");
    let mut bin_path = None;
    let mut out_dir = None;
    let mut sign = None;
    let mut notarize = None;
    let mut release = false;
    let mut dev = false;
    let mut positional = Vec::new();
    let mut app_args = Vec::new();

    let mut i = 2;
    while i < args.len() {
        let next = args.get(i + 1).cloned();
        match (args[i].as_str(), next) {
            ("--", _) => {
                app_args = args[i + 1..].to_vec();
                break;
            }
            ("--manifest", Some(v)) => {
                manifest = PathBuf::from(v);
                i += 2;
            }
            ("--bin", Some(v)) => {
                bin_path = Some(PathBuf::from(v));
                i += 2;
            }
            ("--out", Some(v)) => {
                out_dir = Some(PathBuf::from(v));
                i += 2;
            }
            ("--sign", Some(v)) => {
                sign = Some(v);
                i += 2;
            }
            ("--notarize", Some(v)) => {
                notarize = Some(v);
                i += 2;
            }
            ("--release", _) => {
                release = true;
                i += 1;
            }
            ("--dev", _) => {
                dev = true;
                i += 1;
            }
            (other, _) => {
                positional.push(other.to_owned());
                i += 1;
            }
        }
    }

    if target == "icons" {
        let Some(src) = positional.first() else {
            fail("ui-pack icons needs a source PNG");
        };
        let out = out_dir.unwrap_or_else(|| {
            PathBuf::from(src)
                .parent()
                .map_or_else(|| PathBuf::from("."), PathBuf::from)
        });
        match make_icons(&PathBuf::from(src), &out) {
            Ok(icns) => println!("Created {}", icns.display()),
            Err(e) => fail(e),
        }
        return;
    }

    if matches!(target, "update-key" | "update-feed") {
        match target {
            "update-key" => {
                let dir = out_dir.clone().unwrap_or_else(|| PathBuf::from("."));
                match crate::update::make_key(&dir) {
                    Ok((key, public)) => {
                        println!(
                            "Private key: {} (keep it secret; don't commit it)",
                            key.display()
                        );
                        println!("Public key for ui::update::check:\n{public}");
                    }
                    Err(e) => fail(e),
                }
            }
            "update-feed" => {
                let [key, version, url] = [0, 1, 2].map(|i| positional.get(i).cloned());
                let (Some(key), Some(version), Some(url)) = (key, version, url) else {
                    fail("usage: ui-pack update-feed <private-key.pem> <version> <url> [notes]");
                };
                match crate::update::make_feed(
                    &PathBuf::from(key),
                    &version,
                    &url,
                    positional.get(3).map(String::as_str),
                ) {
                    Ok(feed) => print!("{feed}"),
                    Err(e) => fail(e),
                }
            }
            _ => unreachable!(),
        }
        return;
    }

    let meta = AppMetadata::from_manifest(&manifest).unwrap_or_else(|e| fail(e));
    let gen_out = out_dir.clone();
    let out_dir = out_dir.unwrap_or_else(|| PathBuf::from("target/pack"));

    match target {
        "macos" | "mas" | "mac-app-store" => {
            let bin =
                bin_path.unwrap_or_else(|| PathBuf::from("target/release").join(&meta.crate_name));
            let channel = if target == "macos" {
                MacChannel::Direct
            } else {
                MacChannel::AppStore
            };
            match pack_macos_app(
                &meta,
                &MacPackOptions {
                    binary_path: bin,
                    output_dir: out_dir,
                    channel,
                    cef_framework: None,
                    sign,
                    notarize,
                },
            ) {
                Ok(app) => println!("Created bundle: {}", app.display()),
                Err(e) => fail(e),
            }
        }
        "run" => {
            let code = run(
                &meta,
                &RunOptions {
                    manifest,
                    release,
                    dev,
                    app_args,
                },
            )
            .unwrap_or_else(|e| fail(e));
            std::process::exit(code);
        }
        "gen-ts" => {
            let out = gen_out.unwrap_or_else(|| meta.manifest_dir.join("ipc.d.ts"));
            match gen_ts(&meta, &manifest, &out) {
                Ok(path) => println!("Wrote {}", path.display()),
                Err(e) => fail(e),
            }
        }
        "msix" => {
            println!(
                "{}",
                render_appx_manifest(&meta, &format!("{}.exe", meta.crate_name), "CN=Publisher")
            );
        }
        "linux" => {
            println!("{}", render_desktop_entry(&meta, &meta.crate_name));
        }
        other => fail(format!("unknown target `{other}`\n{USAGE}")),
    }
}
