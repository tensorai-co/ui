//! Linux XDG `.desktop` / AppDir / `.deb` control metadata generators.

use crate::metadata::AppMetadata;

pub fn render_desktop_entry(meta: &AppMetadata, executable: &str) -> String {
    let name = &meta.display_name;
    let icon = meta.icon.as_deref().unwrap_or(&meta.crate_name);
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name={name}\n\
         Exec={executable} %U\n\
         Icon={icon}\n\
         Terminal=false\n\
         Categories=Utility;\n"
    )
}
