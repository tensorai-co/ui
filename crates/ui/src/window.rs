//! Native host window creation, styling (titlebars, backdrops, tabbing), and thread-safe handles.

use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};

use crate::Result;
use crate::app::App;
use crate::backend::Runtime;
use crate::dialog::Dialog;
use crate::event::WindowEvent;
use crate::menu::Menu;
use crate::platform::RawWindowHandle;

/// Opaque unique window identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WindowId(pub u64);

/// Logical screen rectangle in OS-neutral top-left points relative to the primary display (for size)
/// or screen coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Bounds {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// Host UI color theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Theme {
    #[default]
    Light,
    Dark,
}

/// Window presentation state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum WindowState {
    #[default]
    Normal,
    Minimized,
    Maximized,
    Fullscreen,
}

/// Monitor / display descriptor.
#[derive(Debug, Clone, PartialEq)]
pub struct Display {
    pub name: String,
    pub bounds: Bounds,
    pub scale: f64,
    pub primary: bool,
}

/// Window chrome style across macOS, Windows, and Linux.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TitleBar {
    /// Standard OS titlebar and window frame.
    #[default]
    Native,
    /// Content extends beneath a transparent header strip (`height` logical points).
    /// On macOS, `.traffic_lights(x, y)` optionally offsets the standard window controls.
    Inset {
        height: u32,
        traffic_lights: Option<(u32, u32)>,
    },
    /// Borderless / chromeless window frame.
    Hidden,
}

impl TitleBar {
    /// Creates an inset titlebar with a unified drag/header strip height in logical points.
    pub const fn inset(height: u32) -> Self {
        Self::Inset {
            height,
            traffic_lights: None,
        }
    }

    /// Sets custom traffic-light `(x, y)` inset coordinates for `TitleBar::Inset`.
    pub const fn traffic_lights(self, x: u32, y: u32) -> Self {
        match self {
            Self::Inset { height, .. } => Self::Inset {
                height,
                traffic_lights: Some((x, y)),
            },
            other => other,
        }
    }
}

/// Native window material / translucency backdrop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Backdrop {
    #[default]
    None,
    Sidebar,
    Transient,
    Mica,
    Acrylic,
}

/// macOS native window tabbing behavior (`NSWindowTabbingMode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tabbing {
    #[default]
    Automatic,
    Preferred,
    Disallowed,
}

/// Shared thread-safe callback type for per-window events.
pub(crate) type WindowEventHandler = Arc<Mutex<Box<dyn FnMut(WindowEvent) + Send>>>;

pub(crate) struct WindowSpec {
    pub title: String,
    pub size: (u32, u32),
    pub min_size: Option<(u32, u32)>,
    pub max_size: Option<(u32, u32)>,
    pub position: Option<(i32, i32)>,
    pub resizable: bool,
    pub transparent: bool,
    pub visible: bool,
    pub title_bar: TitleBar,
    pub backdrop: Backdrop,
    pub tabbing: Tabbing,
    pub menu: Option<Menu>,
    pub on_event: Option<WindowEventHandler>,
}

/// Builder for configuring and opening a native window.
pub struct WindowBuilder {
    spec: WindowSpec,
}

impl WindowBuilder {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            spec: WindowSpec {
                title: title.into(),
                size: (960, 640),
                min_size: None,
                max_size: None,
                position: None,
                resizable: true,
                transparent: false,
                visible: true,
                title_bar: TitleBar::Native,
                backdrop: Backdrop::None,
                tabbing: Tabbing::Automatic,
                menu: None,
                on_event: None,
            },
        }
    }

    pub fn size(mut self, width: u32, height: u32) -> Self {
        self.spec.size = (width, height);
        self
    }

    pub fn min_size(mut self, width: u32, height: u32) -> Self {
        self.spec.min_size = Some((width, height));
        self
    }

    pub fn max_size(mut self, width: u32, height: u32) -> Self {
        self.spec.max_size = Some((width, height));
        self
    }

    pub fn position(mut self, x: i32, y: i32) -> Self {
        self.spec.position = Some((x, y));
        self
    }

    pub fn resizable(mut self, resizable: bool) -> Self {
        self.spec.resizable = resizable;
        self
    }

    pub fn transparent(mut self, transparent: bool) -> Self {
        self.spec.transparent = transparent;
        self
    }

    pub fn visible(mut self, visible: bool) -> Self {
        self.spec.visible = visible;
        self
    }

    pub fn title_bar(mut self, title_bar: TitleBar) -> Self {
        self.spec.title_bar = title_bar;
        self
    }

    pub fn backdrop(mut self, backdrop: Backdrop) -> Self {
        self.spec.backdrop = backdrop;
        self
    }

    pub fn tabbing(mut self, tabbing: Tabbing) -> Self {
        self.spec.tabbing = tabbing;
        self
    }

    pub fn menu(mut self, menu: Menu) -> Self {
        self.spec.menu = Some(menu);
        self
    }

    pub fn on_event(mut self, f: impl FnMut(WindowEvent) + Send + 'static) -> Self {
        self.spec.on_event = Some(Arc::new(Mutex::new(Box::new(f))));
        self
    }

    /// Creates and opens the native window attached to `app`.
    pub fn open(self, app: &App) -> Result<Window> {
        app.open_window(self.spec)
    }
}

