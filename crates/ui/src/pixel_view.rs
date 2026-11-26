//! A native view of pixels in a window, over part of its page: for what a webview shouldn't draw itself at native
//! speed, such as a virtual machine's screen, a video or a game. The app places it (`set_frame`, in the page's CSS
//! pixels, which are the window's points, from the content's top left), draws into it from any thread (`draw`, in the
//! view's own pixels, at the window's backing scale), and hears the user's mouse, wheel, keys and focus over it, on
//! the main thread. Keys come as W3C `KeyboardEvent.code` names (`KeyA`, `ShiftLeft`, `ArrowUp`), as a page would get
//! them, whatever the host.
//!
//! ```ignore
//! let view = PixelView::new(&win, |event| println!("{event:?}"))?;
//! view.set_frame(800.0, 48.0, 480.0, 600.0);
//! let scale = view.scale();
//! view.resize((480.0 * scale) as i32, (600.0 * scale) as i32, 0x202020);
//! view.draw(0, 0, 2, 1, vec![0xff0000, 0x00ff00]);
//! ```

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::Result;
use crate::backend::Runtime;
use crate::window::Window;

/// A mouse button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelButton {
    Left,
    Right,
    Middle,
    /// Back, forward and the rest, by their number (3 and up).
    Other(u8),
}

/// What the user did over a pixel view. Places are in its points from its top left.
#[derive(Debug, Clone, PartialEq)]
pub enum PixelEvent {
    /// The pointer moved over it (or anywhere, while a button pressed on it is held).
    PointerMove { x: f64, y: f64 },
    /// A button went down: `count` is 2 for a double click.
    PointerDown { x: f64, y: f64, button: PixelButton, count: u32 },
    PointerUp { x: f64, y: f64, button: PixelButton },
    /// The pointer left it.
    PointerLeave,
    /// Scrolling there: as the host has it, the user's natural scrolling applied, positive `dy` moving the content down
    /// (as a wheel turned away from the user does without it). `precise`: in points, from a trackpad; else in a wheel's
    /// lines.
    Scroll { x: f64, y: f64, dx: f64, dy: f64, precise: bool },
    /// A key, by its W3C `KeyboardEvent.code` (empty for one that has none), down or up, a held key's repeat marked;
    /// with what it types on the host's layout, if anything.
    Key { code: String, down: bool, repeat: bool, text: String },
    /// It took the keyboard, or lost it.
    Focus(bool),
}

/// Drawing waiting for the main thread.
#[derive(Default)]
pub(crate) struct Pending {
    pub resize: Option<(i32, i32, u32)>,
    pub tiles: Vec<(i32, i32, i32, i32, Vec<u32>)>,
}

/// What a view's handle shares with the main thread.
pub(crate) struct Shared {
    pub pending: Mutex<Pending>,
    /// A present is queued on the main thread already.
    pub queued: AtomicBool,
}

/// A handle to a pixel view: cheap to clone, and usable from any thread.
#[derive(Clone)]
pub struct PixelView {
    id: u64,
    runtime: Arc<Runtime>,
    shared: Arc<Shared>,
}

static NEXT: AtomicU64 = AtomicU64::new(1);

impl PixelView {
    /// A new view in `win`, above its page, hidden until it's given a frame. `on_event` is called on the main thread.
    pub fn new(win: &Window, on_event: impl FnMut(PixelEvent) + Send + 'static) -> Result<PixelView> {
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let shared = Arc::new(Shared { pending: Mutex::new(Pending::default()), queued: AtomicBool::new(false) });
        let window = win.id;
        let handler: Box<dyn FnMut(PixelEvent) + Send> = Box::new(on_event);
        win.runtime.call_main_sync(move || {
            #[cfg(target_os = "macos")]
            {
                crate::backend::macos_pixel_view::create(id, window, handler)
            }
            #[cfg(not(target_os = "macos"))]
            {
                let _ = (window, handler);
                Err(crate::Error::Unsupported("pixel views are macOS-only for now"))
            }
        })?;
        Ok(PixelView { id, runtime: win.runtime.clone(), shared })
    }

    fn on_main(&self, f: impl FnOnce(u64) + Send + 'static) {
        let id = self.id;
        if self.runtime.is_main() {
            f(id);
        } else {
            self.runtime.dispatch_on_main(Box::new(move || f(id)));
        }
    }

