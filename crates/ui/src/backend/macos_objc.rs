//! Pure-Rust Objective-C runtime, Foundation, AppKit, CoreGraphics, and Block ABI helpers.

#![allow(dead_code)]

use std::ffi::{CStr, CString, c_char, c_void};
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub(crate) type Id = *mut c_void;
pub(crate) type Sel = *const c_void;

pub(crate) const NIL: Id = std::ptr::null_mut();

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct Point {
    pub x: f64,
    pub y: f64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct Size {
    pub width: f64,
    pub height: f64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct Rect {
    pub origin: Point,
    pub size: Size,
}

#[link(name = "objc")]
unsafe extern "C" {
    pub(crate) fn objc_getClass(name: *const c_char) -> Id;
    pub(crate) fn sel_registerName(name: *const c_char) -> Sel;
    pub(crate) fn objc_msgSend();
    pub(crate) fn objc_msgSendSuper();
    pub(crate) fn object_getClass(obj: Id) -> Id;
    pub(crate) fn class_getSuperclass(cls: Id) -> Id;
    #[cfg(target_arch = "x86_64")]
    pub(crate) fn objc_msgSend_stret();
    pub(crate) fn objc_autoreleasePoolPush() -> *mut c_void;
    pub(crate) fn objc_autoreleasePoolPop(pool: *mut c_void);
    pub(crate) fn objc_allocateClassPair(superclass: Id, name: *const c_char, extra: usize) -> Id;
    pub(crate) fn objc_registerClassPair(class: Id);
    pub(crate) fn class_addMethod(
        class: Id,
        name: Sel,
        imp: *const c_void,
        types: *const c_char,
    ) -> bool;
    pub(crate) fn objc_getProtocol(name: *const c_char) -> Id;
    pub(crate) fn class_addProtocol(class: Id, protocol: Id) -> bool;
}

#[link(name = "AppKit", kind = "framework")]
unsafe extern "C" {}

#[link(name = "WebKit", kind = "framework")]
unsafe extern "C" {}

#[link(name = "UserNotifications", kind = "framework")]
unsafe extern "C" {}

#[link(name = "Speech", kind = "framework")]
unsafe extern "C" {}

#[link(name = "AVFAudio", kind = "framework")]
unsafe extern "C" {}

#[link(name = "Foundation", kind = "framework")]
unsafe extern "C" {
    pub(crate) static NSDefaultRunLoopMode: Id;
    pub(crate) static NSModalPanelRunLoopMode: Id;
}

#[link(name = "System")]
unsafe extern "C" {
    static _NSConcreteStackBlock: *const c_void;
    fn pthread_main_np() -> i32;
}

#[cfg(target_arch = "aarch64")]
pub(crate) const BOOL_ENC: &str = "B";
#[cfg(target_arch = "x86_64")]
pub(crate) const BOOL_ENC: &str = "c";

pub(crate) fn sel(name: &str) -> Sel {
    let name = CString::new(name).unwrap_or_default();
    // SAFETY: NUL-terminated static or owned selector name registered for process lifetime.
    unsafe { sel_registerName(name.as_ptr()) }
}

pub(crate) fn class(name: &str) -> Id {
    let name = CString::new(name).unwrap_or_default();
    // SAFETY: NUL-terminated Objective-C class name.
    unsafe { objc_getClass(name.as_ptr()) }
}

macro_rules! msg {
    ($ret:ty; $obj:expr, $sel:expr $(, $arg:expr => $t:ty)* $(,)?) => {{
        let receiver: $crate::backend::macos_objc::Id = $obj;
        let selector: $crate::backend::macos_objc::Sel = $crate::backend::macos_objc::sel($sel);
        // SAFETY: `objc_msgSend` is invoked via the exact C ABI signature matching `$sel`.
        unsafe {
            let f: unsafe extern "C" fn($crate::backend::macos_objc::Id, $crate::backend::macos_objc::Sel $(, $t)*) -> $ret =
                std::mem::transmute($crate::backend::macos_objc::objc_msgSend as unsafe extern "C" fn());
            f(receiver, selector $(, $arg)*)
        }
    }};
}
pub(crate) use msg;

pub(crate) fn rect_of(obj: Id, name: &str) -> Rect {
    #[cfg(target_arch = "aarch64")]
    {
        msg!(Rect; obj, name)
    }
    #[cfg(target_arch = "x86_64")]
    {
        let mut rect = Rect::default();
        // SAFETY: On x86_64 macOS, NSRect returns use `objc_msgSend_stret`.
        unsafe {
            let f: unsafe extern "C" fn(*mut Rect, Id, Sel) =
                std::mem::transmute(objc_msgSend_stret as unsafe extern "C" fn());
            f(&mut rect, obj, sel(name));
        }
        rect
    }
}

pub(crate) fn rect_for(obj: Id, name: &str, arg: Rect) -> Rect {
    #[cfg(target_arch = "aarch64")]
    {
        msg!(Rect; obj, name, arg => Rect)
    }
    #[cfg(target_arch = "x86_64")]
    {
        let mut rect = Rect::default();
        // SAFETY: On x86_64 macOS, NSRect returns with 1 NSRect arg use `objc_msgSend_stret`.
        unsafe {
            let f: unsafe extern "C" fn(*mut Rect, Id, Sel, Rect) =
                std::mem::transmute(objc_msgSend_stret as unsafe extern "C" fn());
            f(&mut rect, obj, sel(name), arg);
        }
        rect
    }
}

pub(crate) fn pooled<R>(f: impl FnOnce() -> R) -> R {
    // SAFETY: Balanced push/pop on the current thread.
    let pool = unsafe { objc_autoreleasePoolPush() };
    let out = f();
    // SAFETY: Matches the pool pushed immediately above.
    unsafe { objc_autoreleasePoolPop(pool) };
    out
}

pub(crate) fn is_main_thread() -> bool {
    // SAFETY: Standard Darwin main-thread predicate.
    unsafe { pthread_main_np() != 0 }
}

pub(crate) fn ns_app() -> Id {
    msg!(Id; class("NSApplication"), "sharedApplication")
}

pub(crate) fn ns_string(text: &str) -> Id {
    let clean = CString::new(text.replace('\0', "")).unwrap_or_default();
    msg!(Id; class("NSString"), "stringWithUTF8String:", clean.as_ptr() => *const c_char)
}

pub(crate) fn rust_string(obj: Id) -> String {
    if obj.is_null() {
        return String::new();
    }
    let utf8 = msg!(*const c_char; obj, "UTF8String");
    if utf8.is_null() {
        return String::new();
    }
    // SAFETY: `UTF8String` returns an interior NUL-terminated UTF-8 buffer owned by `obj` inside the active pool.
    unsafe { CStr::from_ptr(utf8) }
        .to_string_lossy()
        .into_owned()
}

pub(crate) fn ns_number_bool(v: bool) -> Id {
    msg!(Id; class("NSNumber"), "numberWithBool:", v => bool)
}

pub(crate) fn retain(obj: Id) -> Id {
    if !obj.is_null() {
        msg!(Id; obj, "retain");
    }
    obj
}

pub(crate) fn release(obj: Id) {
    if !obj.is_null() {
        msg!((); obj, "release");
    }
}

pub(crate) fn define_class(
    name: &str,
    superclass: &str,
    methods: &[(&str, *const c_void, &str)],
    protocols: &[&str],
) -> Id {
    let existing = class(name);
    if !existing.is_null() {
        return existing;
    }
    let cname = CString::new(name).unwrap_or_default();
    // SAFETY: Registers a unique Objective-C subclass before instantiation.
    unsafe {
        let sup = class(superclass);
        if sup.is_null() {
            return NIL;
        }
        let cls = objc_allocateClassPair(sup, cname.as_ptr(), 0);
        if cls.is_null() {
            return NIL;
        }
        for (selector, imp, types) in methods {
            let t = CString::new(*types).unwrap_or_default();
            class_addMethod(cls, sel(selector), *imp, t.as_ptr());
        }
        for proto in protocols {
            let p_name = CString::new(*proto).unwrap_or_default();
            let p = objc_getProtocol(p_name.as_ptr());
            if !p.is_null() {
                class_addProtocol(cls, p);
            }
        }
        objc_registerClassPair(cls);
        cls
    }
}

pub(crate) fn wake_app_loop(posted: &AtomicBool) {
    if posted.swap(true, Ordering::AcqRel) {
        return;
    }
    pooled(|| {
        let event = msg!(Id; class("NSEvent"),
            "otherEventWithType:location:modifierFlags:timestamp:windowNumber:context:subtype:data1:data2:",
            15usize => usize,
            Point::default() => Point,
            0usize => usize,
            0.0f64 => f64,
            0isize => isize,
            NIL => Id,
            0i16 => i16,
            0isize => isize,
            0isize => isize,
        );
        if !event.is_null() {
            msg!((); ns_app(), "postEvent:atStart:", event => Id, false => bool);
        }
    });
}

pub(crate) fn pump_once(
    app: Id,
    wait: Duration,
    mode: Id,
    posted: &AtomicBool,
    mut on_event: impl FnMut(Id) -> bool,
) {
    let mut until =
        msg!(Id; class("NSDate"), "dateWithTimeIntervalSinceNow:", wait.as_secs_f64() => f64);
    loop {
        let ev = msg!(Id; app, "nextEventMatchingMask:untilDate:inMode:dequeue:",
            u64::MAX => u64, until => Id, mode => Id, true => bool);
        if ev.is_null() {
            break;
        }
        posted.store(false, Ordering::Release);
        if !on_event(ev) {
            msg!((); app, "sendEvent:", ev => Id);
        }
        until = msg!(Id; class("NSDate"), "distantPast");
    }
    msg!((); app, "updateWindows");
}

// ---------------------------------------------------------------------------
// Objective-C Block ABI helpers (invoke Cocoa completion blocks + create modal completion blocks)
// ---------------------------------------------------------------------------

#[repr(C)]
struct ObjCBlockHeader {
    isa: *const c_void,
    flags: i32,
    reserved: i32,
    invoke: *const c_void,
    descriptor: *const c_void,
}

/// Invokes a Cocoa-supplied `void (^)(NSInteger)` decision/completion block (`decisionHandler`).
pub(crate) fn call_block_isize(block_ptr: Id, arg: isize) {
    if block_ptr.is_null() {
        return;
    }
    // SAFETY: Caller supplies a valid Objective-C block taking one `NSInteger` (`isize`) argument.
    unsafe {
        let blk = &*(block_ptr as *const ObjCBlockHeader);
        let f: unsafe extern "C" fn(*const ObjCBlockHeader, isize) =
            std::mem::transmute(blk.invoke);
        f(blk, arg);
    }
}

pub(crate) struct CompletionWaiter {
    pub done: AtomicBool,
    pub code: AtomicIsize,
}

impl CompletionWaiter {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            done: AtomicBool::new(false),
            code: AtomicIsize::new(0),
        })
    }
}

