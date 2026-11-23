//! Shared macOS AppKit host shell used by both the default `webkit` backend and `--features cef`:
//! `NSApplication` activation, `Menu` -> `NSMenu` responder wiring, `NSWindow` frames/titlebars,
//! traffic lights, backdrops, parented modal sheets, displays, and event loop management.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use crate::backend::macos_objc::{
    BOOL_ENC, Id, NIL, Point, Rect, Sel, Size, class, define_class, msg, ns_app, ns_number_bool,
    ns_string, rect_for, rect_of, release, retain, run_with_modal_block, rust_string, sel,
};
use crate::dialog::{
    FileDialogKind, FileDialogSpec, MessageButtons, MessageDialogSpec, MessageLevel, MessageResult,
};
use crate::event::{Event, WindowEvent};
use crate::menu::{ContextMenuItem, Menu, MenuItem, MenuItemUpdate, Mod, Role, Shortcut};
use crate::window::{
    Backdrop, Bounds, Display, Tabbing, Theme, TitleBar, WindowId, WindowSpec, WindowState,
};
use crate::{Error, Result};

// Window style mask constants.
const MASK_BORDERLESS: usize = 0;
const MASK_TITLED: usize = 1 << 0;
const MASK_CLOSABLE: usize = 1 << 1;
const MASK_MINIATURIZABLE: usize = 1 << 2;
const MASK_RESIZABLE: usize = 1 << 3;
const MASK_FULL_SIZE_CONTENT_VIEW: usize = 1 << 15;

const BACKING_BUFFERED: usize = 2;
const NS_VIEW_WIDTH_SIZABLE: usize = 2;
const NS_VIEW_HEIGHT_SIZABLE: usize = 16;

// Event masks / types.
const NSEVENT_TYPE_LEFT_MOUSE_DOWN: usize = 1;

// Native menu action callback table (main thread only; keyed by menu item tag `isize`).
thread_local! {
    static MENU_HANDLERS: RefCell<HashMap<isize, CustomMenuAction>> = RefCell::new(HashMap::new());
    /// Every built item by id (retained) with its tag, for `MenuItemHandle` updates.
    static MENU_ITEMS_BY_ID: RefCell<HashMap<String, (Id, isize)>> = RefCell::new(HashMap::new());
    /// Tags owned by the app menu, cleared when it is rebuilt (the Dock menu keeps its own).
    static APP_MENU_TAGS: RefCell<Vec<isize>> = const { RefCell::new(Vec::new()) };
    static DOCK_MENU_TAGS: RefCell<Vec<isize>> = const { RefCell::new(Vec::new()) };
    /// The Dock menu returned from `applicationDockMenu:` (retained), or nil.
    static DOCK_MENU: RefCell<Id> = const { RefCell::new(NIL) };
    static NEXT_MENU_TAG: RefCell<isize> = const { RefCell::new(1000) };
    static MENU_TARGET: RefCell<Id> = const { RefCell::new(NIL) };
    static LAST_LEFT_DOWN: RefCell<HashMap<usize, Id>> = RefCell::new(HashMap::new());
    static WINDOW_RECORDS: RefCell<HashMap<WindowId, NativeWindowRecord>> = RefCell::new(HashMap::new());
    static NS_TO_WINDOW_ID: RefCell<HashMap<usize, WindowId>> = RefCell::new(HashMap::new());
    static ACTIVE_DELEGATES: RefCell<HashMap<WindowId, WindowCallbacks>> = RefCell::new(HashMap::new());
}

/// A menu choice the runtime carries out (it needs the focused webview).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MenuCommand {
    /// An `.emit()` item: `menu:<id>` to the page, `Event::Menu(id)` to the app.
    Emit(String),
    /// A webview role such as `Reload` or `ZoomIn`.
    Role(Role),
}

#[derive(Clone)]
struct CustomMenuAction {
    handler: Option<Arc<dyn Fn() + Send + Sync>>,
    command: Option<MenuCommand>,
    enabled: bool,
}

static MENU_COMMANDS: Mutex<Vec<MenuCommand>> = Mutex::new(Vec::new());

pub(crate) fn drain_menu_commands() -> Vec<MenuCommand> {
    std::mem::take(&mut *MENU_COMMANDS.lock().unwrap_or_else(|e| e.into_inner()))
}

static APP_SHOULD_QUIT: AtomicBool = AtomicBool::new(false);
static APP_EVENT_QUEUE: Mutex<Vec<Event>> = Mutex::new(Vec::new());
static WINDOW_EVENT_QUEUE: Mutex<Vec<(WindowId, WindowEvent)>> = Mutex::new(Vec::new());

struct NativeWindowRecord {
    ns_window: Id,
    content_view: Id,
    delegate: Id,
    title_bar: TitleBar,
    backdrop: Backdrop,
    /// A toolbar style lays out the controls itself; don't move them.
    native_controls_layout: bool,
    /// `restore_state` name, for saving full-screen state.
    restore_name: Option<String>,
}

#[derive(Clone)]
struct WindowCallbacks {
    on_event: Option<crate::window::WindowEventHandler>,
}

pub(crate) fn drain_queued_events(mut app_handler: impl FnMut(Event)) {
    let app_evs = {
        let mut g = APP_EVENT_QUEUE.lock().unwrap_or_else(|e| e.into_inner());
        std::mem::take(&mut *g)
    };
    for ev in app_evs {
        app_handler(ev);
    }
    let win_evs = {
        let mut g = WINDOW_EVENT_QUEUE.lock().unwrap_or_else(|e| e.into_inner());
        std::mem::take(&mut *g)
    };
    for (win_id, ev) in win_evs {
        let cb =
            ACTIVE_DELEGATES.with(|d| d.borrow().get(&win_id).and_then(|w| w.on_event.clone()));
        if let Some(cb) = cb
            && let Ok(mut guard) = cb.lock()
        {
            guard(ev.clone());
        }
        app_handler(Event::Window(win_id, ev));
    }
}

pub(crate) fn should_quit_flag() -> bool {
    APP_SHOULD_QUIT.load(Ordering::Acquire)
}

pub(crate) fn request_quit() {
    APP_SHOULD_QUIT.store(true, Ordering::Release);
}

pub(crate) fn reset_quit() {
    APP_SHOULD_QUIT.store(false, Ordering::Release);
}

// ---------------------------------------------------------------------------
// Objective-C callbacks for NSApplicationDelegate / NSWindow / NSWindowDelegate / MenuTarget
// ---------------------------------------------------------------------------

extern "C" fn yes_bool(_: Id, _: Sel) -> bool {
    true
}

extern "C" fn should_terminate_after_last_window(_: Id, _: Sel, _: Id) -> bool {
    true
}

extern "C" fn app_will_terminate(_: Id, _: Sel, _: Id) {
    APP_SHOULD_QUIT.store(true, Ordering::Release);
}

/// `application:openURLs:` receives custom-scheme URLs and opened files alike.
extern "C" fn app_open_urls(_: Id, _: Sel, _: Id, urls: Id) {
    let count = if urls.is_null() { 0 } else { msg!(usize; urls, "count") };
    let mut files = Vec::new();
    let mut links = Vec::new();
    for i in 0..count {
        let url = msg!(Id; urls, "objectAtIndex:", i => usize);
        if url.is_null() {
            continue;
        }
        if msg!(bool; url, "isFileURL") {
            files.push(PathBuf::from(rust_string(msg!(Id; url, "path"))));
        } else {
            links.push(rust_string(msg!(Id; url, "absoluteString")));
        }
    }
    if let Ok(mut q) = APP_EVENT_QUEUE.lock() {
        q.extend(links.into_iter().map(Event::OpenUrl));
        if !files.is_empty() {
            q.push(Event::OpenFiles(files));
        }
    }
}

extern "C" fn app_should_reopen(_: Id, _: Sel, _: Id, _has_visible: bool) -> bool {
    if let Ok(mut q) = APP_EVENT_QUEUE.lock() {
        q.push(Event::Reopen);
    }
    true
}

extern "C" fn window_should_close(_: Id, _: Sel, ns_win: Id) -> bool {
    if let Some(id) = NS_TO_WINDOW_ID.with(|m| m.borrow().get(&(ns_win as usize)).copied())
        && let Ok(mut q) = WINDOW_EVENT_QUEUE.lock()
    {
        q.push((id, WindowEvent::CloseRequested));
    }
    true
}

extern "C" fn window_will_close(_: Id, _: Sel, notification: Id) {
    let ns_win = msg!(Id; notification, "object");
    if let Some(id) = NS_TO_WINDOW_ID.with(|m| m.borrow().get(&(ns_win as usize)).copied()) {
        if let Ok(mut q) = WINDOW_EVENT_QUEUE.lock() {
            q.push((id, WindowEvent::Closed));
        }
        // Save the frame now and release the autosave name, so a window
        // reopened under the same name restores it (AppKit allows one owner).
        if let Some(name) =
            WINDOW_RECORDS.with(|r| r.borrow().get(&id).and_then(|rec| rec.restore_name.clone()))
        {
            msg!((); ns_win, "saveFrameUsingName:", ns_string(&name) => Id);
            msg!(bool; ns_win, "setFrameAutosaveName:", ns_string("") => Id);
        }
        cleanup_window_record(id);
    }
}

extern "C" fn window_did_resize(_: Id, _: Sel, notification: Id) {
    let ns_win = msg!(Id; notification, "object");
    let entry = NS_TO_WINDOW_ID.with(|m| {
        m.borrow().get(&(ns_win as usize)).copied().and_then(|id| {
            WINDOW_RECORDS.with(|r| {
                r.borrow()
                    .get(&id)
                    .map(|rec| (id, rec.title_bar, rec.content_view, rec.native_controls_layout))
            })
        })
    });
    if let Some((id, title_bar, content_view, native_layout)) = entry {
        if let TitleBar::Inset {
            height,
            traffic_lights,
        } = title_bar
            && !(native_layout && traffic_lights.is_none())
        {
            position_traffic_lights(ns_win, height, traffic_lights);
        }
        let bounds = rect_of(content_view, "bounds");
        if let Ok(mut q) = WINDOW_EVENT_QUEUE.lock() {
            q.push((
                id,
                WindowEvent::Resized(
                    bounds.size.width.round() as u32,
                    bounds.size.height.round() as u32,
                ),
            ));
        }
    }
}