    /// Where it is: `x`, `y` from the window's content's top left, `width` by `height`, in points (the page's CSS
    /// pixels). Shown once it has a size.
    pub fn set_frame(&self, x: f64, y: f64, width: f64, height: f64) {
        self.on_main(move |id| {
            #[cfg(target_os = "macos")]
            crate::backend::macos_pixel_view::set_frame(id, x, y, width, height);
            #[cfg(not(target_os = "macos"))]
            let _ = (id, x, y, width, height);
        });
    }

    pub fn set_visible(&self, visible: bool) {
        self.on_main(move |id| {
            #[cfg(target_os = "macos")]
            crate::backend::macos_pixel_view::set_visible(id, visible);
            #[cfg(not(target_os = "macos"))]
            let _ = (id, visible);
        });
    }

    /// The window's pixels per point (2.0 on a Retina screen): what to draw at, to be sharp.
    pub fn scale(&self) -> f64 {
        let id = self.id;
        self.runtime.call_main_sync(move || {
            #[cfg(target_os = "macos")]
            {
                crate::backend::macos_pixel_view::scale(id)
            }
            #[cfg(not(target_os = "macos"))]
            {
                let _ = id;
                1.0
            }
        })
    }

    /// Its pixels' size: `width` by `height`, what's drawn kept where it still fits, `fill` (`0x00RRGGBB`) where it's
    /// new. Shown from the view's top left at the window's scale.
    pub fn resize(&self, width: i32, height: i32, fill: u32) {
        lock(&self.shared.pending).resize = Some((width.max(0), height.max(0), fill));
        self.queue();
    }

    /// Pixels (`0x00RRGGBB`, a row after another) at `(x, y)`, `width` by `height`, in the view's pixels. From any
    /// thread: what's drawn before the main thread gets to it is shown at once.
    pub fn draw(&self, x: i32, y: i32, width: i32, height: i32, pixels: Vec<u32>) {
        if width <= 0 || height <= 0 || pixels.len() < (width * height) as usize {
            return;
        }
        lock(&self.shared.pending).tiles.push((x, y, width, height, pixels));
        self.queue();
    }

    fn queue(&self) {
        if self.shared.queued.swap(true, Ordering::AcqRel) {
            return;
        }
        let shared = self.shared.clone();
        self.on_main(move |id| {
            shared.queued.store(false, Ordering::Release);
            let pending = std::mem::take(&mut *lock(&shared.pending));
            #[cfg(target_os = "macos")]
            crate::backend::macos_pixel_view::present(id, pending);
            #[cfg(not(target_os = "macos"))]
            let _ = (id, pending);
        });
    }

    /// The pointer's look over it, by its CSS name: `default`, `pointer`, `text`, `crosshair`, `move`, `grab`,
    /// `grabbing`, `not-allowed`, `ew-resize`, `ns-resize`, ..., or `none`.
    pub fn set_cursor(&self, name: &str) {
        let name = name.to_string();
        self.on_main(move |id| {
            #[cfg(target_os = "macos")]
            crate::backend::macos_pixel_view::set_cursor(id, &name);
            #[cfg(not(target_os = "macos"))]
            let _ = (id, name);
        });
    }

    /// Gives it the keyboard.
    pub fn focus(&self) {
        self.on_main(|id| {
            #[cfg(target_os = "macos")]
            crate::backend::macos_pixel_view::focus(id);
            #[cfg(not(target_os = "macos"))]
            let _ = id;
        });
    }

    /// Takes it out of its window. Its handles do nothing after.
    pub fn close(&self) {
        self.on_main(|id| {
            #[cfg(target_os = "macos")]
            crate::backend::macos_pixel_view::close(id);
            #[cfg(not(target_os = "macos"))]
            let _ = id;
        });
    }
}