#[repr(C)]
struct RustCompletionDescriptor {
    reserved: usize,
    size: usize,
    copy_helper: unsafe extern "C" fn(*mut RustCompletionBlock, *const RustCompletionBlock),
    dispose_helper: unsafe extern "C" fn(*mut RustCompletionBlock),
}

#[repr(C)]
pub(crate) struct RustCompletionBlock {
    isa: *const c_void,
    flags: i32,
    reserved: i32,
    invoke: unsafe extern "C" fn(*mut RustCompletionBlock, isize),
    descriptor: *const RustCompletionDescriptor,
    ctx: *const CompletionWaiter,
}

unsafe extern "C" fn completion_copy(
    dst: *mut RustCompletionBlock,
    src: *const RustCompletionBlock,
) {
    // SAFETY: Both pointers are valid blocks initialized from an `Arc<CompletionWaiter>` raw pointer.
    unsafe {
        let ptr = (*src).ctx;
        Arc::increment_strong_count(ptr);
        (*dst).ctx = ptr;
    }
}

unsafe extern "C" fn completion_dispose(blk: *mut RustCompletionBlock) {
    // SAFETY: Balances the `Arc::into_raw` / `increment_strong_count` ownership held by this block.
    unsafe {
        let ptr = (*blk).ctx;
        if !ptr.is_null() {
            drop(Arc::from_raw(ptr));
            (*blk).ctx = std::ptr::null();
        }
    }
}

