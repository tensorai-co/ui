//! Windows MSIX manifest generation and direct bundle staging.

use crate::metadata::AppMetadata;

pub fn render_appx_manifest(meta: &AppMetadata, executable: &str, publisher: &str) -> String {
    let name = &meta.display_name;
    let id = &meta.bundle_id;
    let ver = normalize_msix_version(&meta.version);
    let mut ext = String::new();
    for scheme in &meta.url_schemes {
        ext.push_str(&format!(
            "\n        <uap:Extension Category=\"windows.protocol\">\n          <uap:Protocol Name=\"{scheme}\" />\n        </uap:Extension>"
        ));
    }
    if !meta.document_types.is_empty() {
        ext.push_str("\n        <uap:Extension Category=\"windows.fileTypeAssociation\">\n          <uap:FileTypeAssociation Name=\"documents\">\n            <uap:SupportedFileTypes>");
        for t in &meta.document_types {
            ext.push_str(&format!("\n              <uap:FileType>.{}</uap:FileType>", t.trim_start_matches('.')));
        }
        ext.push_str("\n            </uap:SupportedFileTypes>\n          </uap:FileTypeAssociation>\n        </uap:Extension>");
    }
    let extensions = if ext.is_empty() {
        String::new()
    } else {
        format!("\n      <Extensions>{ext}\n      </Extensions>")
    };
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<Package
  xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10"
  xmlns:uap="http://schemas.microsoft.com/appx/manifest/uap/windows10"
  xmlns:rescap="http://schemas.microsoft.com/appx/manifest/foundation/windows10/restrictedcapabilities"
  IgnorableNamespaces="uap rescap">
  <Identity Name="{id}" Publisher="{publisher}" Version="{ver}" />
  <Properties>
    <DisplayName>{name}</DisplayName>
    <PublisherDisplayName>{name}</PublisherDisplayName>
    <Logo>Assets\StoreLogo.png</Logo>
  </Properties>
  <Dependencies>
    <TargetDeviceFamily Name="Windows.Desktop" MinVersion="10.0.19041.0" MaxVersionTested="10.0.26100.0" />
  </Dependencies>
  <Applications>
    <Application Id="App" Executable="{executable}" EntryPoint="Windows.FullTrustApplication">
      <uap:VisualElements DisplayName="{name}" Description="{name}" BackgroundColor="transparent"
        Square150x150Logo="Assets\Square150x150Logo.png" Square44x44Logo="Assets\Square44x44Logo.png" />{extensions}
    </Application>
  </Applications>
  <Capabilities>
    <rescap:Capability Name="runFullTrust" />
    <Capability Name="internetClient" />
  </Capabilities>
</Package>
"#
    )
}

fn normalize_msix_version(ver: &str) -> String {
    let mut parts: Vec<&str> = ver.split('.').take(4).collect();
    while parts.len() < 4 {
        parts.push("0");
    }
    parts.join(".")
}
