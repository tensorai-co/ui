//! Application menu bar, submenus, standard OS roles, and keyboard shortcuts.

use std::sync::Arc;

/// Modifier keys for menu item shortcuts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Mod(u8);

impl Mod {
    pub const NONE: Self = Self(0);
    /// Primary platform shortcut modifier (`Cmd` on macOS, `Ctrl` on Windows/Linux).
    pub const PRIMARY: Self = Self(1 << 0);
    pub const SHIFT: Self = Self(1 << 1);
    pub const ALT: Self = Self(1 << 2);
    pub const CTRL: Self = Self(1 << 3);
    pub const META: Self = Self(1 << 4);

    pub const fn or(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }
}

impl std::ops::BitOr for Mod {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        self.or(rhs)
    }
}

/// Accelerator shortcut attached to a menu item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shortcut {
    pub mods: Mod,
    pub key: String,
}

impl Shortcut {
    pub fn new(mods: Mod, key: impl Into<String>) -> Self {
        Self {
            mods,
            key: key.into(),
        }
    }

    pub fn cmd(key: impl Into<String>) -> Self {
        Self::new(Mod::PRIMARY, key)
    }

    pub fn cmd_shift(key: impl Into<String>) -> Self {
        Self::new(Mod::PRIMARY | Mod::SHIFT, key)
    }
}

/// Built-in OS responder/action roles wired natively (especially macOS `Edit` & app menu).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    /// "About <App>". The app's name is added on macOS.
    About,
    /// "Settings…" (Cmd+,). Emits `menu:settings` to the focused webview and
    /// [`Event::Menu`](crate::event::Event::Menu)`("settings")` to the app.
    Settings,
    /// The system Services submenu (macOS).
    Services,
    Hide,
    HideOthers,
    ShowAll,
    Quit,
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
    /// Reloads the focused webview (Cmd+R).
    Reload,
    /// History back in the focused webview (Cmd+[).
    Back,
    /// History forward in the focused webview (Cmd+]).
    Forward,
    /// Resets the focused webview's page zoom (Cmd+0).
    ActualSize,
    /// Zooms the focused webview in one step (Cmd+=).
    ZoomIn,
    /// Zooms the focused webview out one step (Cmd+-).
    ZoomOut,
    /// Opens developer tools for the focused webview (Alt+Cmd+I); CEF only.
    ToggleDevTools,
    Minimize,
    /// Zooms the window (maximize/restore); "Zoom" in the macOS Window menu.
    Zoom,
    /// Brings all of the app's windows to the front (macOS).
    BringAllToFront,
    CloseWindow,
    Fullscreen,
    /// "<App> Help": opens the app's Help Book (macOS).
    Help,
}

impl Role {
    /// Roles the runtime carries out on the focused webview, rather than the OS.
    pub(crate) fn is_webview_role(self) -> bool {
        matches!(
            self,
            Self::Settings
                | Self::Reload
                | Self::Back
                | Self::Forward
                | Self::ActualSize
                | Self::ZoomIn
                | Self::ZoomOut
                | Self::ToggleDevTools
        )
    }
}

/// Item inside a submenu.
#[derive(Clone)]
pub enum MenuItem {
    Action {
        id: String,
        title: String,
        shortcut: Option<Shortcut>,
        enabled: bool,
        /// Shows a check mark (macOS `NSControlStateValueOn`).
        checked: bool,
        /// Clicking emits `menu:<id>` to the focused webview and `Event::Menu(id)`.
        emit: bool,
        role: Option<Role>,
        handler: Option<Arc<dyn Fn() + Send + Sync>>,
    },
    Separator,
}

impl std::fmt::Debug for MenuItem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Action {
                id,
                title,
                shortcut,
                enabled,
                checked,
                emit,
                role,
                ..
            } => f
                .debug_struct("Action")
                .field("id", id)
                .field("title", title)
                .field("shortcut", shortcut)
                .field("enabled", enabled)
                .field("checked", checked)
                .field("emit", emit)
                .field("role", role)
                .finish(),
            Self::Separator => write!(f, "Separator"),
        }
    }
}