unsafe extern "C" fn completion_invoke(blk: *mut RustCompletionBlock, code: isize) {
    // SAFETY: Invoked by AppKit when a sheet completes, pointing to a live `CompletionWaiter`.
    unsafe {
        let waiter = &*(*blk).ctx;
        waiter.code.store(code, Ordering::Release);
        waiter.done.store(true, Ordering::Release);
    }
}

static COMPLETION_DESCRIPTOR: RustCompletionDescriptor = RustCompletionDescriptor {
    reserved: 0,
    size: std::mem::size_of::<RustCompletionBlock>(),
    copy_helper: completion_copy,
    dispose_helper: completion_dispose,
};

/// Invokes a Cocoa-supplied `void (^)(void)` completion block.
pub(crate) fn call_block_void(block_ptr: Id) {
    if block_ptr.is_null() {
        return;
    }
    // SAFETY: Caller supplies a valid Objective-C block taking no arguments.
    unsafe {
        let blk = &*(block_ptr as *const ObjCBlockHeader);
        let f: unsafe extern "C" fn(*const ObjCBlockHeader) = std::mem::transmute(blk.invoke);
        f(blk);
    }
}

type CallbackFn = Mutex<Option<Box<dyn FnOnce(isize) + Send>>>;

#[repr(C)]
struct RustCallbackDescriptor {
    reserved: usize,
    size: usize,
    copy_helper: unsafe extern "C" fn(*mut RustCallbackBlock, *const RustCallbackBlock),
    dispose_helper: unsafe extern "C" fn(*mut RustCallbackBlock),
}

