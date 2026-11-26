//! `PixelView` on the Mac: an `NSView` of our own above the window's page, layer-backed, its layer's contents an image
//! of its pixels made again at each present (what changed copied in first), at the window's backing scale. Mouse,
//! wheel, keys and focus over it go to the app's handler on the main thread.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::OnceLock;

use crate::backend::macos_appkit::window_ns_ids;
use crate::backend::macos_objc::{BOOL_ENC, Id, NIL, Point, Rect, Sel, Size, class, define_class, msg, rect_of, rust_string};
use crate::pixel_view::{PixelButton, PixelEvent, Pending, mac_key_code};
use crate::window::WindowId;
use crate::{Error, Result};

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGColorSpaceCreateDeviceRGB() -> *mut c_void;
    fn CGDataProviderCreateWithCFData(data: *const c_void) -> *mut c_void;
    fn CGDataProviderRelease(provider: *mut c_void);
    fn CGImageCreate(
        width: usize,
        height: usize,
        bits_per_component: usize,
        bits_per_pixel: usize,
        bytes_per_row: usize,
        space: *mut c_void,
        info: u32,
        provider: *mut c_void,
        decode: *const f64,
        interpolate: bool,
        intent: i32,
    ) -> *mut c_void;
    fn CGImageRelease(image: *mut c_void);
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFDataCreate(allocator: *const c_void, bytes: *const u8, length: isize) -> *const c_void;
    fn CFRelease(cf: *const c_void);
}

#[link(name = "QuartzCore", kind = "framework")]
unsafe extern "C" {}

/// `0x00RRGGBB` in little-endian memory: BGRX, Core Graphics' `kCGImageAlphaNoneSkipFirst | kCGBitmapByteOrder32Little`.
const XRGB_LITTLE: u32 = 6 | (2 << 12);

/// One view's state, on the main thread.
struct View {
    ns_view: Id,
    layer: Id,
    pixels: Vec<u32>,
    width: i32,
    height: i32,
    cursor: Id,
    inside: bool,
    handler: Option<Box<dyn FnMut(PixelEvent) + Send>>,
}

thread_local! {
    static VIEWS: RefCell<HashMap<u64, View>> = RefCell::new(HashMap::new());
    static BY_NS: RefCell<HashMap<usize, u64>> = RefCell::new(HashMap::new());
}

fn space() -> *mut c_void {
    static SPACE: OnceLock<usize> = OnceLock::new();
    // SAFETY: a colour space kept for the process's life.
    *SPACE.get_or_init(|| unsafe { CGColorSpaceCreateDeviceRGB() } as usize) as *mut c_void
}

/// Calls the handler of the view that's `this`, outside the views' borrow, so it may use its handle.
fn emit(this: Id, event: PixelEvent) {
    let Some(id) = BY_NS.with(|m| m.borrow().get(&(this as usize)).copied()) else { return };
    let Some(mut handler) = VIEWS.with(|v| v.borrow_mut().get_mut(&id).and_then(|view| view.handler.take())) else { return };
    handler(event);
    VIEWS.with(|v| {
        if let Some(view) = v.borrow_mut().get_mut(&id) {
            view.handler.get_or_insert(handler);
        }
    });
}

/// Where an event happened, in the view's points from its top left (the view is flipped).
fn place(this: Id, event: Id) -> (f64, f64) {
    let in_window = msg!(Point; event, "locationInWindow");
    let p = msg!(Point; this, "convertPoint:fromView:", in_window => Point, NIL => Id);
    (p.x, p.y)
}

fn button_of(event: Id) -> PixelButton {
    match msg!(isize; event, "buttonNumber") {
        0 => PixelButton::Left,
        1 => PixelButton::Right,
        2 => PixelButton::Middle,
        n => PixelButton::Other(n.clamp(3, 255) as u8),
    }
}

extern "C" fn yes(_: Id, _: Sel) -> bool {
    true
}

extern "C" fn yes_for(_: Id, _: Sel, _: Id) -> bool {
    true
}

extern "C" fn mouse_down(this: Id, _: Sel, event: Id) {
    let window = msg!(Id; this, "window");
    msg!(bool; window, "makeFirstResponder:", this => Id);
    let (x, y) = place(this, event);
    let count = msg!(isize; event, "clickCount").max(1) as u32;
    emit(this, PixelEvent::PointerDown { x, y, button: button_of(event), count });
}

extern "C" fn mouse_up(this: Id, _: Sel, event: Id) {
    let (x, y) = place(this, event);
    emit(this, PixelEvent::PointerUp { x, y, button: button_of(event) });
}

extern "C" fn mouse_moved(this: Id, _: Sel, event: Id) {
    let (x, y) = place(this, event);
    emit(this, PixelEvent::PointerMove { x, y });
}