impl MenuItem {
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Self {
        Self::Action {
            id: id.into(),
            title: title.into(),
            shortcut: None,
            enabled: true,
            checked: false,
            emit: false,
            role: None,
            handler: None,
        }
    }

    pub fn role(role: Role) -> Self {
        let (title, shortcut) = match role {
            Role::About => ("About", None),
            Role::Settings => ("Settings…", Some(Shortcut::cmd(","))),
            Role::Services => ("Services", None),
            Role::Hide => ("Hide", Some(Shortcut::cmd("h"))),
            Role::HideOthers => (
                "Hide Others",
                Some(Shortcut::new(Mod::PRIMARY | Mod::ALT, "h")),
            ),
            Role::ShowAll => ("Show All", None),
            Role::Quit => ("Quit", Some(Shortcut::cmd("q"))),
            Role::Undo => ("Undo", Some(Shortcut::cmd("z"))),
            Role::Redo => ("Redo", Some(Shortcut::cmd_shift("z"))),
            Role::Cut => ("Cut", Some(Shortcut::cmd("x"))),
            Role::Copy => ("Copy", Some(Shortcut::cmd("c"))),
            Role::Paste => ("Paste", Some(Shortcut::cmd("v"))),
            Role::SelectAll => ("Select All", Some(Shortcut::cmd("a"))),
            Role::Reload => ("Reload", Some(Shortcut::cmd("r"))),
            Role::Back => ("Back", Some(Shortcut::cmd("["))),
            Role::Forward => ("Forward", Some(Shortcut::cmd("]"))),
            Role::ActualSize => ("Actual Size", Some(Shortcut::cmd("0"))),
            Role::ZoomIn => ("Zoom In", Some(Shortcut::cmd("="))),
            Role::ZoomOut => ("Zoom Out", Some(Shortcut::cmd("-"))),
            Role::ToggleDevTools => (
                "Developer Tools",
                Some(Shortcut::new(Mod::PRIMARY | Mod::ALT, "i")),
            ),
            Role::Minimize => ("Minimize", Some(Shortcut::cmd("m"))),
            Role::Zoom => ("Zoom", None),
            Role::BringAllToFront => ("Bring All to Front", None),
            Role::Help => ("Help", Some(Shortcut::cmd_shift("/"))),
            Role::CloseWindow => ("Close Window", Some(Shortcut::cmd("w"))),
            Role::Fullscreen => (
                "Toggle Full Screen",
                Some(Shortcut::new(Mod::PRIMARY | Mod::CTRL, "f")),
            ),
        };
        Self::Action {
            id: if role == Role::Settings {
                "settings".to_owned()
            } else {
                format!("role:{role:?}")
            },
            title: title.to_owned(),
            shortcut,
            enabled: true,
            checked: false,
            emit: false,
            role: Some(role),
            handler: None,
        }
    }

    pub fn separator() -> Self {
        Self::Separator
    }

    pub fn shortcut(mut self, shortcut: Shortcut) -> Self {
        if let Self::Action { shortcut: s, .. } = &mut self {
            *s = Some(shortcut);
        }
        self
    }

    pub fn enabled(mut self, is_enabled: bool) -> Self {
        if let Self::Action { enabled, .. } = &mut self {
            *enabled = is_enabled;
        }
        self
    }

    pub fn on_click(mut self, f: impl Fn() + Send + Sync + 'static) -> Self {
        if let Self::Action { handler, .. } = &mut self {
            *handler = Some(Arc::new(f));
        }
        self
    }

    /// Shows a check mark next to the item.
    pub fn checked(mut self, is_checked: bool) -> Self {
        if let Self::Action { checked, .. } = &mut self {
            *checked = is_checked;
        }
        self
    }