/// A block whose first argument (`BOOL` or `NSInteger`) is handed to a Rust
/// closure, at most once, on whatever thread Cocoa calls it from.
#[repr(C)]
pub(crate) struct RustCallbackBlock {
    isa: *const c_void,
    flags: i32,
    reserved: i32,
    invoke: unsafe extern "C" fn(*mut RustCallbackBlock, isize),
    descriptor: *const RustCallbackDescriptor,
    ctx: *const CallbackFn,
}

unsafe extern "C" fn callback_copy(dst: *mut RustCallbackBlock, src: *const RustCallbackBlock) {
    // SAFETY: Both blocks hold an `Arc<CallbackFn>` raw pointer.
    unsafe {
        let ptr = (*src).ctx;
        Arc::increment_strong_count(ptr);
        (*dst).ctx = ptr;
    }
}

unsafe extern "C" fn callback_dispose(blk: *mut RustCallbackBlock) {
    // SAFETY: Balances the reference this block copy holds.
    unsafe {
        let ptr = (*blk).ctx;
        if !ptr.is_null() {
            drop(Arc::from_raw(ptr));
            (*blk).ctx = std::ptr::null();
        }
    }
}

unsafe extern "C" fn callback_invoke(blk: *mut RustCallbackBlock, arg: isize) {
    // SAFETY: `ctx` points to a live `CallbackFn` while any copy of the block exists.
    let f = unsafe { &*(*blk).ctx }
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take();
    if let Some(f) = f {
        f(arg);
    }
}

static CALLBACK_DESCRIPTOR: RustCallbackDescriptor = RustCallbackDescriptor {
    reserved: 0,
    size: std::mem::size_of::<RustCallbackBlock>(),
    copy_helper: callback_copy,
    dispose_helper: callback_dispose,
};

/// Calls `api` with a block that runs `f(first_arg)` when Cocoa invokes it.
/// Cocoa copies completion handlers it keeps, so the stack block may go away
/// once `api` returns. A `BOOL` argument arrives in the low byte of `first_arg`.
pub(crate) fn with_callback_block(f: impl FnOnce(isize) + Send + 'static, api: impl FnOnce(Id)) {
    let ctx: Arc<CallbackFn> = Arc::new(Mutex::new(Some(Box::new(f))));
    // SAFETY: `_NSConcreteStackBlock` is the standard stack-block isa symbol exported by libSystem.
    let isa = unsafe { _NSConcreteStackBlock };
    let mut blk = RustCallbackBlock {
        isa,
        flags: 1 << 25, // BLOCK_HAS_COPY_DISPOSE
        reserved: 0,
        invoke: callback_invoke,
        descriptor: &CALLBACK_DESCRIPTOR,
        ctx: Arc::into_raw(ctx),
    };
    api((&raw mut blk).cast::<c_void>());
    // SAFETY: Balances `Arc::into_raw` above; heap copies hold their own references.
    unsafe { drop(Arc::from_raw(blk.ctx)) };
}

type RepeatFn = Mutex<Box<dyn FnMut(isize, isize) + Send>>;

#[repr(C)]
struct RustRepeatDescriptor {
    reserved: usize,
    size: usize,
    copy_helper: unsafe extern "C" fn(*mut RustRepeatBlock, *const RustRepeatBlock),
    dispose_helper: unsafe extern "C" fn(*mut RustRepeatBlock),
}