extern "C" fn mouse_entered(this: Id, _: Sel, _event: Id) {
    let cursor = VIEWS.with(|v| {
        let mut v = v.borrow_mut();
        let id = BY_NS.with(|m| m.borrow().get(&(this as usize)).copied())?;
        let view = v.get_mut(&id)?;
        view.inside = true;
        Some(view.cursor)
    });
    show_cursor(cursor);
}

extern "C" fn mouse_exited(this: Id, _: Sel, _event: Id) {
    VIEWS.with(|v| {
        if let Some(id) = BY_NS.with(|m| m.borrow().get(&(this as usize)).copied())
            && let Some(view) = v.borrow_mut().get_mut(&id)
        {
            view.inside = false;
        }
    });
    show_cursor(Some(msg!(Id; class("NSCursor"), "arrowCursor")));
    emit(this, PixelEvent::PointerLeave);
}

extern "C" fn cursor_update(this: Id, _: Sel, _event: Id) {
    let cursor = VIEWS.with(|v| BY_NS.with(|m| m.borrow().get(&(this as usize)).copied()).and_then(|id| v.borrow().get(&id).map(|view| view.cursor)));
    show_cursor(cursor);
}

fn show_cursor(cursor: Option<Id>) {
    match cursor {
        Some(c) if !c.is_null() => {
            msg!((); class("NSCursor"), "unhide");
            msg!((); c, "set");
        }
        // `none`: hidden while it's over the view.
        Some(_) => msg!((); class("NSCursor"), "hide"),
        None => {}
    }
}

extern "C" fn scroll_wheel(this: Id, _: Sel, event: Id) {
    let (x, y) = place(this, event);
    let dx = msg!(f64; event, "scrollingDeltaX");
    let dy = msg!(f64; event, "scrollingDeltaY");
    let precise = msg!(bool; event, "hasPreciseScrollingDeltas");
    if dx != 0.0 || dy != 0.0 {
        emit(this, PixelEvent::Scroll { x, y, dx, dy, precise });
    }
}

fn key(this: Id, event: Id, down: bool) {
    let code = mac_key_code(msg!(u16; event, "keyCode")).to_string();
    let repeat = down && msg!(bool; event, "isARepeat");
    let text = if down { rust_string(msg!(Id; event, "characters")) } else { String::new() };
    emit(this, PixelEvent::Key { code, down, repeat, text });
}

extern "C" fn key_down(this: Id, _: Sel, event: Id) {
    key(this, event, true);
}

extern "C" fn key_up(this: Id, _: Sel, event: Id) {
    key(this, event, false);
}

/// A modifier key went down or up: which, from the device's own bits for each side.
extern "C" fn flags_changed(this: Id, _: Sel, event: Id) {
    let key = msg!(u16; event, "keyCode");
    let flags = msg!(usize; event, "modifierFlags");
    // NX_DEVICE*KEYMASK, and caps lock's own flag.
    let bit = match key {
        0x38 => 0x02,
        0x3C => 0x04,
        0x3B => 0x01,
        0x3E => 0x2000,
        0x3A => 0x20,
        0x3D => 0x40,
        0x37 => 0x08,
        0x36 => 0x10,
        0x39 => 1 << 16,
        _ => return,
    };
    let code = mac_key_code(key).to_string();
    emit(this, PixelEvent::Key { code, down: flags & bit != 0, repeat: false, text: String::new() });
}

extern "C" fn became_first_responder(this: Id, _: Sel) -> bool {
    emit(this, PixelEvent::Focus(true));
    true
}

extern "C" fn resigned_first_responder(this: Id, _: Sel) -> bool {
    emit(this, PixelEvent::Focus(false));
    true
}

fn view_class() -> Id {
    static CLASS: OnceLock<usize> = OnceLock::new();
    *CLASS.get_or_init(|| {
        let event = "v@:@";
        let yes_types = format!("{BOOL_ENC}@:");
        let yes_for_types = format!("{BOOL_ENC}@:@");
        define_class(
            "UiPixelView",
            "NSView",
            &[
                ("isFlipped", yes as *const c_void, &yes_types),
                ("acceptsFirstResponder", yes as *const c_void, &yes_types),
                ("acceptsFirstMouse:", yes_for as *const c_void, &yes_for_types),
                ("becomeFirstResponder", became_first_responder as *const c_void, &yes_types),
                ("resignFirstResponder", resigned_first_responder as *const c_void, &yes_types),
                ("mouseDown:", mouse_down as *const c_void, event),
                ("rightMouseDown:", mouse_down as *const c_void, event),
                ("otherMouseDown:", mouse_down as *const c_void, event),
                ("mouseUp:", mouse_up as *const c_void, event),
                ("rightMouseUp:", mouse_up as *const c_void, event),
                ("otherMouseUp:", mouse_up as *const c_void, event),
                ("mouseMoved:", mouse_moved as *const c_void, event),
                ("mouseDragged:", mouse_moved as *const c_void, event),
                ("rightMouseDragged:", mouse_moved as *const c_void, event),
                ("otherMouseDragged:", mouse_moved as *const c_void, event),
                ("mouseEntered:", mouse_entered as *const c_void, event),
                ("mouseExited:", mouse_exited as *const c_void, event),
                ("cursorUpdate:", cursor_update as *const c_void, event),
                ("scrollWheel:", scroll_wheel as *const c_void, event),
                ("keyDown:", key_down as *const c_void, event),
                ("keyUp:", key_up as *const c_void, event),
                ("flagsChanged:", flags_changed as *const c_void, event),
            ],
            &[],
        ) as usize
    }) as Id
}