extern "C" fn window_did_end_live_resize(_: Id, _: Sel, notification: Id) {
    let ns_win = msg!(Id; notification, "object");
    let title_bar = NS_TO_WINDOW_ID.with(|m| {
        m.borrow()
            .get(&(ns_win as usize))
            .copied()
            .and_then(|id| {
                WINDOW_RECORDS.with(|r| {
                    r.borrow()
                        .get(&id)
                        .map(|rec| (rec.title_bar, rec.native_controls_layout))
                })
            })
    });
    if let Some((
        TitleBar::Inset {
            height,
            traffic_lights,
        },
        native_layout,
    )) = title_bar
        && !(native_layout && traffic_lights.is_none())
    {
        position_traffic_lights(ns_win, height, traffic_lights);
    }
}

extern "C" fn window_did_move(_: Id, _: Sel, notification: Id) {
    let ns_win = msg!(Id; notification, "object");
    if let Some(id) = NS_TO_WINDOW_ID.with(|m| m.borrow().get(&(ns_win as usize)).copied()) {
        let bounds = ns_window_bounds(ns_win);
        if let Ok(mut q) = WINDOW_EVENT_QUEUE.lock() {
            q.push((id, WindowEvent::Moved(bounds.x, bounds.y)));
            q.push((id, WindowEvent::BoundsChanged(bounds)));
        }
    }
}

extern "C" fn window_did_become_key(_: Id, _: Sel, notification: Id) {
    let ns_win = msg!(Id; notification, "object");
    if let Some(id) = NS_TO_WINDOW_ID.with(|m| m.borrow().get(&(ns_win as usize)).copied())
        && let Ok(mut q) = WINDOW_EVENT_QUEUE.lock()
    {
        q.push((id, WindowEvent::Focused(true)));
    }
}

extern "C" fn app_dock_menu(_: Id, _: Sel, _: Id) -> Id {
    DOCK_MENU.with(|m| *m.borrow())
}

fn push_fullscreen_event(notification: Id, fullscreen: bool) {
    let ns_win = msg!(Id; notification, "object");
    let Some(id) = NS_TO_WINDOW_ID.with(|m| m.borrow().get(&(ns_win as usize)).copied()) else {
        return;
    };
    if let Some(name) =
        WINDOW_RECORDS.with(|r| r.borrow().get(&id).and_then(|rec| rec.restore_name.clone()))
    {
        let defaults = msg!(Id; class("NSUserDefaults"), "standardUserDefaults");
        msg!((); defaults, "setBool:forKey:", fullscreen => bool, ns_string(&fullscreen_key(&name)) => Id);
    }
    if let Ok(mut q) = WINDOW_EVENT_QUEUE.lock() {
        q.push((id, WindowEvent::FullscreenChanged(fullscreen)));
    }
}

fn fullscreen_key(name: &str) -> String {
    format!("ui.window.{name}.fullscreen")
}

/// Removes the frame and full-screen state saved by `restore_state(name)`.
pub(crate) fn forget_window_state(name: &str) {
    let defaults = msg!(Id; class("NSUserDefaults"), "standardUserDefaults");
    msg!((); defaults, "removeObjectForKey:", ns_string(&format!("NSWindow Frame {name}")) => Id);
    msg!((); defaults, "removeObjectForKey:", ns_string(&fullscreen_key(name)) => Id);
}

fn ns_window_for(id: WindowId) -> Option<Id> {
    WINDOW_RECORDS.with(|r| r.borrow().get(&id).map(|rec| rec.ns_window))
}

pub(crate) fn set_document_edited(id: WindowId, edited: bool) {
    if let Some(w) = ns_window_for(id) {
        msg!((); w, "setDocumentEdited:", edited => bool);
    }
}

pub(crate) fn document_edited(id: WindowId) -> bool {
    ns_window_for(id).is_some_and(|w| msg!(bool; w, "isDocumentEdited"))
}

pub(crate) fn set_window_controls_visible(id: WindowId, visible: bool) {
    if let Some(w) = ns_window_for(id) {
        apply_controls_visible(w, visible);
    }
}

fn apply_controls_visible(ns_win: Id, visible: bool) {
    for kind in 0usize..3 {
        let btn = msg!(Id; ns_win, "standardWindowButton:", kind => usize);
        if !btn.is_null() {
            msg!((); btn, "setHidden:", !visible => bool);
        }
    }
}

const NS_FLOATING_WINDOW_LEVEL: isize = 3;

pub(crate) fn set_window_level(id: WindowId, level: crate::macos::WindowLevel) {
    if let Some(w) = ns_window_for(id) {
        apply_level(w, level);
    }
}

fn apply_level(ns_win: Id, level: crate::macos::WindowLevel) {
    let raw = match level {
        crate::macos::WindowLevel::Normal => 0isize,
        crate::macos::WindowLevel::Floating => NS_FLOATING_WINDOW_LEVEL,
    };
    msg!((); ns_win, "setLevel:", raw => isize);
}

pub(crate) fn window_level(id: WindowId) -> crate::macos::WindowLevel {
    match ns_window_for(id).map(|w| msg!(isize; w, "level")) {
        Some(l) if l >= NS_FLOATING_WINDOW_LEVEL => crate::macos::WindowLevel::Floating,
        _ => crate::macos::WindowLevel::Normal,
    }
}

pub(crate) fn set_titlebar_separator(id: WindowId, on: bool) {
    if let Some(w) = ns_window_for(id) {
        apply_titlebar_separator(w, on);
    }
}

fn apply_titlebar_separator(ns_win: Id, on: bool) {
    if msg!(bool; ns_win, "respondsToSelector:", sel("setTitlebarSeparatorStyle:") => Sel) {
        // NSTitlebarSeparatorStyleNone = 1, Line = 2.
        let style: isize = if on { 2 } else { 1 };
        msg!((); ns_win, "setTitlebarSeparatorStyle:", style => isize);
    }
}

/// Applies `MacWindowOptions` before the window is shown.
fn apply_mac_options(ns_win: Id, opts: &crate::macos::MacWindowOptions) {
    use crate::macos::ToolbarStyle;
    if let Some(style) = opts.toolbar_style {
        let toolbar = msg!(Id; msg!(Id; class("NSToolbar"), "alloc"), "initWithIdentifier:", ns_string("ui.toolbar") => Id);
        if !toolbar.is_null() {
            msg!((); toolbar, "setShowsBaselineSeparator:", false => bool);
            msg!((); ns_win, "setToolbar:", toolbar => Id);
            release(toolbar);
        }
        if msg!(bool; ns_win, "respondsToSelector:", sel("setToolbarStyle:") => Sel) {
            // NSWindowToolbarStyleExpanded = 1, Unified = 3, UnifiedCompact = 4.
            let raw: isize = match style {
                ToolbarStyle::Expanded => 1,
                ToolbarStyle::Unified => 3,
                ToolbarStyle::UnifiedCompact => 4,
            };
            msg!((); ns_win, "setToolbarStyle:", raw => isize);
        }
    }
    if let Some(on) = opts.titlebar_separator {
        apply_titlebar_separator(ns_win, on);
    }
    if !opts.window_controls_visible {
        apply_controls_visible(ns_win, false);
    }
    if opts.level != crate::macos::WindowLevel::Normal {
        apply_level(ns_win, opts.level);
    }
    let mut behavior = msg!(usize; ns_win, "collectionBehavior");
    if opts.all_spaces {
        behavior |= 1 << 0; // NSWindowCollectionBehaviorCanJoinAllSpaces
    }
    if opts.fullscreen_auxiliary {
        behavior |= 1 << 8; // NSWindowCollectionBehaviorFullScreenAuxiliary
    }
    msg!((); ns_win, "setCollectionBehavior:", behavior => usize);
}

extern "C" fn window_did_enter_full_screen(_: Id, _: Sel, notification: Id) {
    push_fullscreen_event(notification, true);
}

extern "C" fn window_did_exit_full_screen(_: Id, _: Sel, notification: Id) {
    push_fullscreen_event(notification, false);
}

extern "C" fn window_did_resign_key(_: Id, _: Sel, notification: Id) {
    let ns_win = msg!(Id; notification, "object");
    if let Some(id) = NS_TO_WINDOW_ID.with(|m| m.borrow().get(&(ns_win as usize)).copied())
        && let Ok(mut q) = WINDOW_EVENT_QUEUE.lock()
    {
        q.push((id, WindowEvent::Focused(false)));
    }
}

extern "C" fn on_custom_menu_item(_: Id, _: Sel, sender: Id) {
    let tag = msg!(isize; sender, "tag");
    let Some(action) = MENU_HANDLERS.with(|h| h.borrow().get(&tag).cloned()) else {
        return;
    };
    if let Some(cb) = action.handler {
        cb();
    }
    if let Some(cmd) = action.command {
        MENU_COMMANDS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(cmd);
    }
}

/// Keeps `MenuItem::enabled(false)` and `set_enabled` in effect: with
/// `autoenablesItems`, AppKit asks the target instead of reading `isEnabled`.
extern "C" fn validate_menu_item(_: Id, _: Sel, item: Id) -> bool {
    let tag = msg!(isize; item, "tag");
    MENU_HANDLERS.with(|h| h.borrow().get(&tag).is_none_or(|a| a.enabled))
}

#[cfg(feature = "cef")]
static CEF_HANDLING_SEND_EVENT: AtomicBool = AtomicBool::new(false);

#[cfg(feature = "cef")]
extern "C" fn cef_is_handling_send_event(_: Id, _: Sel) -> bool {
    CEF_HANDLING_SEND_EVENT.load(Ordering::Acquire)
}

#[cfg(feature = "cef")]
extern "C" fn cef_set_handling_send_event(_: Id, _: Sel, flag: bool) {
    CEF_HANDLING_SEND_EVENT.store(flag, Ordering::Release);
}

