//! macOS `.app` and Mac App Store `.pkg` packaging, Info.plist generation, and sandbox entitlements.

use std::fs;
use std::path::{Path, PathBuf};

use crate::metadata::{AppMetadata, BackendTarget};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacChannel {
    Direct,
    AppStore,
}

#[derive(Debug, Clone)]
pub struct MacPackOptions {
    pub binary_path: PathBuf,
    pub output_dir: PathBuf,
    pub channel: MacChannel,
    pub cef_framework: Option<PathBuf>,
    /// Code-signing identity; `"-"` signs ad hoc. `None` signs ad hoc only
    /// when the manifest names `entitlements`.
    pub sign: Option<String>,
    /// `notarytool` keychain profile: notarize and staple after signing.
    /// Needs a Developer ID `sign` identity.
    pub notarize: Option<String>,
}

pub fn render_info_plist(meta: &AppMetadata, executable: &str) -> String {
    let name = xml_escape(&meta.display_name);
    let bid = xml_escape(&meta.bundle_id);
    let ver = xml_escape(&meta.version);
    let exe = xml_escape(executable);
    let cat = xml_escape(&meta.category);
    let min_os = xml_escape(&meta.min_macos);
    let extra = render_plist_extras(meta);

    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key>
  <string>en</string>
  <key>CFBundleDisplayName</key>
  <string>{name}</string>
  <key>CFBundleExecutable</key>
  <string>{exe}</string>
  <key>CFBundleIdentifier</key>
  <string>{bid}</string>
  <key>CFBundleInfoDictionaryVersion</key>
  <string>6.0</string>
  <key>CFBundleName</key>
  <string>{name}</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>{ver}</string>
  <key>CFBundleVersion</key>
  <string>{ver}</string>
  <key>LSApplicationCategoryType</key>
  <string>{cat}</string>
  <key>LSMinimumSystemVersion</key>
  <string>{min_os}</string>
  <key>NSHighResolutionCapable</key>
  <true/>
{extra}</dict>
</plist>
"#
    )
}

/// `CFBundleURLTypes`, `CFBundleDocumentTypes` and `CFBundleIconFile`, from metadata.
fn render_plist_extras(meta: &AppMetadata) -> String {
    let mut out = String::new();
    if !meta.url_schemes.is_empty() {
        out.push_str("  <key>CFBundleURLTypes</key>\n  <array>\n    <dict>\n");
        out.push_str(&format!(
            "      <key>CFBundleURLName</key>\n      <string>{}</string>\n",
            xml_escape(&meta.bundle_id)
        ));
        out.push_str("      <key>CFBundleURLSchemes</key>\n      <array>\n");
        for scheme in &meta.url_schemes {
            out.push_str(&format!(
                "        <string>{}</string>\n",
                xml_escape(scheme)
            ));
        }
        out.push_str("      </array>\n    </dict>\n  </array>\n");
    }
    if !meta.document_types.is_empty() {
        out.push_str("  <key>CFBundleDocumentTypes</key>\n  <array>\n");
        for ext in &meta.document_types {
            let ext = xml_escape(ext.trim_start_matches('.'));
            out.push_str(&format!(
                "    <dict>\n      <key>CFBundleTypeName</key>\n      <string>{ext} document</string>\n      \
                 <key>CFBundleTypeRole</key>\n      <string>Editor</string>\n      \
                 <key>LSHandlerRank</key>\n      <string>Alternate</string>\n      \
                 <key>CFBundleTypeExtensions</key>\n      <array>\n        <string>{ext}</string>\n      </array>\n    </dict>\n"
            ));
        }
        out.push_str("  </array>\n");
    }
    if let Some(icon) = meta.icon.as_deref().filter(|i| i.ends_with(".icns")) {
        let file = Path::new(icon)
            .file_name()
            .and_then(|f| f.to_str())
            .unwrap_or(icon);
        out.push_str(&format!(
            "  <key>CFBundleIconFile</key>\n  <string>{}</string>\n",
            xml_escape(file)
        ));
    }
    out
}