    /// Clicking the item (or pressing its shortcut) emits `menu:<id>` to the
    /// focused window's webviews, heard in JS with `ipc.menu.on(id, ..)`, and
    /// [`Event::Menu`](crate::event::Event::Menu)`(id)` to the app.
    ///
    /// ```ignore
    /// MenuItem::new("toggle_panel", "Toggle Panel").shortcut(Shortcut::ctrl("`")).emit()
    /// ```
    pub fn emit(mut self) -> Self {
        if let Self::Action { emit, .. } = &mut self {
            *emit = true;
        }
        self
    }
}

impl Shortcut {
    /// `Ctrl+<key>` on every platform (not Cmd on macOS).
    pub fn ctrl(key: impl Into<String>) -> Self {
        Self::new(Mod::CTRL, key)
    }
}

/// Top-level submenu in an application menu bar.
#[derive(Debug, Clone)]
pub struct Submenu {
    pub title: String,
    pub items: Vec<MenuItem>,
}

impl Submenu {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            items: Vec::new(),
        }
    }

    pub fn item(mut self, item: MenuItem) -> Self {
        self.items.push(item);
        self
    }

    pub fn separator(mut self) -> Self {
        self.items.push(MenuItem::Separator);
        self
    }
}

/// Application menu bar.
#[derive(Debug, Clone)]
pub struct Menu {
    pub submenus: Vec<Submenu>,
}

impl Default for Menu {
    fn default() -> Self {
        Self::standard()
    }
}

impl Menu {
    pub fn new() -> Self {
        Self {
            submenus: Vec::new(),
        }
    }

    pub fn submenu(mut self, submenu: Submenu) -> Self {
        self.submenus.push(submenu);
        self
    }

    /// Standard App, Edit, View, Window and Help menus, so `Cmd+C`/`V`/`X`/`A`/`Z`/`Q`/`W`/`M`
    /// and page zoom (`Cmd+0`/`=`/`-`) work out of the box in `WKWebView` and CEF.
    ///
    /// Add a Settings item with [`with_settings`](Self::with_settings). On macOS the
    /// Help menu gets the system's menu search field.
    pub fn standard() -> Self {
        Self::new()
            .submenu(
                Submenu::new("App")
                    .item(MenuItem::role(Role::About))
                    .separator()
                    .item(MenuItem::role(Role::Services))
                    .separator()
                    .item(MenuItem::role(Role::Hide))
                    .item(MenuItem::role(Role::HideOthers))
                    .item(MenuItem::role(Role::ShowAll))
                    .separator()
                    .item(MenuItem::role(Role::Quit)),
            )
            .submenu(
                Submenu::new("Edit")
                    .item(MenuItem::role(Role::Undo))
                    .item(MenuItem::role(Role::Redo))
                    .separator()
                    .item(MenuItem::role(Role::Cut))
                    .item(MenuItem::role(Role::Copy))
                    .item(MenuItem::role(Role::Paste))
                    .item(MenuItem::role(Role::SelectAll)),
            )
            .submenu(
                Submenu::new("View")
                    .item(MenuItem::role(Role::ActualSize))
                    .item(MenuItem::role(Role::ZoomIn))
                    .item(MenuItem::role(Role::ZoomOut))
                    .separator()
                    .item(MenuItem::role(Role::Fullscreen)),
            )
            .submenu(
                Submenu::new("Window")
                    .item(MenuItem::role(Role::Minimize))
                    .item(MenuItem::role(Role::Zoom))
                    .separator()
                    .item(MenuItem::role(Role::CloseWindow))
                    .separator()
                    .item(MenuItem::role(Role::BringAllToFront)),
            )
            .submenu(Submenu::new("Help"))
    }