/// Cheap, `Clone + Send + Sync` handle to an open native window.
#[derive(Clone)]
pub struct Window {
    pub(crate) id: WindowId,
    pub(crate) runtime: Arc<Runtime>,
}

impl Window {
    /// Starts building a new native window with the given title.
    #[allow(clippy::new_ret_no_self)]
    pub fn new(title: impl Into<String>) -> WindowBuilder {
        WindowBuilder::new(title)
    }

    pub fn id(&self) -> WindowId {
        self.id
    }

    pub fn set_title(&self, title: &str) {
        self.runtime.set_window_title(self.id, title.to_owned());
    }

    pub fn title(&self) -> String {
        self.runtime.window_title(self.id)
    }

    pub fn set_size(&self, width: u32, height: u32) {
        self.runtime.set_window_size(self.id, width, height);
    }

    pub fn bounds(&self) -> Bounds {
        self.runtime.window_bounds(self.id)
    }

    pub fn show(&self) {
        self.runtime.set_window_visible(self.id, true);
    }

    pub fn hide(&self) {
        self.runtime.set_window_visible(self.id, false);
    }

    pub fn focus(&self) {
        self.runtime.focus_window(self.id);
    }

    pub fn minimize(&self) {
        self.runtime.minimize_window(self.id);
    }

    pub fn maximize(&self) {
        self.runtime.maximize_window(self.id);
    }

    pub fn set_fullscreen(&self, fullscreen: bool) {
        self.runtime.set_window_fullscreen(self.id, fullscreen);
    }

    pub fn state(&self) -> WindowState {
        self.runtime.window_state(self.id)
    }

    /// The appearance the window draws with (its own, or the system's).
    pub fn theme(&self) -> Theme {
        self.runtime.window_theme(self.id)
    }

    /// Sets the window's appearance: vibrancy, window controls and native
    /// sheets follow it. `None` follows the system. Pages can do the same with
    /// `ipc.window.setTheme("light" | "dark" | "system")`.
    pub fn set_theme(&self, theme: Option<Theme>) {
        self.runtime.set_window_theme(self.id, theme);
    }

    /// Shows a native context menu at `(x, y)`, in points from the top-left of
    /// the window's content area, and returns the chosen item's id (or `None`
    /// when dismissed). Blocks until the menu closes, like a native menu.
    pub fn popup_menu(
        &self,
        items: Vec<crate::menu::ContextMenuItem>,
        x: f64,
        y: f64,
    ) -> Option<String> {
        self.runtime.popup_menu(self.id, items, x, y)
    }

    /// Starts a native OS window drag using the current pointer event.
    pub fn begin_drag(&self) {
        self.runtime.begin_window_drag(self.id);
    }

    pub fn close(&self) {
        self.runtime.close_window(self.id);
    }

    /// Returns a parented native dialog builder (sheet on macOS, owner modal on Windows/Linux).
    pub fn dialog(&self) -> Dialog {
        Dialog {
            window: self.id,
            runtime: self.runtime.clone(),
        }
    }

    pub fn raw_handle(&self) -> RawWindowHandle {
        self.runtime.raw_window_handle(self.id)
    }
}

impl std::fmt::Debug for Window {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Window").field("id", &self.id).finish()
    }
}

impl PartialEq for Window {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for Window {}

impl Hash for Window {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}