pub(crate) fn create(id: u64, window: WindowId, handler: Box<dyn FnMut(PixelEvent) + Send>) -> Result<()> {
    let Some((ns_window, content_view, _)) = window_ns_ids(window) else {
        return Err(Error::InvalidState("the window is closed".into()));
    };
    let cls = view_class();
    if cls.is_null() {
        return Err(Error::Platform("couldn't make the pixel view's class".into()));
    }
    let zero = Rect { origin: Point { x: 0.0, y: 0.0 }, size: Size { width: 0.0, height: 0.0 } };
    let ns_view = msg!(Id; msg!(Id; cls, "alloc"), "initWithFrame:", zero => Rect);
    msg!((); ns_view, "setWantsLayer:", true => bool);
    let layer = msg!(Id; ns_view, "layer");
    // Pixels at the window's scale, from the top left, never stretched while a resize catches up.
    msg!((); layer, "setContentsGravity:", ns_string("topLeft") => Id);
    let scale = msg!(f64; ns_window, "backingScaleFactor");
    msg!((); layer, "setContentsScale:", scale => f64);
    msg!((); layer, "setMasksToBounds:", true => bool);
    msg!((); ns_view, "setHidden:", true => bool);
    // Mouse moves, entering and leaving, and the cursor, wherever its visible part is.
    let options: usize = 0x01 | 0x02 | 0x04 | 0x80 | 0x200;
    let area = msg!(Id; msg!(Id; class("NSTrackingArea"), "alloc"), "initWithRect:options:owner:userInfo:", zero => Rect, options => usize, ns_view => Id, NIL => Id);
    msg!((); ns_view, "addTrackingArea:", area => Id);
    msg!((); area, "release");
    // Above the page (and anything added before it).
    msg!((); content_view, "addSubview:positioned:relativeTo:", ns_view => Id, 1isize => isize, NIL => Id);
    BY_NS.with(|m| m.borrow_mut().insert(ns_view as usize, id));
    VIEWS.with(|v| {
        v.borrow_mut().insert(
            id,
            View { ns_view, layer, pixels: Vec::new(), width: 0, height: 0, cursor: msg!(Id; class("NSCursor"), "arrowCursor"), inside: false, handler: Some(handler) },
        )
    });
    Ok(())
}

fn ns_string(s: &str) -> Id {
    crate::backend::macos_objc::ns_string(s)
}

fn with_view<R>(id: u64, f: impl FnOnce(&mut View) -> R) -> Option<R> {
    VIEWS.with(|v| v.borrow_mut().get_mut(&id).map(f))
}

/// A view's `NSView` and layer, for AppKit calls made outside the views' borrow: AppKit may call back into the view
/// (focus, the cursor, the pointer entering) while they run.
fn handles(id: u64) -> Option<(Id, Id)> {
    with_view(id, |view| (view.ns_view, view.layer))
}

pub(crate) fn set_frame(id: u64, x: f64, y: f64, width: f64, height: f64) {
    let Some((ns_view, layer)) = handles(id) else { return };
    let parent = msg!(Id; ns_view, "superview");
    if parent.is_null() {
        return;
    }
    // The content view isn't flipped: its origin is at the bottom.
    let bounds = rect_of(parent, "bounds");
    let flipped = msg!(bool; parent, "isFlipped");
    let top = if flipped { y } else { bounds.size.height - y - height };
    let frame = Rect { origin: Point { x, y: top }, size: Size { width: width.max(0.0), height: height.max(0.0) } };
    msg!((); ns_view, "setFrame:", frame => Rect);
    let window = msg!(Id; ns_view, "window");
    if !window.is_null() {
        let scale = msg!(f64; window, "backingScaleFactor");
        msg!((); layer, "setContentsScale:", scale => f64);
    }
    let hidden = width <= 0.0 || height <= 0.0;
    msg!((); ns_view, "setHidden:", hidden => bool);
}