    /// Adds "Settings…" (Cmd+,) after "About" in the first (app) menu.
    pub fn with_settings(mut self) -> Self {
        if let Some(app) = self.submenus.first_mut() {
            let at = app
                .items
                .iter()
                .position(|i| matches!(i, MenuItem::Action { role: Some(Role::About), .. }))
                .map_or(0, |p| p + 1);
            app.items.insert(at, MenuItem::Separator);
            app.items.insert(at + 1, MenuItem::role(Role::Settings));
        }
        self
    }

    /// Adds a submenu before the Window menu (or at the end), e.g. an app's
    /// File or Go menu alongside the standard ones.
    pub fn insert_submenu(mut self, submenu: Submenu) -> Self {
        let at = self
            .submenus
            .iter()
            .position(|s| s.title.eq_ignore_ascii_case("Window"))
            .unwrap_or(self.submenus.len());
        self.submenus.insert(at, submenu);
        self
    }
}

/// Changes to apply to a menu item after the menu is built: see
/// [`App::menu_item`](crate::App::menu_item) and `ipc.menu.set(..)` in JS.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MenuItemUpdate {
    pub title: Option<String>,
    pub enabled: Option<bool>,
    pub checked: Option<bool>,
}

/// Handle to one item of the current app menu, by id.
#[derive(Clone)]
pub struct MenuItemHandle {
    pub(crate) id: String,
    pub(crate) runtime: Arc<crate::backend::Runtime>,
}

impl MenuItemHandle {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn set_title(&self, title: impl Into<String>) {
        self.update(MenuItemUpdate {
            title: Some(title.into()),
            ..Default::default()
        });
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.update(MenuItemUpdate {
            enabled: Some(enabled),
            ..Default::default()
        });
    }

    pub fn set_checked(&self, checked: bool) {
        self.update(MenuItemUpdate {
            checked: Some(checked),
            ..Default::default()
        });
    }

    pub fn update(&self, update: MenuItemUpdate) {
        self.runtime.update_menu_item(self.id.clone(), update);
    }
}

impl std::fmt::Debug for MenuItemHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MenuItemHandle").field("id", &self.id).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roles(sub: &Submenu) -> Vec<Option<Role>> {
        sub.items
            .iter()
            .filter_map(|i| match i {
                MenuItem::Action { role, .. } => Some(*role),
                MenuItem::Separator => None,
            })
            .collect()
    }

    #[test]
    fn standard_menu_layout() {
        let m = Menu::standard();
        let titles: Vec<&str> = m.submenus.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(titles, ["App", "Edit", "View", "Window", "Help"]);
        assert!(roles(&m.submenus[0]).contains(&Some(Role::Services)));
        assert!(!roles(&m.submenus[0]).contains(&Some(Role::Settings)));
        assert_eq!(
            roles(&m.submenus[2]),
            [
                Some(Role::ActualSize),
                Some(Role::ZoomIn),
                Some(Role::ZoomOut),
                Some(Role::Fullscreen)
            ]
        );
        assert!(m.submenus[4].items.is_empty());
    }

    #[test]
    fn settings_emit_checked_and_insert() {
        let m = Menu::standard()
            .with_settings()
            .insert_submenu(Submenu::new("Go").item(MenuItem::new("panel", "Panel").emit().checked(true)));
        let app = roles(&m.submenus[0]);
        assert_eq!(&app[..2], &[Some(Role::About), Some(Role::Settings)]);
        let MenuItem::Action { id, shortcut, .. } = &m.submenus[0].items[2] else {
            panic!("expected Settings after a separator");
        };
        assert_eq!(id, "settings");
        assert_eq!(shortcut, &Some(Shortcut::cmd(",")));

        let titles: Vec<&str> = m.submenus.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(titles, ["App", "Edit", "View", "Go", "Window", "Help"]);
        let MenuItem::Action { emit, checked, .. } = &m.submenus[3].items[0] else {
            panic!("expected an action");
        };
        assert!(*emit && *checked);
    }
}
