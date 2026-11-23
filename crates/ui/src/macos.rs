//! macOS-only extensions, in the style of `std::os::unix`: `use ui::macos::AppExt;`
//! then `app.macos().dock()`. Calling them on another OS is a compile error, not
//! a silent no-op.

use std::sync::Arc;

use crate::app::App;
use crate::backend::Runtime;
use crate::backend::macos_appkit as appkit;
use crate::menu::MenuItem;
use crate::window::{Window, WindowBuilder, WindowId};
use crate::{Error, Result};

/// Adds [`macos`](AppExt::macos) to [`App`].
pub trait AppExt {
    /// macOS-only app features.
    fn macos(&self) -> MacApp;
}

impl AppExt for App {
    fn macos(&self) -> MacApp {
        MacApp {
            runtime: self.runtime.clone(),
        }
    }
}

/// macOS-only app features; see [`AppExt`].
#[derive(Clone)]
pub struct MacApp {
    runtime: Arc<Runtime>,
}

impl MacApp {
    /// The app's Dock tile.
    pub fn dock(&self) -> Dock {
        Dock {
            runtime: self.runtime.clone(),
        }
    }
}

impl std::fmt::Debug for MacApp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MacApp")
    }
}

/// How insistently [`Dock::bounce`] asks for attention.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Attention {
    /// One bounce.
    #[default]
    Informational,
    /// Bounces until the app is activated or the request is cancelled.
    Critical,
}

/// A pending [`Dock::bounce`], to pass to [`Dock::cancel_bounce`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttentionRequest(isize);

/// The app's Dock tile: icon, badge, attention and right-click menu.
///
/// ```ignore
/// use ui::macos::{AppExt, Attention};
/// let dock = app.macos().dock();
/// dock.set_icon(include_bytes!("../assets/AppIcon.icns"))?; // e.g. for `cargo run`
/// dock.set_badge("2");
/// dock.bounce(Attention::Informational);
/// dock.set_menu(vec![MenuItem::new("new_window", "New Window").emit()]);
/// ```
#[derive(Clone)]
pub struct Dock {
    runtime: Arc<Runtime>,
}

impl Dock {
    /// Sets the Dock icon from `.icns`, `.png` or `.jpg` bytes, e.g. so an
    /// unbundled `cargo run` binary shows the app's icon.
    pub fn set_icon(&self, image: &[u8]) -> Result<()> {
        let bytes = image.to_vec();
        if self.runtime.call_main_sync(move || appkit::dock_set_icon(&bytes)) {
            Ok(())
        } else {
            Err(Error::Platform("could not decode the Dock icon image".into()))
        }
    }

    /// Goes back to the bundle's icon (or the generic one when unbundled).
    pub fn reset_icon(&self) {
        self.runtime.call_main_sync(appkit::dock_reset_icon);
    }

    /// Shows a badge such as `"3"` or `"!"`; an empty string clears it.
    pub fn set_badge(&self, label: &str) {
        let label = label.to_owned();
        self.runtime
            .call_main_sync(move || appkit::dock_set_badge(Some(&label)));
    }

    pub fn clear_badge(&self) {
        self.runtime.call_main_sync(|| appkit::dock_set_badge(None));
    }

    /// The badge currently shown, if any.
    pub fn badge(&self) -> Option<String> {
        self.runtime.call_main_sync(appkit::dock_badge)
    }

    /// Bounces the Dock icon. Does nothing (returns `None`) while the app is active.
    pub fn bounce(&self, attention: Attention) -> Option<AttentionRequest> {
        let critical = attention == Attention::Critical;
        let id = self.runtime.call_main_sync(move || appkit::dock_bounce(critical));
        (id != 0).then_some(AttentionRequest(id))
    }

    pub fn cancel_bounce(&self, request: AttentionRequest) {
        self.runtime
            .call_main_sync(move || appkit::dock_cancel_bounce(request.0));
    }

    /// The menu shown when the Dock icon is right-clicked. Items made with
    /// `.on_click(..)` or `.emit()` behave as in the app menu (`.emit()` items
    /// reach the focused page as `menu:<id>` and the app as `Event::Menu(id)`).
    /// An empty list removes it.
    pub fn set_menu(&self, items: Vec<MenuItem>) {
        self.runtime.call_main_sync(move || appkit::set_dock_menu(&items));
    }
}

impl std::fmt::Debug for Dock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Dock")
    }
}

/// Title-bar height and layout with a toolbar (`NSWindowToolbarStyle`). Gives
/// taller unified title bars with the window controls centered vertically,
/// without `.window_controls(..)` pixel offsets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolbarStyle {
    /// The standard tall unified title bar (about 52pt).
    Unified,
    /// A shorter unified title bar (about 38pt).
    UnifiedCompact,
    /// Title above the toolbar, as in older apps.
    Expanded,
}