/// Signs `path` with `identity` (`"-"` for ad hoc), with `entitlements` if given.
pub fn codesign(path: &Path, identity: &str, entitlements: Option<&Path>) -> Result<(), String> {
    let mut cmd = std::process::Command::new("codesign");
    cmd.args(["--force", "--sign", identity]);
    if let Some(e) = entitlements {
        cmd.arg("--entitlements").arg(e);
    }
    if identity != "-" {
        cmd.args(["--options", "runtime", "--timestamp"]);
    }
    let status = cmd
        .arg(path)
        .status()
        .map_err(|e| format!("spawn codesign: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("codesign failed for {}", path.display()))
    }
}

/// The entitlements to sign with: the manifest's `entitlements` file, or the
/// inline `[package.metadata.ui.entitlements]` table written to a temp file.
pub fn entitlements_path(meta: &AppMetadata) -> Option<PathBuf> {
    if let Some(e) = &meta.entitlements {
        return Some(meta.manifest_dir.join(e));
    }
    if meta.entitlement_keys.is_empty() {
        return None;
    }
    let path = std::env::temp_dir().join(format!(
        "ui-pack-{}-{}.entitlements",
        meta.crate_name,
        std::process::id()
    ));
    fs::write(&path, render_entitlements(&meta.entitlement_keys)).ok()?;
    Some(path)
}

/// An entitlements plist from `(key, enabled)` pairs.
pub fn render_entitlements(keys: &[(String, bool)]) -> String {
    let mut body = String::new();
    for (k, on) in keys {
        body.push_str(&format!(
            "  <key>{}</key>\n  <{}/>\n",
            xml_escape(k),
            if *on { "true" } else { "false" }
        ));
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n{body}</dict>\n</plist>\n"
    )
}

/// The commands that notarize and staple `app` (zip, submit and wait, staple).
pub fn notarize_commands(app: &Path, profile: &str) -> Vec<Vec<String>> {
    let zip = app.with_extension("zip");
    let s = |p: &Path| p.to_string_lossy().into_owned();
    vec![
        vec![
            "ditto".into(),
            "-c".into(),
            "-k".into(),
            "--keepParent".into(),
            s(app),
            s(&zip),
        ],
        vec![
            "xcrun".into(),
            "notarytool".into(),
            "submit".into(),
            s(&zip),
            "--keychain-profile".into(),
            profile.into(),
            "--wait".into(),
        ],
        vec!["xcrun".into(), "stapler".into(), "staple".into(), s(app)],
    ]
}

fn notarize(app: &Path, profile: &str) -> Result<(), String> {
    for cmd in notarize_commands(app, profile) {
        let status = std::process::Command::new(&cmd[0])
            .args(&cmd[1..])
            .status()
            .map_err(|e| format!("run {}: {e}", cmd[0]))?;
        if !status.success() {
            return Err(format!("`{}` failed", cmd.join(" ")));
        }
    }
    let _ = fs::remove_file(app.with_extension("zip"));
    Ok(())
}

pub fn render_sandbox_entitlements() -> &'static str {
    r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>com.apple.security.app-sandbox</key>
  <true/>
  <key>com.apple.security.network.client</key>
  <true/>
  <key>com.apple.security.files.user-selected.read-write</key>
  <true/>
</dict>
</plist>
"#
}