#[cfg(feature = "cef")]
pub(crate) fn install_cef_app_protocol() {
    use crate::backend::macos_objc::{class_addMethod, class_addProtocol, objc_getProtocol};
    use std::ffi::CString;

    static DONE: OnceLock<()> = OnceLock::new();
    DONE.get_or_init(|| {
        let app_cls = class("NSApplication");
        if app_cls.is_null() {
            return;
        }
        let get_types = CString::new(format!("{BOOL_ENC}@:")).unwrap_or_default();
        let set_types = CString::new(format!("v@:{BOOL_ENC}")).unwrap_or_default();
        let proto_name = CString::new("CefAppProtocol").unwrap_or_default();
        // SAFETY: Adds `CefAppProtocol` selector implementations onto `NSApplication` once at startup.
        unsafe {
            class_addMethod(
                app_cls,
                sel("isHandlingSendEvent"),
                cef_is_handling_send_event as *const c_void,
                get_types.as_ptr(),
            );
            class_addMethod(
                app_cls,
                sel("setHandlingSendEvent:"),
                cef_set_handling_send_event as *const c_void,
                set_types.as_ptr(),
            );
            let proto = objc_getProtocol(proto_name.as_ptr());
            if !proto.is_null() {
                class_addProtocol(app_cls, proto);
            }
        }
    });
}

fn ensure_classes() -> (Id, Id, Id, Id) {
    static CLASSES: OnceLock<(usize, usize, usize, usize)> = OnceLock::new();
    let &(app_del, win_cls, win_del, menu_target) = CLASSES.get_or_init(|| {
        let notif_types = "v@:@".to_owned();
        let bool_self_types = format!("{BOOL_ENC}@:");
        let app_del_cls = define_class(
            "UiAppDelegate",
            "NSObject",
            &[
                (
                    "applicationShouldTerminateAfterLastWindowClosed:",
                    should_terminate_after_last_window as *const c_void,
                    &format!("{BOOL_ENC}@:@"),
                ),
                (
                    "applicationDockMenu:",
                    app_dock_menu as *const c_void,
                    "@@:@",
                ),
                (
                    "application:openURLs:",
                    app_open_urls as *const c_void,
                    "v@:@@",
                ),
                (
                    "applicationShouldHandleReopen:hasVisibleWindows:",
                    app_should_reopen as *const c_void,
                    &format!("{BOOL_ENC}@:@{BOOL_ENC}"),
                ),
                (
                    "applicationWillTerminate:",
                    app_will_terminate as *const c_void,
                    &notif_types,
                ),
            ],
            &["NSApplicationDelegate"],
        );
        let win_cls = define_class(
            "UiNSWindow",
            "NSWindow",
            &[
                (
                    "canBecomeKeyWindow",
                    yes_bool as *const c_void,
                    &bool_self_types,
                ),
                (
                    "canBecomeMainWindow",
                    yes_bool as *const c_void,
                    &bool_self_types,
                ),
            ],
            &[],
        );
        let win_del_cls = define_class(
            "UiWindowDelegate",
            "NSObject",
            &[
                (
                    "windowShouldClose:",
                    window_should_close as *const c_void,
                    &format!("{BOOL_ENC}@:@"),
                ),
                (
                    "windowWillClose:",
                    window_will_close as *const c_void,
                    &notif_types,
                ),
                (
                    "windowDidResize:",
                    window_did_resize as *const c_void,
                    &notif_types,
                ),
                (
                    "windowDidEndLiveResize:",
                    window_did_end_live_resize as *const c_void,
                    &notif_types,
                ),
                (
                    "windowDidMove:",
                    window_did_move as *const c_void,
                    &notif_types,
                ),
                (
                    "windowDidBecomeKey:",
                    window_did_become_key as *const c_void,
                    &notif_types,
                ),
                (
                    "windowDidEnterFullScreen:",
                    window_did_enter_full_screen as *const c_void,
                    &notif_types,
                ),
                (
                    "windowDidExitFullScreen:",
                    window_did_exit_full_screen as *const c_void,
                    &notif_types,
                ),
                (
                    "windowDidResignKey:",
                    window_did_resign_key as *const c_void,
                    &notif_types,
                ),
            ],
            &["NSWindowDelegate"],
        );
        let menu_target_cls = define_class(
            "UiMenuTarget",
            "NSObject",
            &[
                (
                    "onCustomMenuItem:",
                    on_custom_menu_item as *const c_void,
                    "v@:@",
                ),
                (
                    "validateMenuItem:",
                    validate_menu_item as *const c_void,
                    if BOOL_ENC == "c" { "c@:@" } else { "B@:@" },
                ),
            ],
            &[],
        );
        (
            app_del_cls as usize,
            win_cls as usize,
            win_del_cls as usize,
            menu_target_cls as usize,
        )
    });
    (
        app_del as Id,
        win_cls as Id,
        win_del as Id,
        menu_target as Id,
    )
}

pub(crate) fn init_ns_app(with_cef_protocol: bool) -> Result<Id> {
    reset_quit();
    #[cfg(feature = "cef")]
    if with_cef_protocol {
        install_cef_app_protocol();
    }
    #[cfg(not(feature = "cef"))]
    let _ = with_cef_protocol;

    let (app_del_cls, _, _, _) = ensure_classes();
    let app = ns_app();
    if app.is_null() {
        return Err(Error::Platform(
            "NSApplication sharedApplication returned null".into(),
        ));
    }
    let del = msg!(Id; msg!(Id; app_del_cls, "alloc"), "init");
    msg!((); app, "setDelegate:", del => Id);
    // Regular activation policy so bare `cargo run` gets Dock icon, menu bar, and focus.
    msg!(bool; app, "setActivationPolicy:", 0isize => isize);
    set_app_menu(&Menu::standard());
    msg!((); app, "finishLaunching");
    msg!((); app, "activateIgnoringOtherApps:", true => bool);
    Ok(app)
}

pub(crate) fn record_event_if_mouse_down(ev: Id) {
    if ev.is_null() {
        return;
    }
    let kind = msg!(usize; ev, "type");
    if kind == NSEVENT_TYPE_LEFT_MOUSE_DOWN {
        let win = msg!(Id; ev, "window");
        if !win.is_null() {
            LAST_LEFT_DOWN.with(|m| {
                let mut map = m.borrow_mut();
                if let Some(old) = map.insert(win as usize, retain(ev)) {
                    release(old);
                }
            });
        }
    }
}

pub(crate) fn begin_window_drag(win_id: WindowId) {
    let ns_win = WINDOW_RECORDS.with(|r| r.borrow().get(&win_id).map(|rec| rec.ns_window));
    let Some(ns_win) = ns_win else {
        return;
    };
    let ev = msg!(Id; ns_app(), "currentEvent");
    let drag_ev = if !ev.is_null() && msg!(usize; ev, "type") == NSEVENT_TYPE_LEFT_MOUSE_DOWN {
        ev
    } else {
        LAST_LEFT_DOWN.with(|m| m.borrow().get(&(ns_win as usize)).copied().unwrap_or(ev))
    };
    if !drag_ev.is_null() {
        msg!((); ns_win, "performWindowDragWithEvent:", drag_ev => Id);
    }
}