pub(crate) fn set_visible(id: u64, visible: bool) {
    if let Some((ns_view, _)) = handles(id) {
        msg!((); ns_view, "setHidden:", !visible => bool);
    }
}

pub(crate) fn scale(id: u64) -> f64 {
    let Some((ns_view, _)) = handles(id) else { return 1.0 };
    let window = msg!(Id; ns_view, "window");
    if window.is_null() { 1.0 } else { msg!(f64; window, "backingScaleFactor") }
}

pub(crate) fn present(id: u64, pending: Pending) {
    with_view(id, |view| {
        if let Some((width, height, fill)) = pending.resize
            && (width, height) != (view.width, view.height)
        {
            let mut next = vec![fill; (width.max(0) * height.max(0)) as usize];
            for y in 0..view.height.min(height) {
                let n = view.width.min(width) as usize;
                let (from, to) = ((y * view.width) as usize, (y * width) as usize);
                next[to..to + n].copy_from_slice(&view.pixels[from..from + n]);
            }
            view.pixels = next;
            view.width = width;
            view.height = height;
        }
        for (x, y, w, h, pixels) in pending.tiles {
            // What of it is inside.
            let (left, top) = (x.max(0), y.max(0));
            let (right, bottom) = ((x + w).min(view.width), (y + h).min(view.height));
            if left >= right || top >= bottom {
                continue;
            }
            let n = (right - left) as usize;
            for row in top..bottom {
                let from = ((row - y) * w + (left - x)) as usize;
                let to = (row * view.width + left) as usize;
                view.pixels[to..to + n].copy_from_slice(&pixels[from..from + n]);
            }
        }
        show(view);
    });
}

/// The pixels as the layer's contents: an image of a copy of them, so they never change under Core Animation.
fn show(view: &View) {
    if view.width <= 0 || view.height <= 0 {
        return;
    }
    let (w, h) = (view.width as usize, view.height as usize);
    // SAFETY: the pixels copied into a CFData and an image made over it, which holds what it needs.
    let image = unsafe {
        let data = CFDataCreate(std::ptr::null(), view.pixels.as_ptr().cast(), (w * h * 4) as isize);
        let provider = CGDataProviderCreateWithCFData(data);
        let image = CGImageCreate(w, h, 8, 32, w * 4, space(), XRGB_LITTLE, provider, std::ptr::null(), false, 0);
        CGDataProviderRelease(provider);
        CFRelease(data);
        image
    };
    if image.is_null() {
        return;
    }
    msg!((); class("CATransaction"), "begin");
    msg!((); class("CATransaction"), "setDisableActions:", true => bool);
    msg!((); view.layer, "setContents:", image as Id => Id);
    msg!((); class("CATransaction"), "commit");
    // SAFETY: made above, released once; the layer holds its own reference.
    unsafe { CGImageRelease(image) };
}

/// A CSS cursor name's `NSCursor`, or nil for `none`.
fn cursor_named(name: &str) -> Id {
    let selector = match name {
        "none" => return NIL,
        "pointer" => "pointingHandCursor",
        "text" => "IBeamCursor",
        "vertical-text" => "IBeamCursorForVerticalLayout",
        "crosshair" | "cell" => "crosshairCursor",
        "grab" | "move" | "all-scroll" => "openHandCursor",
        "grabbing" => "closedHandCursor",
        "not-allowed" | "no-drop" => "operationNotAllowedCursor",
        "copy" => "dragCopyCursor",
        "alias" => "dragLinkCursor",
        "context-menu" => "contextualMenuCursor",
        "ew-resize" | "col-resize" | "e-resize" | "w-resize" => "resizeLeftRightCursor",
        "ns-resize" | "row-resize" | "n-resize" | "s-resize" => "resizeUpDownCursor",
        _ => "arrowCursor",
    };
    msg!(Id; class("NSCursor"), selector)
}

pub(crate) fn set_cursor(id: u64, name: &str) {
    let cursor = cursor_named(name);
    let inside = with_view(id, |view| {
        view.cursor = cursor;
        view.inside
    });
    if inside == Some(true) {
        show_cursor(Some(cursor));
    }
}

pub(crate) fn focus(id: u64) {
    let Some((ns_view, _)) = handles(id) else { return };
    let window = msg!(Id; ns_view, "window");
    if !window.is_null() {
        msg!(bool; window, "makeFirstResponder:", ns_view => Id);
    }
}

pub(crate) fn close(id: u64) {
    let Some(view) = VIEWS.with(|v| v.borrow_mut().remove(&id)) else { return };
    BY_NS.with(|m| m.borrow_mut().remove(&(view.ns_view as usize)));
    if view.inside {
        show_cursor(Some(msg!(Id; class("NSCursor"), "arrowCursor")));
    }
    msg!((); view.ns_view, "removeFromSuperview");
    msg!((); view.ns_view, "release");
}