pub fn pack_macos_app(meta: &AppMetadata, opts: &MacPackOptions) -> Result<PathBuf, String> {
    if opts.channel == MacChannel::AppStore && meta.backend == BackendTarget::Cef {
        return Err("Mac App Store builds cannot bundle CEF; use the default `webkit` backend for MAS channel".into());
    }

    let app_dir = opts.output_dir.join(format!("{}.app", meta.display_name));
    let contents = app_dir.join("Contents");
    let macos_dir = contents.join("MacOS");
    let res_dir = contents.join("Resources");
    let fw_dir = contents.join("Frameworks");

    fs::create_dir_all(&macos_dir).map_err(|e| e.to_string())?;
    fs::create_dir_all(&res_dir).map_err(|e| e.to_string())?;

    let exe_name = opts
        .binary_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(&meta.crate_name);
    let dst_bin = macos_dir.join(exe_name);
    fs::copy(&opts.binary_path, &dst_bin).map_err(|e| format!("copy binary: {e}"))?;

    fs::write(
        contents.join("Info.plist"),
        render_info_plist(meta, exe_name),
    )
    .map_err(|e| format!("write Info.plist: {e}"))?;

    if let Some(icon) = meta.icon.as_deref().filter(|i| i.ends_with(".icns")) {
        let src = meta.manifest_dir.join(icon);
        let name = Path::new(icon).file_name().unwrap_or_default();
        fs::copy(&src, res_dir.join(name))
            .map_err(|e| format!("copy icon {}: {e}", src.display()))?;
    }

    if opts.channel == MacChannel::AppStore {
        let mas = match entitlements_path(meta) {
            Some(p) => fs::read_to_string(&p)
                .map_err(|e| format!("read entitlements {}: {e}", p.display()))?,
            None => render_sandbox_entitlements().to_owned(),
        };
        fs::write(opts.output_dir.join("entitlements.mas.plist"), mas)
            .map_err(|e| format!("write entitlements: {e}"))?;
    } else if meta.backend == BackendTarget::Cef
        && let Some(ref cef_fw) = resolve_cef_framework(opts)
    {
        fs::create_dir_all(&fw_dir).map_err(|e| e.to_string())?;
        let dst_fw = fw_dir.join("Chromium Embedded Framework.framework");
        copy_bundle_preserving_symlinks(cef_fw, &dst_fw)?;

        if let Some(parent) = cef_fw.parent() {
            for helper_name in [
                "ui Helper.app",
                "ui Helper (Alerts).app",
                "ui Helper (GPU).app",
                "ui Helper (Plugin).app",
                "ui Helper (Renderer).app",
            ] {
                let src_helper = parent.join(helper_name);
                if src_helper.exists() {
                    let dst_helper = fw_dir.join(helper_name);
                    copy_bundle_preserving_symlinks(&src_helper, &dst_helper)?;
                    let plist_path = dst_helper.join("Contents/Info.plist");
                    if let Ok(plist) = fs::read_to_string(&plist_path) {
                        let rewritten = plist.replace(
                            "<key>CFBundleIdentifier</key><string>org.ui.cef</string>",
                            &format!(
                                "<key>CFBundleIdentifier</key><string>{}</string>",
                                xml_escape(&meta.bundle_id)
                            ),
                        );
                        fs::write(&plist_path, rewritten)
                            .map_err(|e| format!("update helper Info.plist: {e}"))?;
                    }
                }
            }
        }
    }

    if opts.channel == MacChannel::Direct {
        let entitlements = entitlements_path(meta);
        let identity = opts
            .sign
            .clone()
            .or_else(|| entitlements.as_ref().map(|_| "-".to_owned()));
        if let Some(identity) = &identity {
            codesign(&app_dir, identity, entitlements.as_deref())?;
        }
        if let Some(profile) = &opts.notarize {
            match identity.as_deref() {
                Some(id) if id != "-" => notarize(&app_dir, profile)?,
                _ => return Err("--notarize needs a Developer ID identity in --sign".into()),
            }
        }
    }

    Ok(app_dir)
}

fn resolve_cef_framework(opts: &MacPackOptions) -> Option<PathBuf> {
    if let Some(ref explicit) = opts.cef_framework
        && explicit.exists()
    {
        return Some(explicit.clone());
    }
    for anc in opts.binary_path.ancestors().skip(1).take(3) {
        let candidate = anc.join("Chromium Embedded Framework.framework");
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

fn copy_bundle_preserving_symlinks(src: &Path, dst: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let status = std::process::Command::new("ditto")
            .arg(src)
            .arg(dst)
            .status()
            .map_err(|e| format!("spawn ditto: {e}"))?;
        if !status.success() {
            return Err(format!(
                "ditto failed copying {} to {}",
                src.display(),
                dst.display()
            ));
        }
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        copy_dir_recursive(src, dst)
    }
}

#[cfg(not(target_os = "macos"))]
fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    fs::create_dir_all(dst).map_err(|e| e.to_string())?;
    for entry in fs::read_dir(src).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let ty = entry.file_type().map_err(|e| e.to_string())?;
        let dst_path = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_recursive(&entry.path(), &dst_path)?;
        } else {
            fs::copy(entry.path(), &dst_path).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
