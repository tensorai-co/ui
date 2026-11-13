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
    About,
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
    Minimize,
    Zoom,
    CloseWindow,
    Fullscreen,
}

/// Item inside a submenu.
#[derive(Clone)]
pub enum MenuItem {
    Action {
        id: String,
        title: String,
        shortcut: Option<Shortcut>,
        enabled: bool,
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
                role,
                ..
            } => f
                .debug_struct("Action")
                .field("id", id)
                .field("title", title)
                .field("shortcut", shortcut)
                .field("enabled", enabled)
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
            role: None,
            handler: None,
        }
    }

    pub fn role(role: Role) -> Self {
        let (title, shortcut) = match role {
            Role::About => ("About", None),
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
            Role::Minimize => ("Minimize", Some(Shortcut::cmd("m"))),
            Role::Zoom => ("Zoom", None),
            Role::CloseWindow => ("Close Window", Some(Shortcut::cmd("w"))),
            Role::Fullscreen => (
                "Toggle Full Screen",
                Some(Shortcut::new(Mod::PRIMARY | Mod::CTRL, "f")),
            ),
        };
        Self::Action {
            id: format!("role:{role:?}"),
            title: title.to_owned(),
            shortcut,
            enabled: true,
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

    /// Standard OS application + Edit + Window menu so `Cmd+C`/`V`/`X`/`A`/`Z`/`Q`/`W`/`M`
    /// work automatically out of the box in `WKWebView` and CEF inputs.
    pub fn standard() -> Self {
        Self::new()
            .submenu(
                Submenu::new("App")
                    .item(MenuItem::role(Role::About))
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
                Submenu::new("Window")
                    .item(MenuItem::role(Role::Minimize))
                    .item(MenuItem::role(Role::Zoom))
                    .item(MenuItem::role(Role::Fullscreen))
                    .separator()
                    .item(MenuItem::role(Role::CloseWindow)),
            )
    }
}
