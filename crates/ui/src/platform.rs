//! Escape-hatch raw native OS handles.

use std::ffi::c_void;

/// Raw host window handle returned by `Window::raw_handle()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawWindowHandle {
    #[cfg(target_os = "macos")]
    AppKit {
        ns_window: *mut c_void,
        content_view: *mut c_void,
    },
    #[cfg(target_os = "windows")]
    Win32 {
        hwnd: isize,
    },
    #[cfg(target_os = "linux")]
    X11 {
        window: u64,
        display: *mut c_void,
    },
    #[cfg(target_os = "linux")]
    Wayland {
        surface: *mut c_void,
        display: *mut c_void,
    },
    Unknown,
}

/// SAFETY: raw handles are opaque identifiers; callers must only dereference them on the UI thread.
unsafe impl Send for RawWindowHandle {}
/// SAFETY: raw handles are opaque identifiers; callers must only dereference them on the UI thread.
unsafe impl Sync for RawWindowHandle {}

/// Raw native webview handle returned by `WebView::raw_handle()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawWebViewHandle {
    #[cfg(target_os = "macos")]
    WkWebView {
        wk_webview: *mut c_void,
    },
    #[cfg(feature = "cef")]
    Cef {
        browser_id: i32,
    },
    Unknown,
}

/// SAFETY: raw handles are opaque identifiers; callers must only dereference them on the UI thread.
unsafe impl Send for RawWebViewHandle {}
/// SAFETY: raw handles are opaque identifiers; callers must only dereference them on the UI thread.
unsafe impl Sync for RawWebViewHandle {}