/// Window level (`NSWindowLevel`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WindowLevel {
    #[default]
    Normal,
    /// Floats above normal windows, like palettes and utility panels.
    Floating,
}

/// macOS-only window options, set with [`WindowBuilderExt::macos`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacWindowOptions {
    pub(crate) titlebar_separator: Option<bool>,
    pub(crate) toolbar_style: Option<ToolbarStyle>,
    pub(crate) window_controls_visible: bool,
    pub(crate) level: WindowLevel,
    pub(crate) all_spaces: bool,
    pub(crate) fullscreen_auxiliary: bool,
}

impl Default for MacWindowOptions {
    fn default() -> Self {
        Self {
            titlebar_separator: None,
            toolbar_style: None,
            window_controls_visible: true,
            level: WindowLevel::Normal,
            all_spaces: false,
            fullscreen_auxiliary: false,
        }
    }
}

impl MacWindowOptions {
    /// Shows or hides the hairline under the title bar.
    pub fn titlebar_separator(mut self, on: bool) -> Self {
        self.titlebar_separator = Some(on);
        self
    }

    pub fn toolbar_style(mut self, style: ToolbarStyle) -> Self {
        self.toolbar_style = Some(style);
        self
    }

    /// Hides the close, minimize and zoom buttons, e.g. on a sign-in screen.
    pub fn window_controls_visible(mut self, on: bool) -> Self {
        self.window_controls_visible = on;
        self
    }

    pub fn level(mut self, level: WindowLevel) -> Self {
        self.level = level;
        self
    }

    /// Shows the window on every Space.
    pub fn all_spaces(mut self, on: bool) -> Self {
        self.all_spaces = on;
        self
    }

    /// Lets the window appear over another app's full-screen window.
    pub fn fullscreen_auxiliary(mut self, on: bool) -> Self {
        self.fullscreen_auxiliary = on;
        self
    }
}

/// Adds [`macos`](WindowBuilderExt::macos) to [`WindowBuilder`].
///
/// ```ignore
/// use ui::macos::{ToolbarStyle, WindowBuilderExt};
/// Window::new("Editor")
///     .title_bar(TitleBar::inset(52))
///     .macos(|m| m.titlebar_separator(false).toolbar_style(ToolbarStyle::Unified))
///     .open(app)?;
/// ```
pub trait WindowBuilderExt {
    fn macos(self, f: impl FnOnce(MacWindowOptions) -> MacWindowOptions) -> Self;
}

impl WindowBuilderExt for WindowBuilder {
    fn macos(self, f: impl FnOnce(MacWindowOptions) -> MacWindowOptions) -> Self {
        self.map_macos(f)
    }
}

/// Adds [`macos`](WindowExt::macos) to [`Window`].
pub trait WindowExt {
    /// macOS-only features of an open window.
    fn macos(&self) -> MacWindow;
}

impl WindowExt for Window {
    fn macos(&self) -> MacWindow {
        MacWindow {
            id: self.id,
            runtime: self.runtime.clone(),
        }
    }
}

/// macOS-only features of an open window; see [`WindowExt`].
#[derive(Clone)]
pub struct MacWindow {
    id: WindowId,
    runtime: Arc<Runtime>,
}

impl MacWindow {
    /// The dot in the close button while there are unsaved changes.
    pub fn set_document_edited(&self, edited: bool) {
        let id = self.id;
        self.runtime
            .call_main_sync(move || appkit::set_document_edited(id, edited));
    }

    pub fn document_edited(&self) -> bool {
        let id = self.id;
        self.runtime.call_main_sync(move || appkit::document_edited(id))
    }

    pub fn set_window_controls_visible(&self, visible: bool) {
        let id = self.id;
        self.runtime
            .call_main_sync(move || appkit::set_window_controls_visible(id, visible));
    }

    pub fn set_level(&self, level: WindowLevel) {
        let id = self.id;
        self.runtime.call_main_sync(move || appkit::set_window_level(id, level));
    }

    pub fn level(&self) -> WindowLevel {
        let id = self.id;
        self.runtime.call_main_sync(move || appkit::window_level(id))
    }

    pub fn set_titlebar_separator(&self, on: bool) {
        let id = self.id;
        self.runtime
            .call_main_sync(move || appkit::set_titlebar_separator(id, on));
    }
}

impl std::fmt::Debug for MacWindow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MacWindow").field("id", &self.id).finish()
    }
}

/// Forgets what [`WindowBuilder::restore_state`] saved under `name`, e.g. for
/// a "Reset Window Layout" command.
pub fn forget_window_state(name: &str) {
    appkit::forget_window_state(name);
}
