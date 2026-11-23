//! Application menu bar, submenus, standard OS roles, and keyboard shortcuts.

use std::sync::Arc;

/// Modifier keys for menu item shortcuts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Modifiers(u8);

/// Renamed to [`Modifiers`].
#[deprecated(since = "0.2.0", note = "renamed to `Modifiers`")]
pub type Mod = Modifiers;

impl Modifiers {
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

impl std::ops::BitOr for Modifiers {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        self.or(rhs)
    }
}

/// Accelerator shortcut attached to a menu item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shortcut {
    pub mods: Modifiers,
    pub key: String,
}

impl Shortcut {
    pub fn new(mods: Modifiers, key: impl Into<String>) -> Self {
        Self {
            mods,
            key: key.into(),
        }
    }

    /// The primary modifier: Cmd on macOS, Ctrl on Windows and Linux.
    pub fn primary(key: impl Into<String>) -> Self {
        Self::new(Modifiers::PRIMARY, key)
    }

    /// Primary + Shift.
    pub fn primary_shift(key: impl Into<String>) -> Self {
        Self::new(Modifiers::PRIMARY | Modifiers::SHIFT, key)
    }

    #[deprecated(since = "0.2.0", note = "renamed to `Shortcut::primary` (Cmd or Ctrl)")]
    pub fn cmd(key: impl Into<String>) -> Self {
        Self::primary(key)
    }

    #[deprecated(since = "0.2.0", note = "renamed to `Shortcut::primary_shift`")]
    pub fn cmd_shift(key: impl Into<String>) -> Self {
        Self::primary_shift(key)
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
    /// Maximizes or restores the window; titled "Zoom" in the macOS Window menu.
    Maximize,
    /// Brings all of the app's windows to the front (macOS).
    BringAllToFront,
    CloseWindow,
    Fullscreen,
    /// "<App> Help": opens the app's Help Book (macOS).
    Help,
}

#[allow(non_upper_case_globals)]
impl Role {
    #[deprecated(since = "0.2.0", note = "renamed to `Role::Maximize`")]
    pub const Zoom: Self = Self::Maximize;

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
            Role::Settings => ("Settings…", Some(Shortcut::primary(","))),
            Role::Services => ("Services", None),
            Role::Hide => ("Hide", Some(Shortcut::primary("h"))),
            Role::HideOthers => (
                "Hide Others",
                Some(Shortcut::new(Modifiers::PRIMARY | Modifiers::ALT, "h")),
            ),
            Role::ShowAll => ("Show All", None),
            Role::Quit => ("Quit", Some(Shortcut::primary("q"))),
            Role::Undo => ("Undo", Some(Shortcut::primary("z"))),
            Role::Redo => ("Redo", Some(Shortcut::primary_shift("z"))),
            Role::Cut => ("Cut", Some(Shortcut::primary("x"))),
            Role::Copy => ("Copy", Some(Shortcut::primary("c"))),
            Role::Paste => ("Paste", Some(Shortcut::primary("v"))),
            Role::SelectAll => ("Select All", Some(Shortcut::primary("a"))),
            Role::Reload => ("Reload", Some(Shortcut::primary("r"))),
            Role::Back => ("Back", Some(Shortcut::primary("["))),
            Role::Forward => ("Forward", Some(Shortcut::primary("]"))),
            Role::ActualSize => ("Actual Size", Some(Shortcut::primary("0"))),
            Role::ZoomIn => ("Zoom In", Some(Shortcut::primary("="))),
            Role::ZoomOut => ("Zoom Out", Some(Shortcut::primary("-"))),
            Role::ToggleDevTools => (
                "Developer Tools",
                Some(Shortcut::new(Modifiers::PRIMARY | Modifiers::ALT, "i")),
            ),
            Role::Minimize => ("Minimize", Some(Shortcut::primary("m"))),
            Role::Maximize => ("Zoom", None),
            Role::BringAllToFront => ("Bring All to Front", None),
            Role::Help => ("Help", Some(Shortcut::primary_shift("/"))),
            Role::CloseWindow => ("Close Window", Some(Shortcut::primary("w"))),
            Role::Fullscreen => (
                "Toggle Full Screen",
                Some(Shortcut::new(Modifiers::PRIMARY | Modifiers::CTRL, "f")),
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
        Self::new(Modifiers::CTRL, key)
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
                    .item(MenuItem::role(Role::Maximize))
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
                .position(|i| {
                    matches!(
                        i,
                        MenuItem::Action {
                            role: Some(Role::About),
                            ..
                        }
                    )
                })
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
        f.debug_struct("MenuItemHandle")
            .field("id", &self.id)
            .finish()
    }
}

impl Shortcut {
    /// Parses a shortcut written like `"Cmd+Shift+K"`, `"Ctrl+`"`, `"F2"` or
    /// `"Alt+Backspace"`. `Cmd`/`Primary` mean Cmd on macOS and Ctrl elsewhere.
    pub fn parse(text: &str) -> Option<Self> {
        let parts: Vec<&str> = text.split('+').map(str::trim).collect();
        // "Cmd++" ends with an empty part: the key is "+".
        let (key, mods) = match parts.as_slice() {
            [.., a, b] if a.is_empty() && b.is_empty() => ("+", &parts[..parts.len() - 2]),
            [mods @ .., key] if !key.is_empty() => (*key, mods),
            _ => return None,
        };
        let mut m = Modifiers::NONE;
        for part in mods {
            m = m | match part.to_ascii_lowercase().as_str() {
                "cmd" | "command" | "primary" | "cmdorctrl" => Modifiers::PRIMARY,
                "shift" => Modifiers::SHIFT,
                "alt" | "option" | "opt" => Modifiers::ALT,
                "ctrl" | "control" => Modifiers::CTRL,
                "meta" | "super" | "win" => Modifiers::META,
                _ => return None,
            };
        }
        Some(Self::new(m, key))
    }
}

/// One entry of a native context menu (see [`Window::popup_menu`](crate::Window::popup_menu)
/// and `ipc.contextMenu` / `ui.contextMenu` in JS).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextMenuItem {
    Item {
        id: String,
        label: String,
        /// Shown next to the label, e.g. `Shortcut::parse("F2")`.
        shortcut: Option<Shortcut>,
        enabled: bool,
        checked: bool,
        /// Destructive action, drawn in red.
        danger: bool,
        /// Non-empty for a submenu.
        items: Vec<ContextMenuItem>,
    },
    Separator,
}

