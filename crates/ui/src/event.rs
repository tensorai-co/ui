//! Lifecycle events emitted by the application, host windows, and webviews.

use crate::webview::{LoadState, WebViewId};
use crate::window::{Bounds, Theme, WindowId};

/// Top-level event delivered to application listeners.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Window(WindowId, WindowEvent),
    WebView(WebViewId, WebViewEvent),
    ThemeChanged(Theme),
    Reopen,
    QuitRequested,
    /// A menu item made with `.emit()` (or `Role::Settings`, as `"settings"`) was chosen.
    Menu(String),
}

/// Native window events.
#[derive(Debug, Clone, PartialEq)]
pub enum WindowEvent {
    Moved(i32, i32),
    Resized(u32, u32),
    Focused(bool),
    CloseRequested,
    Closed,
    ThemeChanged(Theme),
    BoundsChanged(Bounds),
}

/// WebView engine events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebViewEvent {
    LoadState(LoadState),
    TitleChanged(String),
    UrlChanged(String),
}