pub(crate) fn open_ns_window(id: WindowId, spec: &WindowSpec) -> Result<Id> {
    let (_, win_cls, win_del_cls, _) = ensure_classes();
    let (w, h) = (f64::from(spec.size.0), f64::from(spec.size.1));

    let style = match spec.title_bar {
        TitleBar::Native => {
            let mut m = MASK_TITLED | MASK_CLOSABLE | MASK_MINIATURIZABLE;
            if spec.resizable {
                m |= MASK_RESIZABLE;
            }
            m
        }
        TitleBar::Inset { .. } => {
            let mut m =
                MASK_TITLED | MASK_CLOSABLE | MASK_MINIATURIZABLE | MASK_FULL_SIZE_CONTENT_VIEW;
            if spec.resizable {
                m |= MASK_RESIZABLE;
            }
            m
        }
        TitleBar::Hidden => {
            if spec.resizable {
                MASK_BORDERLESS | MASK_RESIZABLE
            } else {
                MASK_BORDERLESS
            }
        }
    };

    let origin = if let Some((x, y)) = spec.position {
        let primary_h = primary_screen_height();
        Point {
            x: f64::from(x),
            y: primary_h - (f64::from(y) + h),
        }
    } else {
        Point { x: 0.0, y: 0.0 }
    };
    let rect = Rect {
        origin,
        size: Size {
            width: w,
            height: h,
        },
    };

    let alloc_win = msg!(Id; win_cls, "alloc");
    let ns_win = msg!(Id; alloc_win, "initWithContentRect:styleMask:backing:defer:",
        rect => Rect, style => usize, BACKING_BUFFERED => usize, false => bool);
    if ns_win.is_null() {
        return Err(Error::Platform("failed to allocate NSWindow".into()));
    }

    // Never auto-release NSWindow behind our handles on close button click.
    msg!((); ns_win, "setReleasedWhenClosed:", false => bool);
    msg!((); ns_win, "setTitle:", ns_string(&spec.title) => Id);

    if let Some((mw, mh)) = spec.min_size {
        msg!((); ns_win, "setContentMinSize:", Size { width: f64::from(mw), height: f64::from(mh) } => Size);
    }
    if let Some((mw, mh)) = spec.max_size {
        msg!((); ns_win, "setContentMaxSize:", Size { width: f64::from(mw), height: f64::from(mh) } => Size);
    }

    let tab_mode: isize = match spec.tabbing {
        Tabbing::Automatic => 0,
        Tabbing::Preferred => 1,
        Tabbing::Disallowed => 2,
    };
    msg!((); ns_win, "setTabbingMode:", tab_mode => isize);

    match spec.title_bar {
        TitleBar::Inset { .. } => {
            msg!((); ns_win, "setTitlebarAppearsTransparent:", true => bool);
            msg!((); ns_win, "setTitleVisibility:", 1isize => isize); // NSWindowTitleHidden
        }
        TitleBar::Hidden => {
            msg!((); ns_win, "setHasShadow:", true => bool);
        }
        TitleBar::Native => {}
    }

    let content_view = msg!(Id; ns_win, "contentView");
    if spec.transparent || spec.backdrop != Backdrop::None {
        msg!((); ns_win, "setOpaque:", false => bool);
        let clear = msg!(Id; class("NSColor"), "clearColor");
        msg!((); ns_win, "setBackgroundColor:", clear => Id);
    }

    if spec.backdrop != Backdrop::None {
        let material: isize = match spec.backdrop {
            Backdrop::Sidebar => 7,                   // NSVisualEffectMaterialSidebar
            Backdrop::Transient => 6,                 // NSVisualEffectMaterialPopover
            Backdrop::Mica | Backdrop::Acrylic => 21, // NSVisualEffectMaterialUnderWindowBackground
            Backdrop::None => 0,
        };
        let bounds = rect_of(content_view, "bounds");
        let fx = msg!(Id; msg!(Id; class("NSVisualEffectView"), "alloc"), "initWithFrame:", bounds => Rect);
        if !fx.is_null() {
            msg!((); fx, "setAutoresizingMask:", NS_VIEW_WIDTH_SIZABLE | NS_VIEW_HEIGHT_SIZABLE => usize);
            msg!((); fx, "setMaterial:", material => isize);
            msg!((); fx, "setBlendingMode:", 0isize => isize); // BehindWindow
            msg!((); fx, "setState:", 0isize => isize); // FollowsWindowActiveState
            msg!((); content_view, "addSubview:", fx => Id);
            release(fx);
        }
    }

    if let Some(ref menu) = spec.menu {
        set_app_menu(menu);
    }

    apply_mac_options(ns_win, &spec.macos);

    let mut restored = false;
    if let Some(name) = &spec.restore_state {
        restored = msg!(bool; ns_win, "setFrameUsingName:", ns_string(name) => Id);
        msg!(bool; ns_win, "setFrameAutosaveName:", ns_string(name) => Id);
    }
    if spec.position.is_none() && !restored {
        msg!((); ns_win, "center");
    }

    let del = msg!(Id; msg!(Id; win_del_cls, "alloc"), "init");
    msg!((); ns_win, "setDelegate:", del => Id);

    WINDOW_RECORDS.with(|r| {
        r.borrow_mut().insert(
            id,
            NativeWindowRecord {
                ns_window: ns_win,
                content_view,
                delegate: del,
                title_bar: spec.title_bar,
                backdrop: spec.backdrop,
                native_controls_layout: spec.macos.toolbar_style.is_some(),
                restore_name: spec.restore_state.clone(),
            },
        );
    });
    NS_TO_WINDOW_ID.with(|m| {
        m.borrow_mut().insert(ns_win as usize, id);
    });
    ACTIVE_DELEGATES.with(|d| {
        d.borrow_mut().insert(
            id,
            WindowCallbacks {
                on_event: spec.on_event.clone(),
            },
        );
    });

    if spec.visible {
        msg!((); ns_win, "makeKeyAndOrderFront:", NIL => Id);
        msg!((); ns_app(), "activateIgnoringOtherApps:", true => bool);
    }

    if let TitleBar::Inset {
        height,
        traffic_lights,
    } = spec.title_bar
        && !(spec.macos.toolbar_style.is_some() && traffic_lights.is_none())
    {
        position_traffic_lights(ns_win, height, traffic_lights);
    }

    if let Some(name) = &spec.restore_state {
        let defaults = msg!(Id; class("NSUserDefaults"), "standardUserDefaults");
        if spec.visible
            && msg!(bool; defaults, "boolForKey:", ns_string(&fullscreen_key(name)) => Id)
        {
            msg!((); ns_win, "toggleFullScreen:", NIL => Id);
        }
    }

    Ok(ns_win)
}

fn position_traffic_lights(ns_win: Id, height: u32, traffic_lights: Option<(u32, u32)>) {
    let close_btn = msg!(Id; ns_win, "standardWindowButton:", 0usize => usize);
    let mini_btn = msg!(Id; ns_win, "standardWindowButton:", 1usize => usize);
    let zoom_btn = msg!(Id; ns_win, "standardWindowButton:", 2usize => usize);
    if close_btn.is_null() || mini_btn.is_null() || zoom_btn.is_null() {
        return;
    }
    let titlebar_view = msg!(Id; close_btn, "superview");
    if titlebar_view.is_null() {
        return;
    }
    let titlebar_container = msg!(Id; titlebar_view, "superview");
    let container_h = f64::from(height.max(24));

    if !titlebar_container.is_null() {
        let theme_frame = msg!(Id; titlebar_container, "superview");
        if !theme_frame.is_null() {
            let theme_bounds = rect_of(theme_frame, "bounds");
            let mut tc_frame = rect_of(titlebar_container, "frame");
            tc_frame.size.height = container_h;
            tc_frame.origin.y = theme_bounds.size.height - container_h;
            msg!((); titlebar_container, "setFrame:", tc_frame => Rect);

            let mut tv_frame = rect_of(titlebar_view, "frame");
            tv_frame.origin.y = 0.0;
            tv_frame.size.height = container_h;
            msg!((); titlebar_view, "setFrame:", tv_frame => Rect);
        }
    }

    let close_rect = rect_of(close_btn, "frame");
    let mini_rect = rect_of(mini_btn, "frame");
    let spacing = if mini_rect.origin.x > close_rect.origin.x {
        mini_rect.origin.x - close_rect.origin.x
    } else {
        20.0
    };
    let btn_h = close_rect.size.height.max(12.0);
    let flipped = msg!(bool; titlebar_view, "isFlipped");
    let tv_h = rect_of(titlebar_view, "bounds")
        .size
        .height
        .max(container_h);

    let (start_x, top_y) = match traffic_lights {
        Some((x, y)) => (f64::from(x), f64::from(y)),
        None => (14.0, ((container_h - btn_h) / 2.0).round()),
    };
    let target_y = if flipped { top_y } else { tv_h - top_y - btn_h };

    for (idx, btn) in [close_btn, mini_btn, zoom_btn].into_iter().enumerate() {
        let pt = Point {
            x: start_x + spacing * (idx as f64),
            y: target_y,
        };
        msg!((); btn, "setFrameOrigin:", pt => Point);
    }
}

pub(crate) fn window_ns_ids(id: WindowId) -> Option<(Id, Id, Backdrop)> {
    WINDOW_RECORDS.with(|r| {
        r.borrow()
            .get(&id)
            .map(|rec| (rec.ns_window, rec.content_view, rec.backdrop))
    })
}

pub(crate) fn cleanup_window_record(id: WindowId) {
    let rec = WINDOW_RECORDS.with(|r| r.borrow_mut().remove(&id));
    ACTIVE_DELEGATES.with(|d| d.borrow_mut().remove(&id));
    if let Some(rec) = rec {
        NS_TO_WINDOW_ID.with(|m| m.borrow_mut().remove(&(rec.ns_window as usize)));
        LAST_LEFT_DOWN.with(|m| {
            if let Some(ev) = m.borrow_mut().remove(&(rec.ns_window as usize)) {
                release(ev);
            }
        });
        msg!((); rec.ns_window, "setDelegate:", NIL => Id);
        release(rec.delegate);
        release(rec.ns_window);
    }
}

pub(crate) fn set_window_title(id: WindowId, title: &str) {
    if let Some((ns_win, _, _)) = window_ns_ids(id) {
        msg!((); ns_win, "setTitle:", ns_string(title) => Id);
    }
}

pub(crate) fn window_title(id: WindowId) -> String {
    if let Some((ns_win, _, _)) = window_ns_ids(id) {
        return rust_string(msg!(Id; ns_win, "title"));
    }
    String::new()
}

pub(crate) fn set_window_size(id: WindowId, width: u32, height: u32) {
    if let Some((ns_win, _, _)) = window_ns_ids(id) {
        let cur = rect_of(ns_win, "frame");
        let cur_content = rect_for(ns_win, "contentRectForFrameRect:", cur);
        let new_content = Rect {
            origin: Point {
                x: cur_content.origin.x,
                y: cur_content.origin.y + cur_content.size.height - f64::from(height),
            },
            size: Size {
                width: f64::from(width),
                height: f64::from(height),
            },
        };
        let new_frame = rect_for(ns_win, "frameRectForContentRect:", new_content);
        msg!((); ns_win, "setFrame:display:", new_frame => Rect, true => bool);
    }
}

pub(crate) fn window_bounds(id: WindowId) -> Bounds {
    if let Some((ns_win, _, _)) = window_ns_ids(id) {
        return ns_window_bounds(ns_win);
    }
    Bounds::default()
}

fn primary_screen_height() -> f64 {
    let screens = msg!(Id; class("NSScreen"), "screens");
    if screens.is_null() || msg!(usize; screens, "count") == 0 {
        return 900.0;
    }
    let primary = msg!(Id; screens, "objectAtIndex:", 0usize => usize);
    rect_of(primary, "frame").size.height
}

fn ns_window_bounds(ns_win: Id) -> Bounds {
    let frame = rect_of(ns_win, "frame");
    let content = rect_for(ns_win, "contentRectForFrameRect:", frame);
    let primary_h = primary_screen_height();
    Bounds {
        x: content.origin.x.round() as i32,
        y: (primary_h - (content.origin.y + content.size.height)).round() as i32,
        width: content.size.width.max(0.0).round() as u32,
        height: content.size.height.max(0.0).round() as u32,
    }
}

pub(crate) fn set_window_visible(id: WindowId, visible: bool) {
    if let Some((ns_win, _, _)) = window_ns_ids(id) {
        if visible {
            msg!((); ns_win, "makeKeyAndOrderFront:", NIL => Id);
        } else {
            msg!((); ns_win, "orderOut:", NIL => Id);
        }
    }
}

pub(crate) fn focus_window(id: WindowId) {
    if let Some((ns_win, _, _)) = window_ns_ids(id) {
        msg!((); ns_app(), "activateIgnoringOtherApps:", true => bool);
        msg!((); ns_win, "makeKeyAndOrderFront:", NIL => Id);
    }
}

