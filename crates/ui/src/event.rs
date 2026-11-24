//! Lifecycle events emitted by the application, host windows, and webviews.

use crate::webview::{LoadState, WebViewId};

use crate::window::{Bounds, Theme, WindowId};

/// Top-level event delivered to application listeners.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Window(WindowId, WindowEvent),
    WebView(WebViewId, WebViewEvent),
    ThemeChanged(Theme),
    /// The app was activated again, e.g. its Dock icon was clicked with no
    /// windows open.
    Activate,
    QuitRequested,
    /// A menu item made with `.emit()` (or `Role::Settings`, as `"settings"`) was chosen.
    Menu(String),
    /// The app was asked to open a URL with one of its schemes (`url_schemes` in
    /// `[package.metadata.packager.app]`), e.g. a browser sign-in redirect.
    OpenUrl(String),
    /// Files were opened with the app (Finder, Dock icon drop, `open -a`).
    OpenFiles(Vec<std::path::PathBuf>),
    /// A second launch was forwarded by [`SingleInstance`](crate::SingleInstance),
    /// with its command-line arguments.
    SecondInstance(Vec<String>),
    /// A notification shown by this app was clicked; carries its id.
    NotificationClicked(String),
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
    /// The window entered (`true`) or left (`false`) full screen.
    FullscreenChanged(bool),
}

/// WebView engine events.
#[derive(Debug, Clone, PartialEq)]
pub enum WebViewEvent {
    LoadState(LoadState),
    TitleChanged(String),
    UrlChanged(String),
    /// Files dropped on the webview, with their real paths and the drop point
    /// (CSS pixels from the top-left). The page still gets its own `drop`
    /// event. Also sent to the page as `ui:file-drop` (WebKit).
    FileDrop {
        paths: Vec<std::path::PathBuf>,
        x: f64,
        y: f64,
    },
}

#[allow(non_upper_case_globals)]
impl Event {
    #[deprecated(since = "0.2.0", note = "renamed to `Event::Activate`")]
    pub const Reopen: Self = Self::Activate;
}
