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

/// A monitor: its name, bounds, scale and whether it's the primary one.
#[derive(Debug, Clone, PartialEq)]
pub struct Monitor {
    pub name: String,
    pub bounds: Bounds,
    pub scale: f64,
    pub primary: bool,
}

/// Renamed to [`Monitor`]: `Display` clashes with `std::fmt::Display`.
#[deprecated(since = "0.2.0", note = "renamed to `Monitor`")]
pub type Display = Monitor;

/// Where the window controls (close, minimize, zoom; caption buttons on
/// Windows and Linux) sit in an [`TitleBar::Overlay`] title bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WindowControls {
    pub x: u32,
    pub y: u32,
}

impl WindowControls {
    /// Places the first control's top-left corner at `(x, y)` points.
    pub const fn at(x: u32, y: u32) -> Self {
        Self { x, y }
    }
}

/// Window chrome style across macOS, Windows, and Linux.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TitleBar {
    /// Standard OS titlebar and window frame.
    #[default]
    Native,
    /// The page draws under a transparent title-bar strip `height` points tall,
    /// with the window controls on top (the web's Window Controls Overlay).
    Overlay {
        height: u32,
        window_controls: Option<WindowControls>,
    },
    /// No frame or title bar at all.
    Frameless,
}

#[allow(non_upper_case_globals)]
impl TitleBar {
    /// An overlay title bar `height` points tall.
    pub const fn overlay(height: u32) -> Self {
        Self::Overlay {
            height,
            window_controls: None,
        }
    }

    /// Positions the window controls in an `Overlay` title bar.
    pub const fn window_controls(self, controls: WindowControls) -> Self {
        match self {
            Self::Overlay { height, .. } => Self::Overlay {
                height,
                window_controls: Some(controls),
            },
            other => other,
        }
    }

    #[deprecated(since = "0.2.0", note = "renamed to `TitleBar::overlay`")]
    pub const fn inset(height: u32) -> Self {
        Self::overlay(height)
    }

    #[deprecated(
        since = "0.2.0",
        note = "use `.window_controls(WindowControls::at(x, y))`"
    )]
    pub const fn traffic_lights(self, x: u32, y: u32) -> Self {
        self.window_controls(WindowControls::at(x, y))
    }

    #[deprecated(since = "0.2.0", note = "renamed to `TitleBar::Frameless`")]
    pub const Hidden: Self = Self::Frameless;
}

/// Window background material, by purpose. Exact platform materials are a
/// platform-extension concern.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Backdrop {
    #[default]
    None,
    /// A sidebar's translucent material.
    Sidebar,
    /// A popover or menu-like transient panel.
    Popover,
    /// The material behind a whole window (Mica-like on Windows).
    Window,
}

#[allow(non_upper_case_globals)]
impl Backdrop {
    #[deprecated(since = "0.2.0", note = "renamed to `Backdrop::Popover`")]
    pub const Transient: Self = Self::Popover;
    #[deprecated(since = "0.2.0", note = "use `Backdrop::Window`")]
    pub const Mica: Self = Self::Window;
    #[deprecated(since = "0.2.0", note = "use `Backdrop::Window`")]
    pub const Acrylic: Self = Self::Window;
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
    /// Name under which the frame and full-screen state are remembered.
    pub restore_state: Option<String>,
    #[cfg(target_os = "macos")]
    pub macos: crate::macos::MacWindowOptions,
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
                restore_state: None,
                #[cfg(target_os = "macos")]
                macos: crate::macos::MacWindowOptions::default(),
            },
        }
    }

    /// Remembers the window's frame, monitor and full-screen state under `name`
    /// and restores them the next time a window with that name opens, instead
    /// of opening at the coded size and position every launch.
    pub fn restore_state(mut self, name: impl Into<String>) -> Self {
        self.spec.restore_state = Some(name.into());
        self
    }

    pub(crate) fn set_tabbing(&mut self, tabbing: Tabbing) {
        self.spec.tabbing = tabbing;
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn macos_tabbing(&self) -> Option<Tabbing> {
        self.spec.macos.tabbing
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn map_macos(
        mut self,
        f: impl FnOnce(crate::macos::MacWindowOptions) -> crate::macos::MacWindowOptions,
    ) -> Self {
        self.spec.macos = f(std::mem::take(&mut self.spec.macos));
        self
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

    #[deprecated(
        since = "0.2.0",
        note = "macOS-only: use `.macos(|m| m.tabbing(..))` with `ui::macos::WindowBuilderExt`"
    )]
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