pub(crate) fn minimize_window(id: WindowId) {
    if let Some((ns_win, _, _)) = window_ns_ids(id) {
        msg!((); ns_win, "miniaturize:", NIL => Id);
    }
}

pub(crate) fn maximize_window(id: WindowId) {
    if let Some((ns_win, _, _)) = window_ns_ids(id) {
        msg!((); ns_win, "zoom:", NIL => Id);
    }
}

/// What a title-bar double click should do, from System Settings > Desktop & Dock
/// ("Double-click a window's title bar to…"), stored in `AppleActionOnDoubleClick`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TitlebarAction {
    Zoom,
    Minimize,
    None,
}

pub(crate) fn titlebar_action_for(setting: Option<&str>, legacy_minimize: bool) -> TitlebarAction {
    match setting {
        Some("Minimize") => TitlebarAction::Minimize,
        Some("None") => TitlebarAction::None,
        // "Maximize" (zoom) and "Fill" both zoom the window.
        Some(_) => TitlebarAction::Zoom,
        // Older systems only stored the minimize checkbox.
        None if legacy_minimize => TitlebarAction::Minimize,
        None => TitlebarAction::Zoom,
    }
}

pub(crate) fn titlebar_double_click(id: WindowId) {
    let defaults = msg!(Id; class("NSUserDefaults"), "standardUserDefaults");
    let action = if defaults.is_null() {
        TitlebarAction::Zoom
    } else {
        let value = msg!(Id; defaults, "stringForKey:", ns_string("AppleActionOnDoubleClick") => Id);
        let setting = (!value.is_null()).then(|| rust_string(value));
        let legacy = msg!(bool; defaults, "boolForKey:", ns_string("AppleMiniaturizeOnDoubleClick") => Id);
        titlebar_action_for(setting.as_deref(), legacy)
    };
    match action {
        TitlebarAction::Zoom => maximize_window(id),
        TitlebarAction::Minimize => minimize_window(id),
        TitlebarAction::None => {}
    }
}

pub(crate) fn set_window_fullscreen(id: WindowId, fullscreen: bool) {
    if let Some((ns_win, _, _)) = window_ns_ids(id) {
        let mask = msg!(usize; ns_win, "styleMask");
        let is_fs = (mask & (1 << 14)) != 0; // NSWindowStyleMaskFullScreen
        if is_fs != fullscreen {
            msg!((); ns_win, "toggleFullScreen:", NIL => Id);
        }
    }
}

pub(crate) fn window_state(id: WindowId) -> WindowState {
    if let Some((ns_win, _, _)) = window_ns_ids(id) {
        if msg!(bool; ns_win, "isMiniaturized") {
            return WindowState::Minimized;
        }
        let mask = msg!(usize; ns_win, "styleMask");
        if (mask & (1 << 14)) != 0 {
            return WindowState::Fullscreen;
        }
        if msg!(bool; ns_win, "isZoomed") {
            return WindowState::Maximized;
        }
    }
    WindowState::Normal
}

pub(crate) fn close_window(id: WindowId) {
    if let Some((ns_win, _, _)) = window_ns_ids(id) {
        msg!((); ns_win, "close");
        cleanup_window_record(id);
    }
}

/// What a page needs to lay itself out around the window's title bar and controls.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct WindowChrome {
    /// Height of the title-bar strip the page draws under (0 with a native title bar).
    pub titlebar_height: f64,
    /// Width the window controls take from the left edge, margins included.
    pub controls_left: f64,
    pub fullscreen: bool,
    pub focused: bool,
}

pub(crate) fn window_chrome(id: WindowId) -> Option<WindowChrome> {
    let (ns_win, title_bar) =
        WINDOW_RECORDS.with(|r| r.borrow().get(&id).map(|rec| (rec.ns_window, rec.title_bar)))?;
    let mask = msg!(usize; ns_win, "styleMask");
    let fullscreen = (mask & (1 << 14)) != 0; // NSWindowStyleMaskFullScreen
    let focused = msg!(bool; ns_win, "isKeyWindow");
    let native_layout =
        WINDOW_RECORDS.with(|r| r.borrow().get(&id).is_some_and(|rec| rec.native_controls_layout));
    let (titlebar_height, controls_left) = match title_bar {
        TitleBar::Inset { height, .. } => {
            let left = if fullscreen {
                0.0
            } else {
                window_controls_extent(ns_win)
            };
            let h = if native_layout {
                // The toolbar style decides the height: content above the layout rect.
                let content = msg!(Id; ns_win, "contentView");
                let full = rect_of(content, "bounds").size.height;
                let layout = rect_of(ns_win, "contentLayoutRect").size.height;
                (full - layout).max(0.0).round()
            } else {
                f64::from(height)
            };
            (h, left)
        }
        TitleBar::Native | TitleBar::Hidden => (0.0, 0.0),
    };
    Some(WindowChrome {
        titlebar_height,
        controls_left,
        fullscreen,
        focused,
    })
}

/// Right edge of the zoom button plus the same margin the close button has on
/// the left, i.e. the strip a page should leave free for the controls.
fn window_controls_extent(ns_win: Id) -> f64 {
    let close_btn = msg!(Id; ns_win, "standardWindowButton:", 0usize => usize);
    let zoom_btn = msg!(Id; ns_win, "standardWindowButton:", 2usize => usize);
    if close_btn.is_null() || zoom_btn.is_null() || msg!(bool; zoom_btn, "isHidden") {
        return 0.0;
    }
    let close = rect_of(close_btn, "frame");
    let zoom = rect_of(zoom_btn, "frame");
    (zoom.origin.x + zoom.size.width + close.origin.x.max(0.0)).round()
}

/// Sets this window's appearance; `None` follows the system.
pub(crate) fn set_window_theme(id: WindowId, theme: Option<Theme>) {
    let Some((ns_win, _, _)) = window_ns_ids(id) else {
        return;
    };
    let appearance = match theme {
        Some(Theme::Light) => {
            msg!(Id; class("NSAppearance"), "appearanceNamed:", ns_string("NSAppearanceNameAqua") => Id)
        }
        Some(Theme::Dark) => {
            msg!(Id; class("NSAppearance"), "appearanceNamed:", ns_string("NSAppearanceNameDarkAqua") => Id)
        }
        None => NIL,
    };
    msg!((); ns_win, "setAppearance:", appearance => Id);
}

/// The appearance this window actually draws with.
pub(crate) fn window_theme(id: WindowId) -> Theme {
    let Some((ns_win, _, _)) = window_ns_ids(id) else {
        return app_theme();
    };
    let appearance = msg!(Id; ns_win, "effectiveAppearance");
    if appearance.is_null() {
        return app_theme();
    }
    if rust_string(msg!(Id; appearance, "name")).contains("Dark") {
        Theme::Dark
    } else {
        Theme::Light
    }
}

pub(crate) fn app_theme() -> Theme {
    let app = ns_app();
    if app.is_null() {
        return Theme::Light;
    }
    let appearance = msg!(Id; app, "effectiveAppearance");
    if appearance.is_null() {
        return Theme::Light;
    }
    let name = rust_string(msg!(Id; appearance, "name"));
    if name.contains("Dark") {
        Theme::Dark
    } else {
        Theme::Light
    }
}

pub(crate) fn displays() -> Vec<Display> {
    let screens = msg!(Id; class("NSScreen"), "screens");
    if screens.is_null() {
        return Vec::new();
    }
    let count = msg!(usize; screens, "count");
    let primary_h = primary_screen_height();
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let s = msg!(Id; screens, "objectAtIndex:", i => usize);
        let frame = rect_of(s, "frame");
        let scale = msg!(f64; s, "backingScaleFactor").max(1.0);
        let name = if msg!(bool; s, "respondsToSelector:", sel("localizedName") => Sel) {
            rust_string(msg!(Id; s, "localizedName"))
        } else {
            format!("Display {}", i + 1)
        };
        out.push(Display {
            name,
            bounds: Bounds {
                x: frame.origin.x.round() as i32,
                y: (primary_h - (frame.origin.y + frame.size.height)).round() as i32,
                width: frame.size.width.max(0.0).round() as u32,
                height: frame.size.height.max(0.0).round() as u32,
            },
            scale,
            primary: i == 0,
        });
    }
    out
}

pub(crate) fn open_external(url: &str) -> Result<()> {
    let ns_url = msg!(Id; class("NSURL"), "URLWithString:", ns_string(url) => Id);
    if ns_url.is_null() {
        return Err(Error::InvalidState(format!("invalid external URL: {url}")));
    }
    let ws = msg!(Id; class("NSWorkspace"), "sharedWorkspace");
    let ok = msg!(bool; ws, "openURL:", ns_url => Id);
    if ok {
        Ok(())
    } else {
        Err(Error::Platform(format!(
            "NSWorkspace failed to open URL: {url}"
        )))
    }
}

pub(crate) fn notify(title: &str, body: &str) {
    // Use osascript on macOS to avoid UNUserNotificationCenter exceptions on unbundled `cargo run` binaries.
    let esc_title = title.replace('\\', "\\\\").replace('"', "\\\"");
    let esc_body = body.replace('\\', "\\\\").replace('"', "\\\"");
    let script = format!("display notification \"{esc_body}\" with title \"{esc_title}\"");
    let _ = Command::new("/usr/bin/osascript")
        .arg("-e")
        .arg(script)
        .spawn();
}

// ---------------------------------------------------------------------------
// Native NSMenu construction
// ---------------------------------------------------------------------------

const MOD_SHIFT: usize = 1 << 17;
const MOD_CONTROL: usize = 1 << 18;
const MOD_OPTION: usize = 1 << 19;
const MOD_COMMAND: usize = 1 << 20;