impl ContextMenuItem {
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self::Item {
            id: id.into(),
            label: label.into(),
            shortcut: None,
            enabled: true,
            checked: false,
            danger: false,
            items: Vec::new(),
        }
    }

    pub fn separator() -> Self {
        Self::Separator
    }

    fn with(mut self, f: impl FnOnce(&mut Self)) -> Self {
        f(&mut self);
        self
    }

    pub fn shortcut(self, sc: Shortcut) -> Self {
        self.with(|i| {
            if let Self::Item { shortcut, .. } = i {
                *shortcut = Some(sc);
            }
        })
    }

    pub fn enabled(self, on: bool) -> Self {
        self.with(|i| {
            if let Self::Item { enabled, .. } = i {
                *enabled = on;
            }
        })
    }

    pub fn checked(self, on: bool) -> Self {
        self.with(|i| {
            if let Self::Item { checked, .. } = i {
                *checked = on;
            }
        })
    }

    pub fn danger(self) -> Self {
        self.with(|i| {
            if let Self::Item { danger, .. } = i {
                *danger = true;
            }
        })
    }

    pub fn submenu(self, children: Vec<ContextMenuItem>) -> Self {
        self.with(|i| {
            if let Self::Item { items, .. } = i {
                *items = children;
            }
        })
    }

    /// Reads the page's description: `"-"` is a separator, objects are
    /// `{ id, label, shortcut?, enabled?, checked?, danger?, items? }`.
    pub(crate) fn from_page(value: &crate::ipc::Value) -> Result<Vec<Self>, String> {
        use crate::ipc::Value;
        let Value::Array(list) = value else {
            return Err("contextMenu expects an array of items".into());
        };
        list.iter()
            .map(|v| match v {
                Value::String(s) if s == "-" => Ok(Self::Separator),
                Value::Object(_) => {
                    let text = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_owned);
                    let flag = |k: &str, d: bool| v.get(k).and_then(Value::as_bool).unwrap_or(d);
                    let label = text("label").ok_or("context menu item needs a \"label\"")?;
                    let items = match v.get("items") {
                        Some(sub) if !matches!(sub, Value::Null) => Self::from_page(sub)?,
                        _ => Vec::new(),
                    };
                    let id = match text("id") {
                        Some(id) => id,
                        None if !items.is_empty() => String::new(),
                        None => {
                            return Err(format!("context menu item \"{label}\" needs an \"id\""));
                        }
                    };
                    let shortcut = match text("shortcut") {
                        Some(s) => Some(
                            Shortcut::parse(&s).ok_or_else(|| format!("bad shortcut \"{s}\""))?,
                        ),
                        None => None,
                    };
                    Ok(Self::Item {
                        id,
                        label,
                        shortcut,
                        enabled: flag("enabled", true),
                        checked: flag("checked", false),
                        danger: flag("danger", false),
                        items,
                    })
                }
                _ => Err("context menu items are objects or \"-\"".into()),
            })
            .collect()
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
    fn shortcut_parsing() {
        assert_eq!(
            Shortcut::parse("F2"),
            Some(Shortcut::new(Modifiers::NONE, "F2"))
        );
        assert_eq!(
            Shortcut::parse("Cmd+Shift+K"),
            Some(Shortcut::primary_shift("K"))
        );
        assert_eq!(Shortcut::parse("ctrl + `"), Some(Shortcut::ctrl("`")));
        assert_eq!(Shortcut::parse("Cmd++"), Some(Shortcut::primary("+")));
        assert_eq!(Shortcut::parse("Hyper+K"), None);
        assert_eq!(Shortcut::parse(""), None);
        assert_eq!(Shortcut::parse("Cmd+"), None);
    }

    #[test]
    fn context_menu_from_page() {
        let v = crate::json!([
            { "id": "rename", "label": "Rename", "shortcut": "F2" },
            "-",
            { "label": "Move to", "items": [{ "id": "a", "label": "A", "checked": true }] },
            { "id": "delete", "label": "Delete", "danger": true, "enabled": false }
        ]);
        let items = ContextMenuItem::from_page(&v).unwrap();
        assert_eq!(items.len(), 4);
        assert_eq!(
            items[0],
            ContextMenuItem::new("rename", "Rename").shortcut(Shortcut::new(Modifiers::NONE, "F2"))
        );
        assert_eq!(items[1], ContextMenuItem::Separator);
        let ContextMenuItem::Item { items: sub, .. } = &items[2] else {
            panic!()
        };
        assert_eq!(sub[0], ContextMenuItem::new("a", "A").checked(true));
        assert_eq!(
            items[3],
            ContextMenuItem::new("delete", "Delete")
                .danger()
                .enabled(false)
        );

        assert!(ContextMenuItem::from_page(&crate::json!({})).is_err());
        assert!(ContextMenuItem::from_page(&crate::json!([{ "label": "No id" }])).is_err());
        assert!(
            ContextMenuItem::from_page(
                &crate::json!([{ "id": "x", "label": "X", "shortcut": "Hyper+K" }])
            )
            .is_err()
        );
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
        let m = Menu::standard().with_settings().insert_submenu(
            Submenu::new("Go").item(MenuItem::new("panel", "Panel").emit().checked(true)),
        );
        let app = roles(&m.submenus[0]);
        assert_eq!(&app[..2], &[Some(Role::About), Some(Role::Settings)]);
        let MenuItem::Action { id, shortcut, .. } = &m.submenus[0].items[2] else {
            panic!("expected Settings after a separator");
        };
        assert_eq!(id, "settings");
        assert_eq!(shortcut, &Some(Shortcut::primary(",")));

        let titles: Vec<&str> = m.submenus.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(titles, ["App", "Edit", "View", "Go", "Window", "Help"]);
        let MenuItem::Action { emit, checked, .. } = &m.submenus[3].items[0] else {
            panic!("expected an action");
        };
        assert!(*emit && *checked);
    }
}
