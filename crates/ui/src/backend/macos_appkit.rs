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
use crate::menu::{Menu, MenuItem, Mod, Role};
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
    static MENU_HANDLERS: RefCell<HashMap<isize, Arc<dyn Fn() + Send + Sync>>> = RefCell::new(HashMap::new());
    static NEXT_MENU_TAG: RefCell<isize> = const { RefCell::new(1000) };
    static MENU_TARGET: RefCell<Id> = const { RefCell::new(NIL) };
    static LAST_LEFT_DOWN: RefCell<HashMap<usize, Id>> = RefCell::new(HashMap::new());
    static WINDOW_RECORDS: RefCell<HashMap<WindowId, NativeWindowRecord>> = RefCell::new(HashMap::new());
    static NS_TO_WINDOW_ID: RefCell<HashMap<usize, WindowId>> = RefCell::new(HashMap::new());
    static ACTIVE_DELEGATES: RefCell<HashMap<WindowId, WindowCallbacks>> = RefCell::new(HashMap::new());
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
                    .map(|rec| (id, rec.title_bar, rec.content_view))
            })
        })
    });
    if let Some((id, title_bar, content_view)) = entry {
        if let TitleBar::Inset {
            height,
            traffic_lights,
        } = title_bar
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
            .and_then(|id| WINDOW_RECORDS.with(|r| r.borrow().get(&id).map(|rec| rec.title_bar)))
    });
    if let Some(TitleBar::Inset {
        height,
        traffic_lights,
    }) = title_bar
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
    let cb = MENU_HANDLERS.with(|h| h.borrow().get(&tag).cloned());
    if let Some(cb) = cb {
        cb();
    }
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
            &[(
                "onCustomMenuItem:",
                on_custom_menu_item as *const c_void,
                "v@:@",
            )],
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

    if spec.position.is_none() {
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
    {
        position_traffic_lights(ns_win, height, traffic_lights);
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

pub(crate) fn set_app_menu(menu: &Menu) {
    let (_, _, _, target_cls) = ensure_classes();
    MENU_HANDLERS.with(|h| h.borrow_mut().clear());
    let menu_target = MENU_TARGET.with(|slot| {
        let mut s = slot.borrow_mut();
        if s.is_null() {
            *s = retain(msg!(Id; msg!(Id; target_cls, "alloc"), "init"));
        }
        *s
    });

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
                    title,
                    shortcut,
                    enabled,
                    role,
                    handler,
                    ..
                } => {
                    let key = shortcut
                        .as_ref()
                        .map(|s| s.key.to_ascii_lowercase())
                        .unwrap_or_default();
                    let action_sel = match role {
                        Some(Role::About) => "orderFrontStandardAboutPanel:",
                        Some(Role::Hide) => "hide:",
                        Some(Role::HideOthers) => "hideOtherApplications:",
                        Some(Role::ShowAll) => "unhideAllApplications:",
                        Some(Role::Quit) => "terminate:",
                        Some(Role::Undo) => "undo:",
                        Some(Role::Redo) => "redo:",
                        Some(Role::Cut) => "cut:",
                        Some(Role::Copy) => "copy:",
                        Some(Role::Paste) => "paste:",
                        Some(Role::SelectAll) => "selectAll:",
                        Some(Role::Minimize) => "performMiniaturize:",
                        Some(Role::Zoom) => "performZoom:",
                        Some(Role::CloseWindow) => "performClose:",
                        Some(Role::Fullscreen) => "toggleFullScreen:",
                        None => "onCustomMenuItem:",
                    };
                    let ns_item = msg!(Id; msg!(Id; class("NSMenuItem"), "alloc"), "initWithTitle:action:keyEquivalent:",
                        ns_string(title) => Id, sel(action_sel) => Sel, ns_string(&key) => Id);

                    if let Some(sc) = shortcut {
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
                        msg!((); ns_item, "setKeyEquivalentModifierMask:", mask => usize);
                    }
                    msg!((); ns_item, "setEnabled:", *enabled => bool);

                    if role.is_none()
                        && let Some(cb) = handler
                    {
                        let tag = NEXT_MENU_TAG.with(|t| {
                            let mut cur = t.borrow_mut();
                            let v = *cur;
                            *cur += 1;
                            v
                        });
                        MENU_HANDLERS.with(|h| h.borrow_mut().insert(tag, cb.clone()));
                        msg!((); ns_item, "setTag:", tag => isize);
                        msg!((); ns_item, "setTarget:", menu_target => Id);
                    }
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
        release(sub_menu);
        release(top_item);
    }
    msg!((); ns_app(), "setMainMenu:", main_menu => Id);
    release(main_menu);
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