/// The app's display name: `CFBundleName` when bundled, else the process name.
pub(crate) fn app_name() -> String {
    let bundle = msg!(Id; class("NSBundle"), "mainBundle");
    if !bundle.is_null() {
        for key in ["CFBundleDisplayName", "CFBundleName"] {
            let v = msg!(Id; bundle, "objectForInfoDictionaryKey:", ns_string(key) => Id);
            if !v.is_null() && msg!(bool; v, "isKindOfClass:", class("NSString") => Id) {
                let name = rust_string(v);
                if !name.is_empty() {
                    return name;
                }
            }
        }
    }
    let info = msg!(Id; class("NSProcessInfo"), "processInfo");
    rust_string(msg!(Id; info, "processName"))
}

/// `CFBundleShortVersionString` when running from a bundle.
pub(crate) fn bundle_version() -> Option<String> {
    let bundle = msg!(Id; class("NSBundle"), "mainBundle");
    if bundle.is_null() {
        return None;
    }
    let v = msg!(Id; bundle, "objectForInfoDictionaryKey:", ns_string("CFBundleShortVersionString") => Id);
    (!v.is_null() && msg!(bool; v, "isKindOfClass:", class("NSString") => Id))
        .then(|| rust_string(v))
        .filter(|s| !s.is_empty())
}

const PASTEBOARD_STRING: &str = "public.utf8-plain-text";

pub(crate) fn clipboard_text() -> Option<String> {
    let pb = msg!(Id; class("NSPasteboard"), "generalPasteboard");
    if pb.is_null() {
        return None;
    }
    let s = msg!(Id; pb, "stringForType:", ns_string(PASTEBOARD_STRING) => Id);
    (!s.is_null()).then(|| rust_string(s))
}

pub(crate) fn set_clipboard_text(text: &str) {
    let pb = msg!(Id; class("NSPasteboard"), "generalPasteboard");
    if pb.is_null() {
        return;
    }
    msg!(isize; pb, "clearContents");
    msg!(bool; pb, "setString:forType:", ns_string(text) => Id, ns_string(PASTEBOARD_STRING) => Id);
}

/// Default role titles that read better with the app's name, as macOS apps do.
pub(crate) fn role_title_with_app(role: Role, title: &str, app: &str) -> Option<String> {
    match (role, title) {
        (Role::About, "About") => Some(format!("About {app}")),
        (Role::Hide, "Hide") => Some(format!("Hide {app}")),
        (Role::Quit, "Quit") => Some(format!("Quit {app}")),
        (Role::Help, "Help") => Some(format!("{app} Help")),
        _ => None,
    }
}

fn next_menu_tag() -> isize {
    NEXT_MENU_TAG.with(|t| {
        let mut cur = t.borrow_mut();
        let v = *cur;
        *cur += 1;
        v
    })
}

fn native_role_selector(role: Role) -> Option<&'static str> {
    Some(match role {
        Role::About => "orderFrontStandardAboutPanel:",
        Role::Hide => "hide:",
        Role::HideOthers => "hideOtherApplications:",
        Role::ShowAll => "unhideAllApplications:",
        Role::Quit => "terminate:",
        Role::Undo => "undo:",
        Role::Redo => "redo:",
        Role::Cut => "cut:",
        Role::Copy => "copy:",
        Role::Paste => "paste:",
        Role::SelectAll => "selectAll:",
        Role::Minimize => "performMiniaturize:",
        Role::Zoom => "performZoom:",
        Role::BringAllToFront => "arrangeInFront:",
        Role::CloseWindow => "performClose:",
        Role::Fullscreen => "toggleFullScreen:",
        Role::Help => "showHelp:",
        Role::Services
        | Role::Settings
        | Role::Reload
        | Role::Back
        | Role::Forward
        | Role::ActualSize
        | Role::ZoomIn
        | Role::ZoomOut
        | Role::ToggleDevTools => return None,
    })
}

pub(crate) fn set_app_menu(menu: &Menu) {
    let (_, _, _, target_cls) = ensure_classes();
    APP_MENU_TAGS.with(|t| {
        let tags = std::mem::take(&mut *t.borrow_mut());
        MENU_HANDLERS.with(|h| {
            let mut h = h.borrow_mut();
            for tag in tags {
                h.remove(&tag);
            }
        });
    });
    MENU_ITEMS_BY_ID.with(|m| {
        for (_, (item, _)) in m.borrow_mut().drain() {
            release(item);
        }
    });
    let menu_target = MENU_TARGET.with(|slot| {
        let mut s = slot.borrow_mut();
        if s.is_null() {
            *s = retain(msg!(Id; msg!(Id; target_cls, "alloc"), "init"));
        }
        *s
    });
    let app = app_name();

    let main_menu =
        msg!(Id; msg!(Id; class("NSMenu"), "alloc"), "initWithTitle:", ns_string("MainMenu") => Id);
    for sub in &menu.submenus {
        let top_item = msg!(Id; class("NSMenuItem"), "new");
        let sub_menu = msg!(Id; msg!(Id; class("NSMenu"), "alloc"), "initWithTitle:", ns_string(&sub.title) => Id);
        for item in &sub.items {
            match item {
                MenuItem::Separator => {
                    let sep = msg!(Id; class("NSMenuItem"), "separatorItem");
                    msg!((); sub_menu, "addItem:", sep => Id);
                }
                MenuItem::Action {
                    id,
                    title,
                    shortcut,
                    enabled,
                    checked,
                    emit,
                    role,
                    handler,
                } => {
                    let title = role
                        .and_then(|r| role_title_with_app(r, title, &app))
                        .unwrap_or_else(|| title.clone());

                    if *role == Some(Role::Services) {
                        let ns_item = msg!(Id; msg!(Id; class("NSMenuItem"), "alloc"), "initWithTitle:action:keyEquivalent:",
                            ns_string(&title) => Id, std::ptr::null::<c_void>() as Sel => Sel, ns_string("") => Id);
                        let services = msg!(Id; msg!(Id; class("NSMenu"), "alloc"), "initWithTitle:", ns_string(&title) => Id);
                        msg!((); ns_item, "setSubmenu:", services => Id);
                        msg!((); ns_app(), "setServicesMenu:", services => Id);
                        msg!((); sub_menu, "addItem:", ns_item => Id);
                        release(services);
                        release(ns_item);
                        continue;
                    }

                    let key = shortcut
                        .as_ref()
                        .map(|s| ns_key_equivalent(&s.key))
                        .unwrap_or_default();
                    let native_sel = role.and_then(native_role_selector);
                    let command = match role {
                        Some(Role::Settings) => Some(MenuCommand::Emit(id.clone())),
                        Some(r) if r.is_webview_role() => Some(MenuCommand::Role(*r)),
                        Some(_) => None,
                        None if *emit => Some(MenuCommand::Emit(id.clone())),
                        None => None,
                    };
                    let custom = native_sel.is_none() && (handler.is_some() || command.is_some());
                    let action_sel = native_sel.unwrap_or("onCustomMenuItem:");
                    let ns_item = msg!(Id; msg!(Id; class("NSMenuItem"), "alloc"), "initWithTitle:action:keyEquivalent:",
                        ns_string(&title) => Id, sel(action_sel) => Sel, ns_string(&key) => Id);

                    if let Some(sc) = shortcut {
                        msg!((); ns_item, "setKeyEquivalentModifierMask:", modifier_mask(sc) => usize);
                    }
                    msg!((); ns_item, "setEnabled:", *enabled => bool);
                    if *checked {
                        msg!((); ns_item, "setState:", 1isize => isize);
                    }

                    let tag = if custom {
                        let tag = next_menu_tag();
                        MENU_HANDLERS.with(|h| {
                            h.borrow_mut().insert(
                                tag,
                                CustomMenuAction {
                                    handler: handler.clone(),
                                    command,
                                    enabled: *enabled,
                                },
                            )
                        });
                        msg!((); ns_item, "setTag:", tag => isize);
                        msg!((); ns_item, "setTarget:", menu_target => Id);
                        APP_MENU_TAGS.with(|t| t.borrow_mut().push(tag));
                        tag
                    } else {
                        0
                    };
                    MENU_ITEMS_BY_ID.with(|m| {
                        if let Some((old, _)) =
                            m.borrow_mut().insert(id.clone(), (retain(ns_item), tag))
                        {
                            release(old);
                        }
                    });
                    msg!((); sub_menu, "addItem:", ns_item => Id);
                    release(ns_item);
                }
            }
        }
        msg!((); top_item, "setSubmenu:", sub_menu => Id);
        msg!((); main_menu, "addItem:", top_item => Id);
        if sub.title.eq_ignore_ascii_case("Window") {
            msg!((); ns_app(), "setWindowsMenu:", sub_menu => Id);
        }
        if sub.title.eq_ignore_ascii_case("Help") {
            msg!((); ns_app(), "setHelpMenu:", sub_menu => Id);
        }
        release(sub_menu);
        release(top_item);
    }
    msg!((); ns_app(), "setMainMenu:", main_menu => Id);
    release(main_menu);
}

fn modifier_mask(sc: &Shortcut) -> usize {
    let mut mask = 0usize;
    if sc.mods.contains(Mod::PRIMARY) || sc.mods.contains(Mod::META) {
        mask |= MOD_COMMAND;
    }
    if sc.mods.contains(Mod::SHIFT) {
        mask |= MOD_SHIFT;
    }
    if sc.mods.contains(Mod::ALT) {
        mask |= MOD_OPTION;
    }
    if sc.mods.contains(Mod::CTRL) {
        mask |= MOD_CONTROL;
    }
    mask
}

/// `NSMenuItem.keyEquivalent` for a shortcut key name: function, arrow and
/// editing keys map to their AppKit characters, single characters are lowercased.
pub(crate) fn ns_key_equivalent(key: &str) -> String {
    let lower = key.to_ascii_lowercase();
    let named = match lower.as_str() {
        "backspace" => Some('\u{8}'),
        "delete" | "del" => Some('\u{7f}'),
        "enter" | "return" => Some('\r'),
        "escape" | "esc" => Some('\u{1b}'),
        "tab" => Some('\t'),
        "space" => Some(' '),
        "up" => Some('\u{F700}'),
        "down" => Some('\u{F701}'),
        "left" => Some('\u{F702}'),
        "right" => Some('\u{F703}'),
        _ => None,
    };
    if let Some(c) = named {
        return c.to_string();
    }
    if let Some(n) = lower.strip_prefix('f').and_then(|n| n.parse::<u32>().ok())
        && (1..=20).contains(&n)
        && let Some(c) = char::from_u32(0xF704 + n - 1)
    {
        return c.to_string();
    }
    lower
}

