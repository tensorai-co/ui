//! Release bundler and signer (`ui-pack`) for `.app`, Mac App Store `.pkg`, Windows MSIX, and Linux bundles.

pub mod cli;
pub mod linux;
pub mod macos;
pub mod metadata;
pub mod run;
pub mod update;
pub mod windows;

#[cfg(test)]
mod tests {
    use super::linux::render_desktop_entry;
    use super::macos::{
        MacChannel, MacPackOptions, pack_macos_app, render_info_plist, render_sandbox_entitlements,
    };
    use super::metadata::{AppMetadata, BackendTarget};
    use super::windows::render_appx_manifest;

    #[test]
    fn url_schemes_documents_and_entitlements() {
        let meta = AppMetadata::parse(
            "[package]\nname = \"notes\"\n[package.metadata.ui]\nbundle_id = \"com.example.notes\"\n\
             url_schemes = [\"notes\", \"notes-dev\"]\ndocument_types = [\"md\", \".txt\"]\n\
             entitlements = \"app.entitlements\"\nicon = \"assets/AppIcon.icns\"\n",
        )
        .unwrap();
        assert_eq!(meta.url_schemes, ["notes", "notes-dev"]);
        assert_eq!(meta.document_types, ["md", ".txt"]);
        assert_eq!(meta.entitlements.as_deref(), Some("app.entitlements"));

        let plist = render_info_plist(&meta, "notes");
        assert!(plist.contains("<key>CFBundleURLSchemes</key>"));
        assert!(plist.contains("<string>notes-dev</string>"));
        assert!(plist.contains("<key>CFBundleTypeExtensions</key>"));
        assert!(
            plist.contains("<string>txt</string>"),
            "leading dot stripped"
        );
        assert!(plist.contains("<key>CFBundleIconFile</key>\n  <string>AppIcon.icns</string>"));
        let speechy = AppMetadata::parse(
            "[package]\nname = \"d\"\n[package.metadata.ui]\nmicrophone_usage = \"Dictation\"\nspeech_usage = \"Turns speech into text\"\n",
        )
        .unwrap();
        let p2 = render_info_plist(&speechy, "d");
        assert!(
            p2.contains("<key>NSMicrophoneUsageDescription</key>\n  <string>Dictation</string>")
        );
        assert!(p2.contains("<key>NSSpeechRecognitionUsageDescription</key>"));

        let appx = render_appx_manifest(&meta, "notes.exe", "CN=Example");
        assert!(appx.contains(r#"<uap:Protocol Name="notes" />"#));
        assert!(appx.contains("<uap:FileType>.md</uap:FileType>"));
        assert!(
            render_desktop_entry(&meta, "notes")
                .contains("MimeType=x-scheme-handler/notes;x-scheme-handler/notes-dev;")
        );
    }

    #[test]
    fn inline_entitlements_and_notarize_commands() {
        let meta = AppMetadata::parse(
            "[package]\nname = \"vm\"\n[package.metadata.ui.entitlements]\n\
             \"com.apple.security.hypervisor\" = true\n\"com.apple.security.cs.allow-jit\" = false\n",
        )
        .unwrap();
        assert_eq!(
            meta.entitlement_keys,
            [
                ("com.apple.security.hypervisor".to_owned(), true),
                ("com.apple.security.cs.allow-jit".to_owned(), false)
            ]
        );
        let path = super::macos::entitlements_path(&meta).expect("inline entitlements written");
        let plist = std::fs::read_to_string(&path).unwrap();
        assert!(plist.contains("<key>com.apple.security.hypervisor</key>\n  <true/>"));
        assert!(plist.contains("<key>com.apple.security.cs.allow-jit</key>\n  <false/>"));
        let _ = std::fs::remove_file(path);

        let cmds = super::macos::notarize_commands(std::path::Path::new("/o/My App.app"), "prof");
        assert_eq!(cmds[0][..4], ["ditto", "-c", "-k", "--keepParent"]);
        assert_eq!(cmds[1][..3], ["xcrun", "notarytool", "submit"]);
        assert!(cmds[1].ends_with(&["--keychain-profile".into(), "prof".into(), "--wait".into()]));
        assert_eq!(cmds[2], ["xcrun", "stapler", "staple", "/o/My App.app"]);
    }

    #[test]
    fn parses_package_metadata_ui() {
        let toml = r#"
[package]
name = "my-app"
version = "0.4.2"

[package.metadata.ui]
name = "My App"
bundle_id = "com.example.myapp"
backend = "webkit"
"#;
        let meta = AppMetadata::parse(toml).unwrap();
        assert_eq!(meta.crate_name, "my-app");
        assert_eq!(meta.display_name, "My App");
        assert_eq!(meta.version, "0.4.2");
        assert_eq!(meta.bundle_id, "com.example.myapp");
        assert_eq!(meta.backend, BackendTarget::WebKit);
    }

    #[test]
    fn generates_platform_manifests_and_enforces_mas_rule() {
        let meta = AppMetadata::parse(
            "[package]\nname = \"demo\"\n[package.metadata.ui]\nbackend = \"cef\"\nbundle_id = \"com.example.demo\"\n",
        )
        .unwrap();
        let plist = render_info_plist(&meta, "demo");
        assert!(plist.contains("<string>com.example.demo</string>"));
        assert!(render_sandbox_entitlements().contains("com.apple.security.app-sandbox"));
        assert!(render_appx_manifest(&meta, "demo.exe", "CN=Example").contains("0.1.0.0"));
        assert!(render_desktop_entry(&meta, "demo").contains("Exec=demo %U"));

        let err = pack_macos_app(
            &meta,
            &MacPackOptions {
                binary_path: std::path::PathBuf::from("nonexistent"),
                output_dir: std::env::temp_dir(),
                channel: MacChannel::AppStore,
                cef_framework: None,
                sign: None,
                notarize: None,
            },
        )
        .unwrap_err();
        assert!(err.contains("Mac App Store"));
    }

    #[test]
    fn packages_direct_macos_cef_bundle_with_preserved_symlinks() {
        let root = std::env::temp_dir().join(format!("ui-pack-cef-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let stage = root.join("stage");
        let bin = stage.join("demo");
        let fw = stage.join("Chromium Embedded Framework.framework");
        let helper_contents = stage.join("ui Helper.app/Contents");
        let helper_macos = helper_contents.join("MacOS");
        std::fs::create_dir_all(fw.join("Versions/A")).unwrap();
        std::fs::write(fw.join("Versions/A/Chromium Embedded Framework"), b"libcef").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(
            "Versions/A/Chromium Embedded Framework",
            fw.join("Chromium Embedded Framework"),
        )
        .unwrap();
        std::fs::create_dir_all(&helper_macos).unwrap();
        std::fs::write(helper_macos.join("ui Helper"), b"helper").unwrap();
        std::fs::write(
            helper_contents.join("Info.plist"),
            "<dict><key>CFBundleIdentifier</key><string>org.ui.cef</string></dict>",
        )
        .unwrap();
        std::fs::write(&bin, b"demo-bin").unwrap();

        let meta = AppMetadata::parse(
            "[package]\nname = \"demo\"\n[package.metadata.ui]\nbackend = \"cef\"\nbundle_id = \"com.example.demo\"\n",
        )
        .unwrap();

        let out_dir = root.join("out");
        let app = pack_macos_app(
            &meta,
            &MacPackOptions {
                binary_path: bin,
                output_dir: out_dir,
                channel: MacChannel::Direct,
                cef_framework: None,
                sign: None,
                notarize: None,
            },
        )
        .unwrap();

        let bundled_helper_plist = std::fs::read_to_string(
            app.join("Contents/Frameworks/ui Helper.app/Contents/Info.plist"),
        )
        .unwrap();
        assert!(bundled_helper_plist.contains("<string>com.example.demo</string>"));
        #[cfg(unix)]
        {
            let link_meta =
                std::fs::symlink_metadata(app.join("Contents/Frameworks/Chromium Embedded Framework.framework/Chromium Embedded Framework"))
                    .unwrap();
            assert!(link_meta.file_type().is_symlink());
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
