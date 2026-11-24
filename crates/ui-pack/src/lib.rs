//! `ui-pack`: packaging and the dev loop for `ui` apps.
//!
//! Packaging is [packager](https://github.com/tensorai-co/packager)'s: `ui-pack pack` reads
//! `[package.metadata.packager]` from the app's `Cargo.toml` and builds `.app`, `.dmg`, `.pkg`,
//! `.msi`, `.msix`, `.deb` and `.snap` packages through it, adding what a `ui` app needs on top
//! (the Chromium Embedded Framework and its helper apps for `backend = "cef"`). `run` and
//! `gen-ts` are the dev loop.

pub mod cli;
pub mod metadata;
pub mod pack;
pub mod run;

#[cfg(test)]
mod tests {
    use super::metadata::{BackendTarget, UiMetadata};

    #[test]
    fn reads_ui_metadata() {
        let meta = UiMetadata::parse(
            "[package]\nname = \"my-app\"\nversion = \"0.4.2\"\n\n[package.metadata.ui]\n\
             backend = \"cef\"\ndev_command = \"npm run dev\"\ndev_url = \"http://localhost:3000\"\n",
        )
        .unwrap();
        assert_eq!(meta.crate_name, "my-app");
        assert_eq!(meta.backend, BackendTarget::Cef);
        assert_eq!(meta.dev_command.as_deref(), Some("npm run dev"));
        assert_eq!(meta.dev_url.as_deref(), Some("http://localhost:3000"));
        let plain = UiMetadata::parse("[package]\nname = \"x\"\n").unwrap();
        assert_eq!(plain.backend, BackendTarget::Auto);
    }

    #[test]
    fn points_moved_packaging_keys_at_packager() {
        let err = UiMetadata::parse(
            "[package]\nname = \"x\"\n[package.metadata.ui]\nbundle_id = \"com.example.x\"\n",
        )
        .unwrap_err();
        assert!(
            err.contains("[package.metadata.packager.app] identifier"),
            "{err}"
        );
        assert!(
            UiMetadata::parse("[package]\nname = \"x\"\n[package.metadata.ui]\nbackend = \"qt\"\n")
                .is_err()
        );
    }

    /// A CEF app's `.app` carries the framework and the helpers next to the binary, the helpers
    /// taking the app's identifier, all signed by packager.
    #[cfg(target_os = "macos")]
    #[test]
    fn packs_cef_framework_and_helpers() {
        use std::path::Path;
        let root = std::env::temp_dir().join(format!("ui-pack-cef-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let release = root.join("target/release");
        // Any Mach-O executable will do as the app, framework and helper binaries.
        let exe = std::env::current_exe().unwrap();
        let bundle = |dir: &Path, exe_name: &str, id: &str, kind: &str| {
            let contents = if kind == "FMWK" {
                dir.join("Versions/A/Resources")
            } else {
                dir.join("Contents")
            };
            std::fs::create_dir_all(&contents).unwrap();
            let bin_dir = if kind == "FMWK" {
                dir.join("Versions/A")
            } else {
                dir.join("Contents/MacOS")
            };
            std::fs::create_dir_all(&bin_dir).unwrap();
            std::fs::copy(&exe, bin_dir.join(exe_name)).unwrap();
            std::fs::write(
                contents.join("Info.plist"),
                format!(
                    "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\"><dict>\
                     <key>CFBundleExecutable</key><string>{exe_name}</string>\
                     <key>CFBundleIdentifier</key><string>{id}</string>\
                     <key>CFBundlePackageType</key><string>{kind}</string></dict></plist>\n"
                ),
            )
            .unwrap();
        };
        let fw = release.join("Chromium Embedded Framework.framework");
        bundle(
            &fw,
            "Chromium Embedded Framework",
            "org.cef.framework",
            "FMWK",
        );
        std::os::unix::fs::symlink("A", fw.join("Versions/Current")).unwrap();
        std::os::unix::fs::symlink(
            "Versions/Current/Chromium Embedded Framework",
            fw.join("Chromium Embedded Framework"),
        )
        .unwrap();
        std::os::unix::fs::symlink("Versions/Current/Resources", fw.join("Resources")).unwrap();
        bundle(
            &release.join("ui Helper (GPU).app"),
            "ui Helper (GPU)",
            "org.ui.cef",
            "APPL",
        );
        std::fs::copy(&exe, release.join("demo")).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"demo\"\nversion = \"1.0.0\"\n\n[package.metadata.ui]\nbackend = \"cef\"\n\n\
             [package.metadata.packager.app]\nidentifier = \"com.example.demo\"\n\n[package.metadata.packager.package.app]\n",
        )
        .unwrap();

        let manifest = root.join("Cargo.toml");
        let meta = UiMetadata::from_manifest(&manifest).unwrap();
        let packages = super::pack::pack(
            &meta,
            &super::pack::PackOptions {
                manifest,
                only: Some(vec!["app".into()]),
                out_dir: Some(root.join("out")),
                binaries: vec![release.join("demo")],
                build: false,
                notarize: false,
            },
        )
        .unwrap();
        let app = packages[0].path();
        let helper_plist = std::fs::read_to_string(
            app.join("Contents/Frameworks/ui Helper (GPU).app/Contents/Info.plist"),
        )
        .unwrap();
        assert!(
            helper_plist.contains("<string>com.example.demo</string>"),
            "{helper_plist}"
        );
        let link = app.join(
            "Contents/Frameworks/Chromium Embedded Framework.framework/Chromium Embedded Framework",
        );
        assert!(
            std::fs::symlink_metadata(link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        packager::sign::bundle::verify_bundle(app).unwrap();
        let _ = std::fs::remove_dir_all(&root);
    }
}