/// Builds an `NSMenu` for a context menu; chosen items write their id into `choice`.
fn build_context_menu(
    items: &[ContextMenuItem],
    target: Id,
    choice: &Arc<Mutex<Option<String>>>,
    tags: &mut Vec<isize>,
) -> Id {
    let menu = msg!(Id; msg!(Id; class("NSMenu"), "alloc"), "initWithTitle:", ns_string("") => Id);
    msg!((); menu, "setAutoenablesItems:", false => bool);
    for item in items {
        match item {
            ContextMenuItem::Separator => {
                let sep = msg!(Id; class("NSMenuItem"), "separatorItem");
                msg!((); menu, "addItem:", sep => Id);
            }
            ContextMenuItem::Item {
                id,
                label,
                shortcut,
                enabled,
                checked,
                danger,
                items: children,
            } => {
                let key = shortcut
                    .as_ref()
                    .map(|s| ns_key_equivalent(&s.key))
                    .unwrap_or_default();
                let ns_item = msg!(Id; msg!(Id; class("NSMenuItem"), "alloc"), "initWithTitle:action:keyEquivalent:",
                    ns_string(label) => Id, sel("onCustomMenuItem:") => Sel, ns_string(&key) => Id);
                if let Some(sc) = shortcut {
                    msg!((); ns_item, "setKeyEquivalentModifierMask:", modifier_mask(sc) => usize);
                }
                msg!((); ns_item, "setEnabled:", *enabled => bool);
                if *checked {
                    msg!((); ns_item, "setState:", 1isize => isize);
                }
                if *danger {
                    let red = msg!(Id; class("NSColor"), "systemRedColor");
                    let attrs = msg!(Id; class("NSDictionary"), "dictionaryWithObject:forKey:",
                        red => Id, ns_string("NSColor") => Id);
                    let title = msg!(Id; msg!(Id; class("NSAttributedString"), "alloc"),
                        "initWithString:attributes:", ns_string(label) => Id, attrs => Id);
                    msg!((); ns_item, "setAttributedTitle:", title => Id);
                    release(title);
                }
                if children.is_empty() {
                    let tag = next_menu_tag();
                    let choice = Arc::clone(choice);
                    let id = id.clone();
                    MENU_HANDLERS.with(|h| {
                        h.borrow_mut().insert(
                            tag,
                            CustomMenuAction {
                                handler: Some(Arc::new(move || {
                                    *choice.lock().unwrap_or_else(|e| e.into_inner()) =
                                        Some(id.clone());
                                })),
                                command: None,
                                enabled: true,
                            },
                        )
                    });
                    tags.push(tag);
                    msg!((); ns_item, "setTag:", tag => isize);
                    msg!((); ns_item, "setTarget:", target => Id);
                } else {
                    let sub = build_context_menu(children, target, choice, tags);
                    msg!((); ns_item, "setSubmenu:", sub => Id);
                    release(sub);
                }
                msg!((); menu, "addItem:", ns_item => Id);
                release(ns_item);
            }
        }
    }
    menu
}

/// Shows a context menu at `(x, y)` (points from the content area's top-left)
/// and returns the chosen item's id once it closes, or `None` if dismissed.
pub(crate) fn popup_context_menu(
    win_id: WindowId,
    items: &[ContextMenuItem],
    x: f64,
    y: f64,
) -> Option<String> {
    let (_, _, _, target_cls) = ensure_classes();
    let target = MENU_TARGET.with(|slot| {
        let mut s = slot.borrow_mut();
        if s.is_null() {
            *s = retain(msg!(Id; msg!(Id; target_cls, "alloc"), "init"));
        }
        *s
    });
    let content = WINDOW_RECORDS.with(|r| r.borrow().get(&win_id).map(|rec| rec.content_view))?;
    let choice = Arc::new(Mutex::new(None));
    let mut tags = Vec::new();
    let menu = build_context_menu(items, target, &choice, &mut tags);
    let bounds = rect_of(content, "bounds");
    let flipped = msg!(bool; content, "isFlipped");
    let at = Point {
        x,
        y: if flipped { y } else { bounds.size.height - y },
    };
    msg!(bool; menu, "popUpMenuPositioningItem:atLocation:inView:", NIL => Id, at => Point, content => Id);
    release(menu);
    MENU_HANDLERS.with(|h| {
        let mut h = h.borrow_mut();
        for tag in tags {
            h.remove(&tag);
        }
    });
    choice.lock().unwrap_or_else(|e| e.into_inner()).take()
}

// ---------------------------------------------------------------------------
// Dock tile
// ---------------------------------------------------------------------------

/// Sets the Dock icon from `.icns` / `.png` / `.jpg` bytes. Returns false if
/// AppKit can't decode them.
pub(crate) fn dock_set_icon(bytes: &[u8]) -> bool {
    let data = msg!(Id; class("NSData"), "dataWithBytes:length:",
        bytes.as_ptr() as *const c_void => *const c_void, bytes.len() => usize);
    if data.is_null() {
        return false;
    }
    let image = msg!(Id; msg!(Id; class("NSImage"), "alloc"), "initWithData:", data => Id);
    if image.is_null() {
        return false;
    }
    msg!((); ns_app(), "setApplicationIconImage:", image => Id);
    release(image);
    true
}

pub(crate) fn dock_reset_icon() {
    msg!((); ns_app(), "setApplicationIconImage:", NIL => Id);
}

pub(crate) fn dock_set_badge(label: Option<&str>) {
    let tile = msg!(Id; ns_app(), "dockTile");
    if tile.is_null() {
        return;
    }
    let value = label.filter(|l| !l.is_empty()).map_or(NIL, ns_string);
    msg!((); tile, "setBadgeLabel:", value => Id);
}

pub(crate) fn dock_badge() -> Option<String> {
    let tile = msg!(Id; ns_app(), "dockTile");
    if tile.is_null() {
        return None;
    }
    let v = msg!(Id; tile, "badgeLabel");
    (!v.is_null()).then(|| rust_string(v)).filter(|s| !s.is_empty())
}

/// `requestUserAttention:`; returns the request id (0 when the app is already active).
pub(crate) fn dock_bounce(critical: bool) -> isize {
    // NSCriticalRequest = 0, NSInformationalRequest = 10.
    let kind: usize = if critical { 0 } else { 10 };
    msg!(isize; ns_app(), "requestUserAttention:", kind => usize)
}

pub(crate) fn dock_cancel_bounce(request: isize) {
    msg!((); ns_app(), "cancelUserAttentionRequest:", request => isize);
}

/// Replaces the menu shown when the Dock icon is right-clicked. Items with
/// `.on_click` or `.emit()` work as in the app menu; an empty list removes it.
pub(crate) fn set_dock_menu(items: &[MenuItem]) {
    let (_, _, _, target_cls) = ensure_classes();
    DOCK_MENU_TAGS.with(|t| {
        let tags = std::mem::take(&mut *t.borrow_mut());
        MENU_HANDLERS.with(|h| {
            let mut h = h.borrow_mut();
            for tag in tags {
                h.remove(&tag);
            }
        });
    });
    let old = DOCK_MENU.with(|m| std::mem::replace(&mut *m.borrow_mut(), NIL));
    if !old.is_null() {
        release(old);
    }
    if items.is_empty() {
        return;
    }
    let target = MENU_TARGET.with(|slot| {
        let mut s = slot.borrow_mut();
        if s.is_null() {
            *s = retain(msg!(Id; msg!(Id; target_cls, "alloc"), "init"));
        }
        *s
    });
    let menu = msg!(Id; msg!(Id; class("NSMenu"), "alloc"), "initWithTitle:", ns_string("Dock") => Id);
    for item in items {
        match item {
            MenuItem::Separator => {
                let sep = msg!(Id; class("NSMenuItem"), "separatorItem");
                msg!((); menu, "addItem:", sep => Id);
            }
            MenuItem::Action {
                id,
                title,
                enabled,
                checked,
                emit,
                handler,
                ..
            } => {
                let ns_item = msg!(Id; msg!(Id; class("NSMenuItem"), "alloc"), "initWithTitle:action:keyEquivalent:",
                    ns_string(title) => Id, sel("onCustomMenuItem:") => Sel, ns_string("") => Id);
                if *checked {
                    msg!((); ns_item, "setState:", 1isize => isize);
                }
                let tag = next_menu_tag();
                MENU_HANDLERS.with(|h| {
                    h.borrow_mut().insert(
                        tag,
                        CustomMenuAction {
                            handler: handler.clone(),
                            command: emit.then(|| MenuCommand::Emit(id.clone())),
                            enabled: *enabled,
                        },
                    )
                });
                DOCK_MENU_TAGS.with(|t| t.borrow_mut().push(tag));
                msg!((); ns_item, "setTag:", tag => isize);
                msg!((); ns_item, "setTarget:", target => Id);
                msg!((); menu, "addItem:", ns_item => Id);
                release(ns_item);
            }
        }
    }
    DOCK_MENU.with(|m| *m.borrow_mut() = menu);
}

/// Applies a `MenuItemHandle` / `ipc.menu.set` change to the built menu.
/// Returns false when no item has that id.
pub(crate) fn update_menu_item(id: &str, update: &MenuItemUpdate) -> bool {
    let Some((item, tag)) = MENU_ITEMS_BY_ID.with(|m| m.borrow().get(id).copied()) else {
        return false;
    };
    if let Some(title) = &update.title {
        msg!((); item, "setTitle:", ns_string(title) => Id);
    }
    if let Some(enabled) = update.enabled {
        msg!((); item, "setEnabled:", enabled => bool);
        MENU_HANDLERS.with(|h| {
            if let Some(a) = h.borrow_mut().get_mut(&tag) {
                a.enabled = enabled;
            }
        });
    }
    if let Some(checked) = update.checked {
        msg!((); item, "setState:", isize::from(checked) => isize);
    }
    true
}

