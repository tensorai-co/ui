//! Minimal `ui-pack` CLI for packaging `ui` desktop applications.

use std::path::PathBuf;

use ui_pack::linux::render_desktop_entry;
use ui_pack::macos::{MacChannel, MacPackOptions, pack_macos_app};
use ui_pack::metadata::AppMetadata;
use ui_pack::windows::render_appx_manifest;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--help" || a == "-h") || args.len() < 2 {
        eprintln!(
            "Usage: ui-pack <macos|mas|msix|linux> --manifest Cargo.toml [--bin path] [--out dir]"
        );
        return;
    }
    let target = args[1].as_str();
    let mut manifest = PathBuf::from("Cargo.toml");
    let mut bin_path = None;
    let mut out_dir = PathBuf::from("target/pack");

    let mut i = 2;
    while i < args.len() {
        match args[i].as_str() {
            "--manifest" if i + 1 < args.len() => {
                manifest = PathBuf::from(&args[i + 1]);
                i += 2;
            }
            "--bin" if i + 1 < args.len() => {
                bin_path = Some(PathBuf::from(&args[i + 1]));
                i += 2;
            }
            "--out" if i + 1 < args.len() => {
                out_dir = PathBuf::from(&args[i + 1]);
                i += 2;
            }
            _ => i += 1,
        }
    }

    let meta = AppMetadata::from_manifest(&manifest).unwrap_or_else(|e| {
        eprintln!("error: {e}");
        std::process::exit(1);
    });

    match target {
        "macos" | "mas" => {
            let bin =
                bin_path.unwrap_or_else(|| PathBuf::from("target/release").join(&meta.crate_name));
            let ch = if target == "mas" {
                MacChannel::AppStore
            } else {
                MacChannel::Direct
            };
            match pack_macos_app(
                &meta,
                &MacPackOptions {
                    binary_path: bin,
                    output_dir: out_dir,
                    channel: ch,
                    cef_framework: None,
                },
            ) {
                Ok(app) => println!("Created bundle: {}", app.display()),
                Err(e) => {
                    eprintln!("error: {e}");
                    std::process::exit(1);
                }
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
        other => {
            eprintln!("unknown target `{other}`");
            std::process::exit(1);
        }
    }
}