/// A block Cocoa may call many times, with two pointer-sized arguments
/// (e.g. an audio tap's buffer and time, or a result handler's result and error).
#[repr(C)]
pub(crate) struct RustRepeatBlock {
    isa: *const c_void,
    flags: i32,
    reserved: i32,
    invoke: unsafe extern "C" fn(*mut RustRepeatBlock, isize, isize),
    descriptor: *const RustRepeatDescriptor,
    ctx: *const RepeatFn,
}

unsafe extern "C" fn repeat_copy(dst: *mut RustRepeatBlock, src: *const RustRepeatBlock) {
    // SAFETY: Both blocks hold an `Arc<RepeatFn>` raw pointer.
    unsafe {
        let ptr = (*src).ctx;
        Arc::increment_strong_count(ptr);
        (*dst).ctx = ptr;
    }
}

unsafe extern "C" fn repeat_dispose(blk: *mut RustRepeatBlock) {
    // SAFETY: Balances the reference this block copy holds.
    unsafe {
        let ptr = (*blk).ctx;
        if !ptr.is_null() {
            drop(Arc::from_raw(ptr));
            (*blk).ctx = std::ptr::null();
        }
    }
}

unsafe extern "C" fn repeat_invoke(blk: *mut RustRepeatBlock, a: isize, b: isize) {
    // SAFETY: `ctx` is live while any copy of the block exists.
    let f = unsafe { &*(*blk).ctx };
    let mut f = f.lock().unwrap_or_else(|e| e.into_inner());
    f(a, b);
}

static REPEAT_DESCRIPTOR: RustRepeatDescriptor = RustRepeatDescriptor {
    reserved: 0,
    size: std::mem::size_of::<RustRepeatBlock>(),
    copy_helper: repeat_copy,
    dispose_helper: repeat_dispose,
};

/// Calls `api` with a block that runs `f(a, b)` each time Cocoa invokes it.
pub(crate) fn with_repeat_block(
    f: impl FnMut(isize, isize) + Send + 'static,
    api: impl FnOnce(Id),
) {
    let ctx: Arc<RepeatFn> = Arc::new(Mutex::new(Box::new(f)));
    // SAFETY: `_NSConcreteStackBlock` is the standard stack-block isa symbol exported by libSystem.
    let isa = unsafe { _NSConcreteStackBlock };
    let mut blk = RustRepeatBlock {
        isa,
        flags: 1 << 25, // BLOCK_HAS_COPY_DISPOSE
        reserved: 0,
        invoke: repeat_invoke,
        descriptor: &REPEAT_DESCRIPTOR,
        ctx: Arc::into_raw(ctx),
    };
    api((&raw mut blk).cast::<c_void>());
    // SAFETY: Balances `Arc::into_raw` above; heap copies hold their own references.
    unsafe { drop(Arc::from_raw(blk.ctx)) };
}

/// Runs `f` with a stack-allocated Objective-C `void (^)(NSModalResponse)` completion block,
/// pumping the AppKit modal run-loop until completion fires.
pub(crate) fn run_with_modal_block(app: Id, posted: &AtomicBool, f: impl FnOnce(Id)) -> isize {
    let waiter = CompletionWaiter::new();
    // SAFETY: `_NSConcreteStackBlock` is the standard stack-block isa symbol exported by libSystem.
    let isa = unsafe { _NSConcreteStackBlock };
    let mut blk = RustCompletionBlock {
        isa,
        flags: 1 << 25, // BLOCK_HAS_COPY_DISPOSE
        reserved: 0,
        invoke: completion_invoke,
        descriptor: &COMPLETION_DESCRIPTOR,
        ctx: Arc::into_raw(waiter.clone()),
    };
    let blk_ptr = (&raw mut blk).cast::<c_void>();
    f(blk_ptr);

    // SAFETY: `NSDefaultRunLoopMode` handles sheet/window-modal Cocoa events cleanly.
    let mode = unsafe { NSDefaultRunLoopMode };
    while !waiter.done.load(Ordering::Acquire) {
        pooled(|| {
            pump_once(app, Duration::from_millis(16), mode, posted, |_| false);
        });
    }
    // Balance our initial stack block `Arc::into_raw`. (Copied heap blocks balance themselves via dispose_helper.)
    // SAFETY: `blk.ctx` was created by `Arc::into_raw` above and the stack block will not be reused after return.
    unsafe { drop(Arc::from_raw(blk.ctx)) };
    waiter.code.load(Ordering::Acquire)
}
