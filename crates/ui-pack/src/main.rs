//! `ui-pack` CLI for packaging and running `ui` desktop applications.

use std::path::PathBuf;

use ui_pack::linux::render_desktop_entry;
use ui_pack::macos::{MacChannel, MacPackOptions, pack_macos_app};
use ui_pack::metadata::AppMetadata;
use ui_pack::run::{RunOptions, make_icons, run};
use ui_pack::windows::render_appx_manifest;

const USAGE: &str = "\
Usage:
  ui-pack macos [--manifest Cargo.toml] [--bin path] [--out dir] [--sign identity]
  ui-pack mac-app-store [--manifest Cargo.toml] [--bin path] [--out dir]   (alias: mas)
  ui-pack msix | linux [--manifest Cargo.toml]
  ui-pack run [--manifest Cargo.toml] [--release] [--dev] [-- app args…]
  ui-pack icons <icon.png> [--out dir]";

fn fail(msg: impl std::fmt::Display) -> ! {
    eprintln!("error: {msg}");
    std::process::exit(1);
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--help" || a == "-h") || args.len() < 2 {
        eprintln!("{USAGE}");
        return;
    }
    let target = args[1].as_str();
    let mut manifest = PathBuf::from("Cargo.toml");
    let mut bin_path = None;
    let mut out_dir = None;
    let mut sign = None;
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
            PathBuf::from(src).parent().map_or_else(|| PathBuf::from("."), PathBuf::from)
        });
        match make_icons(&PathBuf::from(src), &out) {
            Ok(icns) => println!("Created {}", icns.display()),
            Err(e) => fail(e),
        }
        return;
    }

    let meta = AppMetadata::from_manifest(&manifest).unwrap_or_else(|e| fail(e));
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