/// The window with keyboard focus, if it is one of ours.
pub(crate) fn key_window_id() -> Option<WindowId> {
    let key = msg!(Id; ns_app(), "keyWindow");
    let win = if key.is_null() {
        msg!(Id; ns_app(), "mainWindow")
    } else {
        key
    };
    if win.is_null() {
        return None;
    }
    NS_TO_WINDOW_ID.with(|m| m.borrow().get(&(win as usize)).copied())
}

// ---------------------------------------------------------------------------
// Native parented file/message sheet dialogs
// ---------------------------------------------------------------------------

pub(crate) fn run_file_dialog(
    win_id: WindowId,
    kind: FileDialogKind,
    spec: &FileDialogSpec,
    posted: &AtomicBool,
) -> Vec<PathBuf> {
    let app = ns_app();
    let ns_win = window_ns_ids(win_id).map(|(w, _, _)| w).unwrap_or(NIL);
    let panel = match kind {
        FileDialogKind::SaveFile => msg!(Id; class("NSSavePanel"), "savePanel"),
        _ => msg!(Id; class("NSOpenPanel"), "openPanel"),
    };
    if panel.is_null() {
        return Vec::new();
    }
    if let Some(ref title) = spec.title {
        msg!((); panel, "setTitle:", ns_string(title) => Id);
        msg!((); panel, "setMessage:", ns_string(title) => Id);
    }
    if let Some(ref dir) = spec.directory {
        let dir_str = dir.to_string_lossy();
        let url = msg!(Id; class("NSURL"), "fileURLWithPath:", ns_string(&dir_str) => Id);
        msg!((); panel, "setDirectoryURL:", url => Id);
    }
    if let Some(ref name) = spec.default_name {
        msg!((); panel, "setNameFieldStringValue:", ns_string(name) => Id);
    }
    if !spec.filters.is_empty()
        && msg!(bool; panel, "respondsToSelector:", sel("setAllowedFileTypes:") => Sel)
    {
        let mut exts = Vec::new();
        for f in &spec.filters {
            for ext in &f.extensions {
                exts.push(ns_string(ext));
            }
        }
        let arr = msg!(Id; class("NSArray"), "arrayWithObjects:count:", exts.as_ptr() => *const Id, exts.len() => usize);
        msg!((); panel, "setAllowedFileTypes:", arr => Id);
    }

    match kind {
        FileDialogKind::OpenFile => {
            msg!((); panel, "setCanChooseFiles:", true => bool);
            msg!((); panel, "setCanChooseDirectories:", false => bool);
            msg!((); panel, "setAllowsMultipleSelection:", false => bool);
        }
        FileDialogKind::OpenFiles => {
            msg!((); panel, "setCanChooseFiles:", true => bool);
            msg!((); panel, "setCanChooseDirectories:", false => bool);
            msg!((); panel, "setAllowsMultipleSelection:", true => bool);
        }
        FileDialogKind::PickFolder => {
            msg!((); panel, "setCanChooseFiles:", false => bool);
            msg!((); panel, "setCanChooseDirectories:", true => bool);
            msg!((); panel, "setAllowsMultipleSelection:", false => bool);
            msg!((); panel, "setCanCreateDirectories:", true => bool);
        }
        FileDialogKind::SaveFile => {
            msg!((); panel, "setCanCreateDirectories:", true => bool);
        }
    }

    let response = if !ns_win.is_null() && msg!(bool; ns_win, "isVisible") {
        run_with_modal_block(app, posted, |blk| {
            msg!((); panel, "beginSheetModalForWindow:completionHandler:", ns_win => Id, blk => Id);
        })
    } else {
        msg!(isize; panel, "runModal")
    };

    if response != 1 {
        return Vec::new();
    }
    match kind {
        FileDialogKind::SaveFile => {
            let url = msg!(Id; panel, "URL");
            if url.is_null() {
                Vec::new()
            } else {
                let p = rust_string(msg!(Id; url, "path"));
                if p.is_empty() {
                    Vec::new()
                } else {
                    vec![PathBuf::from(p)]
                }
            }
        }
        _ => {
            let urls = msg!(Id; panel, "URLs");
            let count = msg!(usize; urls, "count");
            let mut out = Vec::with_capacity(count);
            for i in 0..count {
                let url = msg!(Id; urls, "objectAtIndex:", i => usize);
                let p = rust_string(msg!(Id; url, "path"));
                if !p.is_empty() {
                    out.push(PathBuf::from(p));
                }
            }
            out
        }
    }
}

pub(crate) fn run_message_dialog(
    win_id: WindowId,
    spec: &MessageDialogSpec,
    posted: &AtomicBool,
) -> MessageResult {
    let app = ns_app();
    let ns_win = window_ns_ids(win_id).map(|(w, _, _)| w).unwrap_or(NIL);
    let alert = msg!(Id; msg!(Id; class("NSAlert"), "alloc"), "init");
    if alert.is_null() {
        return MessageResult::Cancel;
    }
    msg!((); alert, "setMessageText:", ns_string(&spec.title) => Id);
    msg!((); alert, "setInformativeText:", ns_string(&spec.body) => Id);

    let style: usize = match spec.level {
        MessageLevel::Warning => 0,
        MessageLevel::Info => 1,
        MessageLevel::Error => 2,
    };
    msg!((); alert, "setAlertStyle:", style => usize);

    match spec.buttons {
        MessageButtons::Ok => {
            msg!(Id; alert, "addButtonWithTitle:", ns_string("OK") => Id);
        }
        MessageButtons::OkCancel => {
            msg!(Id; alert, "addButtonWithTitle:", ns_string("OK") => Id);
            msg!(Id; alert, "addButtonWithTitle:", ns_string("Cancel") => Id);
        }
        MessageButtons::YesNo => {
            msg!(Id; alert, "addButtonWithTitle:", ns_string("Yes") => Id);
            msg!(Id; alert, "addButtonWithTitle:", ns_string("No") => Id);
        }
        MessageButtons::YesNoCancel => {
            msg!(Id; alert, "addButtonWithTitle:", ns_string("Yes") => Id);
            msg!(Id; alert, "addButtonWithTitle:", ns_string("No") => Id);
            msg!(Id; alert, "addButtonWithTitle:", ns_string("Cancel") => Id);
        }
    }

    let code = if !ns_win.is_null() && msg!(bool; ns_win, "isVisible") {
        run_with_modal_block(app, posted, |blk| {
            msg!((); alert, "beginSheetModalForWindow:completionHandler:", ns_win => Id, blk => Id);
        })
    } else {
        msg!(isize; alert, "runModal")
    };
    release(alert);

    match (spec.buttons, code) {
        (MessageButtons::Ok, _) => MessageResult::Ok,
        (MessageButtons::OkCancel, 1000) => MessageResult::Ok,
        (MessageButtons::OkCancel, _) => MessageResult::Cancel,
        (MessageButtons::YesNo, 1000) => MessageResult::Yes,
        (MessageButtons::YesNo, _) => MessageResult::No,
        (MessageButtons::YesNoCancel, 1000) => MessageResult::Yes,
        (MessageButtons::YesNoCancel, 1001) => MessageResult::No,
        (MessageButtons::YesNoCancel, _) => MessageResult::Cancel,
    }
}

pub(crate) fn set_key_value_bool(obj: Id, key: &str, val: bool) {
    msg!((); obj, "setValue:forKey:", ns_number_bool(val) => Id, ns_string(key) => Id);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_equivalents_for_named_keys() {
        assert_eq!(ns_key_equivalent("K"), "k");
        assert_eq!(ns_key_equivalent("F2"), "\u{F705}");
        assert_eq!(ns_key_equivalent("Backspace"), "\u{8}");
        assert_eq!(ns_key_equivalent("Up"), "\u{F700}");
        assert_eq!(ns_key_equivalent("="), "=");
        assert_eq!(ns_key_equivalent("F99"), "f99");
    }

    #[test]
    fn context_menu_builds_native_items_and_reports_the_choice() {
        let (_, _, _, target_cls) = ensure_classes();
        let target = msg!(Id; msg!(Id; target_cls, "alloc"), "init");
        let items = vec![
            ContextMenuItem::new("rename", "Rename").shortcut(Shortcut::parse("F2").unwrap()),
            ContextMenuItem::separator(),
            ContextMenuItem::new("move", "Move to")
                .submenu(vec![ContextMenuItem::new("a", "Folder A").checked(true)]),
            ContextMenuItem::new("delete", "Delete")
                .shortcut(Shortcut::parse("Cmd+Backspace").unwrap())
                .danger()
                .enabled(false),
        ];
        let choice = Arc::new(Mutex::new(None));
        let mut tags = Vec::new();
        let menu = build_context_menu(&items, target, &choice, &mut tags);

        assert_eq!(msg!(isize; menu, "numberOfItems"), 4);
        let item = |m: Id, i: isize| msg!(Id; m, "itemAtIndex:", i => isize);
        let rename = item(menu, 0);
        assert_eq!(rust_string(msg!(Id; rename, "title")), "Rename");
        assert_eq!(rust_string(msg!(Id; rename, "keyEquivalent")), "\u{F705}");
        assert!(msg!(bool; item(menu, 1), "isSeparatorItem"));
        let sub = msg!(Id; item(menu, 2), "submenu");
        assert!(!sub.is_null());
        assert_eq!(msg!(isize; item(sub, 0), "state"), 1);
        let delete = item(menu, 3);
        assert!(!msg!(bool; delete, "isEnabled"));
        assert!(!msg!(Id; delete, "attributedTitle").is_null());
        assert_eq!(msg!(usize; delete, "keyEquivalentModifierMask"), MOD_COMMAND);
        assert_eq!(tags.len(), 3, "one tag per leaf item");

        // Choosing "Rename": AppKit sends the item's action to its target.
        assert_eq!(msg!(Id; rename, "target"), target);
        msg!((); target, "onCustomMenuItem:", rename => Id);
        assert_eq!(choice.lock().unwrap().as_deref(), Some("rename"));
        release(menu);
        release(target);
    }
}