impl std::fmt::Debug for PixelView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PixelView").field("id", &self.id).finish()
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// A Mac key's W3C `KeyboardEvent.code`, by its virtual key code (`kVK_*`): the physical key, whatever the layout.
pub fn mac_key_code(key: u16) -> &'static str {
    match key {
        0x00 => "KeyA",
        0x01 => "KeyS",
        0x02 => "KeyD",
        0x03 => "KeyF",
        0x04 => "KeyH",
        0x05 => "KeyG",
        0x06 => "KeyZ",
        0x07 => "KeyX",
        0x08 => "KeyC",
        0x09 => "KeyV",
        0x0A => "IntlBackslash",
        0x0B => "KeyB",
        0x0C => "KeyQ",
        0x0D => "KeyW",
        0x0E => "KeyE",
        0x0F => "KeyR",
        0x10 => "KeyY",
        0x11 => "KeyT",
        0x12 => "Digit1",
        0x13 => "Digit2",
        0x14 => "Digit3",
        0x15 => "Digit4",
        0x16 => "Digit6",
        0x17 => "Digit5",
        0x18 => "Equal",
        0x19 => "Digit9",
        0x1A => "Digit7",
        0x1B => "Minus",
        0x1C => "Digit8",
        0x1D => "Digit0",
        0x1E => "BracketRight",
        0x1F => "KeyO",
        0x20 => "KeyU",
        0x21 => "BracketLeft",
        0x22 => "KeyI",
        0x23 => "KeyP",
        0x24 => "Enter",
        0x25 => "KeyL",
        0x26 => "KeyJ",
        0x27 => "Quote",
        0x28 => "KeyK",
        0x29 => "Semicolon",
        0x2A => "Backslash",
        0x2B => "Comma",
        0x2C => "Slash",
        0x2D => "KeyN",
        0x2E => "KeyM",
        0x2F => "Period",
        0x30 => "Tab",
        0x31 => "Space",
        0x32 => "Backquote",
        0x33 => "Backspace",
        0x35 => "Escape",
        0x36 => "MetaRight",
        0x37 => "MetaLeft",
        0x38 => "ShiftLeft",
        0x39 => "CapsLock",
        0x3A => "AltLeft",
        0x3B => "ControlLeft",
        0x3C => "ShiftRight",
        0x3D => "AltRight",
        0x3E => "ControlRight",
        0x3F => "Fn",
        0x40 => "F17",
        0x41 => "NumpadDecimal",
        0x43 => "NumpadMultiply",
        0x45 => "NumpadAdd",
        0x47 => "NumLock",
        0x4B => "NumpadDivide",
        0x4C => "NumpadEnter",
        0x4E => "NumpadSubtract",
        0x51 => "NumpadEqual",
        0x52 => "Numpad0",
        0x53 => "Numpad1",
        0x54 => "Numpad2",
        0x55 => "Numpad3",
        0x56 => "Numpad4",
        0x57 => "Numpad5",
        0x58 => "Numpad6",
        0x59 => "Numpad7",
        0x5B => "Numpad8",
        0x5C => "Numpad9",
        0x60 => "F5",
        0x61 => "F6",
        0x62 => "F7",
        0x63 => "F3",
        0x64 => "F8",
        0x65 => "F9",
        0x67 => "F11",
        0x69 => "F13",
        0x6B => "F14",
        0x6D => "F10",
        0x6E => "ContextMenu",
        0x6F => "F12",
        0x71 => "F15",
        0x72 => "Insert",
        0x73 => "Home",
        0x74 => "PageUp",
        0x75 => "Delete",
        0x76 => "F4",
        0x77 => "End",
        0x78 => "F2",
        0x79 => "PageDown",
        0x7A => "F1",
        0x7B => "ArrowLeft",
        0x7C => "ArrowRight",
        0x7D => "ArrowDown",
        0x7E => "ArrowUp",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mac_keys_are_named_as_the_web_names_them() {
        assert_eq!(mac_key_code(0x00), "KeyA");
        assert_eq!(mac_key_code(0x1D), "Digit0");
        assert_eq!(mac_key_code(0x24), "Enter");
        assert_eq!(mac_key_code(0x37), "MetaLeft");
        assert_eq!(mac_key_code(0x7E), "ArrowUp");
        assert_eq!(mac_key_code(0x34), "");
        // Every letter once.
        let letters: std::collections::BTreeSet<&str> = (0..0x80).map(mac_key_code).filter(|c| c.starts_with("Key")).collect();
        assert_eq!(letters.len(), 26);
    }
}
