//! Chromium Embedded Framework (CEF) host backend for macOS (`--features cef`), Windows, and Linux.
//!
//! Uses `ui-cef-sys` C-API bindings directly (zero C++/CMake build, multi-process Chromium via
//! auto-staged `ui Helper*.app` subprocesses) and embeds child Alloy views inside `NSWindow`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::{CString, c_char, c_int, c_void};
use std::path::PathBuf;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;

#[cfg(target_os = "macos")]
use crate::backend::macos_appkit;
#[cfg(target_os = "macos")]
use crate::backend::macos_objc::{self, Id, msg, rect_of};
use crate::backend::{
    IpcEndpoint, IpcReplyTarget, IpcResponder, IpcRouteAction, ipc_binary_response,
};
use crate::event::WebViewEvent;
use crate::ipc::{CallSpec, Value, encode_base64, encode_rpc_response};
use crate::js_shim::SHIM_JS;
#[cfg(not(target_os = "macos"))]
use crate::platform;
use crate::webview::{
    AssetHandler, IPC_SCHEME, InitialContent, LoadState, Nav, NavHandler, NavRequest, Profile,
    Request, Response, WebViewId, WebViewSpec,
};
#[cfg(target_os = "macos")]
use crate::window::Backdrop;
use crate::window::WindowId;
use crate::{Error, Result};

use ui_cef_sys::{
    CEF_SCHEME_OPTION_CORS_ENABLED, CEF_SCHEME_OPTION_CSP_BYPASSING,
    CEF_SCHEME_OPTION_FETCH_ENABLED, CEF_SCHEME_OPTION_SECURE, CEF_SCHEME_OPTION_STANDARD, CefApi,
    cef_add_ref, cef_app_t, cef_base_ref_counted_t, cef_browser_host_t,
    cef_browser_process_handler_t, cef_browser_settings_t, cef_browser_t, cef_callback_t,
    cef_client_t, cef_command_line_t, cef_cookie_manager_t, cef_display_handler_t, cef_frame_t,
    cef_life_span_handler_t, cef_load_handler_t, cef_log_severity_t, cef_main_args_t,
    cef_post_data_element_t, cef_post_data_t, cef_postdataelement_type_t, cef_process_id_t,
    cef_process_message_t, cef_rect_t, cef_release, cef_request_context_settings_t,
    cef_request_context_t, cef_request_handler_t, cef_request_t, cef_resource_handler_t,
    cef_resource_request_handler_t, cef_response_t, cef_return_value_t, cef_runtime_style_t,
    cef_scheme_handler_factory_t, cef_scheme_registrar_t, cef_settings_t, cef_string_t,
    cef_window_info_t, cef_window_open_disposition_t,
};

pub(crate) enum CefNotification {
    Event(WebViewId, WebViewEvent),
}

// ---------------------------------------------------------------------------
// Generic ref-counted C-API object wrapper
// ---------------------------------------------------------------------------

#[repr(C)]
struct CefObj<T, V> {
    c_struct: T,
    rc: AtomicUsize,
    data: V,
}

unsafe extern "C" fn cef_obj_add_ref<T, V>(self_ptr: *mut cef_base_ref_counted_t) {
    // SAFETY: `self_ptr` sits at offset 0 of a live `CefObj<T, V>` whose `rc` tracks strong refs.
    unsafe {
        let obj = self_ptr.cast::<CefObj<T, V>>();
        (*obj).rc.fetch_add(1, Ordering::Relaxed);
    }
}

unsafe extern "C" fn cef_obj_release<T, V>(self_ptr: *mut cef_base_ref_counted_t) -> c_int {
    // SAFETY: Balances a prior `alloc_cef_obj` (`rc=1`) or `cef_obj_add_ref`, freeing when reaching 0.
    unsafe {
        let obj = self_ptr.cast::<CefObj<T, V>>();
        if (*obj).rc.fetch_sub(1, Ordering::AcqRel) == 1 {
            drop(Box::from_raw(obj));
            1
        } else {
            0
        }
    }
}

unsafe extern "C" fn cef_obj_has_one_ref<T, V>(self_ptr: *mut cef_base_ref_counted_t) -> c_int {
    // SAFETY: `self_ptr` points to a live `CefObj<T, V>`.
    let obj = self_ptr.cast::<CefObj<T, V>>();
    // SAFETY: Valid initialized `AtomicUsize`.
    let rc = unsafe { (*obj).rc.load(Ordering::Acquire) };
    c_int::from(rc == 1)
}

unsafe extern "C" fn cef_obj_has_at_least_one_ref<T, V>(
    self_ptr: *mut cef_base_ref_counted_t,
) -> c_int {
    // SAFETY: `self_ptr` points to a live `CefObj<T, V>`.
    let obj = self_ptr.cast::<CefObj<T, V>>();
    // SAFETY: Valid initialized `AtomicUsize`.
    let rc = unsafe { (*obj).rc.load(Ordering::Acquire) };
    c_int::from(rc >= 1)
}

fn cef_base_vtbl<T, V>() -> cef_base_ref_counted_t {
    cef_base_ref_counted_t {
        size: size_of::<T>(),
        add_ref: Some(cef_obj_add_ref::<T, V>),
        release: Some(cef_obj_release::<T, V>),
        has_one_ref: Some(cef_obj_has_one_ref::<T, V>),
        has_at_least_one_ref: Some(cef_obj_has_at_least_one_ref::<T, V>),
    }
}

fn alloc_cef_obj<T, V>(data: V, init: impl FnOnce(&mut T, cef_base_ref_counted_t)) -> *mut T {
    // SAFETY: All CEF C-API callback structs are POD structs where zeroed fields mean NULL function pointers.
    let mut c_struct: T = unsafe { std::mem::zeroed() };
    init(&mut c_struct, cef_base_vtbl::<T, V>());
    let obj = Box::new(CefObj {
        c_struct,
        rc: AtomicUsize::new(1),
        data,
    });
    Box::into_raw(obj).cast::<T>()
}

/// Accesses the immutable payload `V` stored inside `CefObj<T, V>`.
///
/// # Safety
/// `ptr` must be a valid non-null `*mut T` allocated via `alloc_cef_obj::<T, V>`.
unsafe fn cef_obj_data<'a, T, V>(ptr: *mut T) -> &'a V {
    // SAFETY: Caller guarantees `ptr` points to `c_struct` at offset 0 of a live `CefObj<T, V>`.
    unsafe { &(*ptr.cast::<CefObj<T, V>>()).data }
}

// ---------------------------------------------------------------------------
// Thread-safe runtime state
// ---------------------------------------------------------------------------

struct SendPtr<T>(*mut T);

impl<T> Clone for SendPtr<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for SendPtr<T> {}

impl<T> SendPtr<T> {
    fn get(self) -> *mut T {
        self.0
    }
}

// SAFETY: Managed exclusively from main UI thread or cleared cleanly on shutdown.
unsafe impl<T> Send for SendPtr<T> {}
// SAFETY: Read/written behind Mutex with main-thread browser API calls.
unsafe impl<T> Sync for SendPtr<T> {}

struct CefHostRecord {
    window_id: WindowId,
    cef_browser_id: i32,
    browser: Option<SendPtr<cef_browser_t>>,
    request_context: Option<SendPtr<cef_request_context_t>>,
    external_links: bool,
    on_navigate: Option<NavHandler>,
    on_popup: Option<NavHandler>,
    initial_url: String,
    url: String,
    title: String,
    load_state: LoadState,
    committed: bool,
}

static NOTIFY_QUEUE: Mutex<Vec<CefNotification>> = Mutex::new(Vec::new());
static WEBVIEWS: Mutex<Option<HashMap<WebViewId, CefHostRecord>>> = Mutex::new(None);
static BROWSER_TO_WV: Mutex<Option<HashMap<i32, WebViewId>>> = Mutex::new(None);
static WV_SCHEMES: Mutex<Option<HashMap<WebViewId, BTreeMap<String, AssetHandler>>>> =
    Mutex::new(None);
static WV_ENDPOINTS: Mutex<Option<HashMap<WebViewId, Arc<IpcEndpoint>>>> = Mutex::new(None);
static PENDING_BROWSER_WVS: Mutex<Vec<WebViewId>> = Mutex::new(Vec::new());
static LIVE_BROWSERS: Mutex<Option<HashSet<i32>>> = Mutex::new(None);

static INITIALIZED: AtomicBool = AtomicBool::new(false);
static SHUTDOWN_DONE: AtomicBool = AtomicBool::new(false);
static REGISTERED_SCHEMES: Mutex<Vec<String>> = Mutex::new(Vec::new());
static ROOT_CACHE_PATH: Mutex<Option<PathBuf>> = Mutex::new(None);
static CEF_WAKE_POSTED: AtomicBool = AtomicBool::new(false);

fn with_webviews<R>(f: impl FnOnce(&mut HashMap<WebViewId, CefHostRecord>) -> R) -> R {
    let mut g = WEBVIEWS.lock().unwrap_or_else(|e| e.into_inner());
    let map = g.get_or_insert_with(HashMap::new);
    f(map)
}

fn with_browser_map<R>(f: impl FnOnce(&mut HashMap<i32, WebViewId>) -> R) -> R {
    let mut g = BROWSER_TO_WV.lock().unwrap_or_else(|e| e.into_inner());
    let map = g.get_or_insert_with(HashMap::new);
    f(map)
}

fn with_schemes<R>(
    f: impl FnOnce(&mut HashMap<WebViewId, BTreeMap<String, AssetHandler>>) -> R,
) -> R {
    let mut g = WV_SCHEMES.lock().unwrap_or_else(|e| e.into_inner());
    let map = g.get_or_insert_with(HashMap::new);
    f(map)
}

fn with_endpoints<R>(f: impl FnOnce(&mut HashMap<WebViewId, Arc<IpcEndpoint>>) -> R) -> R {
    let mut g = WV_ENDPOINTS.lock().unwrap_or_else(|e| e.into_inner());
    let map = g.get_or_insert_with(HashMap::new);
    f(map)
}

fn with_live_browsers<R>(f: impl FnOnce(&mut HashSet<i32>) -> R) -> R {
    let mut g = LIVE_BROWSERS.lock().unwrap_or_else(|e| e.into_inner());
    let set = g.get_or_insert_with(HashSet::new);
    f(set)
}

fn push_notification(n: CefNotification) {
    if let Ok(mut q) = NOTIFY_QUEUE.lock() {
        q.push(n);
    }
}

pub(crate) fn drain_notifications() -> Vec<CefNotification> {
    let mut q = NOTIFY_QUEUE.lock().unwrap_or_else(|e| e.into_inner());
    std::mem::take(&mut *q)
}

// ---------------------------------------------------------------------------
// CefApp + CefBrowserProcessHandler
// ---------------------------------------------------------------------------

struct AppData;
struct BrowserProcessHandlerData;

fn configured_custom_schemes() -> Vec<String> {
    let mut schemes = vec!["app".to_owned(), IPC_SCHEME.to_owned()];
    if let Ok(env_val) = std::env::var("UI_CEF_SCHEMES") {
        for part in env_val.split(',') {
            let s = part.trim().to_ascii_lowercase();
            if !s.is_empty() && !schemes.contains(&s) {
                schemes.push(s);
            }
        }
    }
    if let Ok(extra) = REGISTERED_SCHEMES.lock() {
        for s in extra.iter() {
            if !schemes.contains(s) {
                schemes.push(s.clone());
            }
        }
    }
    schemes
}

unsafe extern "C" fn app_on_before_command_line(
    _self: *mut cef_app_t,
    process_type: *const cef_string_t,
    command_line: *mut cef_command_line_t,
) {
    if let Ok(api) = CefApi::get()
        && !command_line.is_null()
    {
        // SAFETY: `process_type` is null or a valid CEF string passed by CEF.
        let ptype = unsafe { api.from_cef_string(process_type) };
        if ptype.is_empty() {
            // SAFETY: `command_line` is valid for the duration of this callback.
            unsafe {
                if let Some(append) = (*command_line).append_switch {
                    #[cfg(target_os = "macos")]
                    {
                        let mut k = api.cef_string("use-mock-keychain");
                        append(command_line, &k);
                        api.clear_cef_string(&mut k);
                    }
                    let _ = &append;
                }
            }
        }
    }
    // SAFETY: Transfers one ref to incoming CEF `command_line` parameter back to CEF.
    unsafe { cef_release(command_line) };
}

unsafe extern "C" fn app_on_register_custom_schemes(
    _self: *mut cef_app_t,
    registrar: *mut cef_scheme_registrar_t,
) {
    if let Ok(api) = CefApi::get()
        && !registrar.is_null()
    {
        let opts = CEF_SCHEME_OPTION_STANDARD
            | CEF_SCHEME_OPTION_SECURE
            | CEF_SCHEME_OPTION_CORS_ENABLED
            | CEF_SCHEME_OPTION_FETCH_ENABLED
            | CEF_SCHEME_OPTION_CSP_BYPASSING;
        // SAFETY: `registrar` is a scoped CEF pointer valid only for this synchronous call.
        if let Some(add) = unsafe { (*registrar).add_custom_scheme } {
            for scheme in configured_custom_schemes() {
                let mut s = api.cef_string(&scheme);
                // SAFETY: Valid registrar pointer and initialized CEF string.
                unsafe {
                    add(registrar, &s, opts);
                    api.clear_cef_string(&mut s);
                }
            }
        }
    }
}

static BROWSER_PROC_HANDLER: OnceLock<SendPtr<cef_browser_process_handler_t>> = OnceLock::new();
static SCHEME_FACTORY: OnceLock<SendPtr<cef_scheme_handler_factory_t>> = OnceLock::new();

unsafe extern "C" fn bph_on_context_initialized(_self: *mut cef_browser_process_handler_t) {
    if let Ok(api) = CefApi::get() {
        let factory = global_scheme_factory();
        for scheme in configured_custom_schemes() {
            let mut s = api.cef_string(&scheme);
            // SAFETY: Transfers 1 ref on `factory` per registration; increment before calling.
            unsafe {
                cef_add_ref(factory);
                (api.cef_register_scheme_handler_factory)(&s, ptr::null(), factory);
                api.clear_cef_string(&mut s);
            }
        }
    }
}

unsafe extern "C" fn bph_on_before_child_launch(
    _self: *mut cef_browser_process_handler_t,
    command_line: *mut cef_command_line_t,
) {
    if let Ok(api) = CefApi::get()
        && !command_line.is_null()
    {
        let csv = configured_custom_schemes().join(",");
        // SAFETY: `command_line` is non-null and valid during this callback.
        unsafe {
            if let Some(append_kv) = (*command_line).append_switch_with_value {
                let mut k = api.cef_string("ui-schemes");
                let mut v = api.cef_string(&csv);
                append_kv(command_line, &k, &v);
                api.clear_cef_string(&mut k);
                api.clear_cef_string(&mut v);
            }
        }
    }
    // SAFETY: Release incoming CEF reference on `command_line`.
    unsafe { cef_release(command_line) };
}

unsafe extern "C" fn bph_on_schedule_message_pump_work(
    _self: *mut cef_browser_process_handler_t,
    delay_ms: i64,
) {
    #[cfg(target_os = "macos")]
    if delay_ms <= 0 {
        macos_objc::wake_app_loop(&CEF_WAKE_POSTED);
    }
    #[cfg(not(target_os = "macos"))]
    let _ = delay_ms;
}

fn open_external_url(url: &str) {
    #[cfg(target_os = "macos")]
    {
        let _ = macos_appkit::open_external(url);
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = url;
    }
}

unsafe extern "C" fn app_get_browser_process_handler(
    _self: *mut cef_app_t,
) -> *mut cef_browser_process_handler_t {
    let ptr = BROWSER_PROC_HANDLER
        .get_or_init(|| {
            let raw = alloc_cef_obj::<cef_browser_process_handler_t, _>(
                BrowserProcessHandlerData,
                |s, base| {
                    s.base = base;
                    s.on_context_initialized = Some(bph_on_context_initialized);
                    s.on_before_child_process_launch = Some(bph_on_before_child_launch);
                    s.on_schedule_message_pump_work = Some(bph_on_schedule_message_pump_work);
                },
            );
            SendPtr(raw)
        })
        .get();
    // SAFETY: Returning a `cef_base_ref_counted_t` across CEF C-API boundary transfers +1 ref.
    unsafe { cef_add_ref(ptr) };
    ptr
}

fn create_cef_app() -> *mut cef_app_t {
    alloc_cef_obj::<cef_app_t, _>(AppData, |s, base| {
        s.base = base;
        s.on_before_command_line_processing = Some(app_on_before_command_line);
        s.on_register_custom_schemes = Some(app_on_register_custom_schemes);
        s.get_browser_process_handler = Some(app_get_browser_process_handler);
    })
}

// ---------------------------------------------------------------------------
// Custom Scheme Factory + Resource Handler (`TID_IO`)
// ---------------------------------------------------------------------------

struct SchemeFactoryData;

#[derive(Default)]
pub(crate) struct CefAsyncState {
    response: Option<Response>,
    callback: usize,
    canceled: bool,
}

impl Drop for CefAsyncState {
    fn drop(&mut self) {
        let cb = std::mem::take(&mut self.callback);
        if cb != 0 {
            // SAFETY: Releases any unconsumed owned `cef_callback_t` reference on drop.
            unsafe { cef_release(cb as *mut c_void) };
        }
    }
}

pub(crate) fn finish_async_rpc(state: &Arc<Mutex<CefAsyncState>>, resp: Response) {
    let cb_ptr = {
        let mut g = state.lock().unwrap_or_else(|e| e.into_inner());
        if g.canceled {
            return;
        }
        g.response = Some(resp);
        std::mem::take(&mut g.callback)
    };
    if cb_ptr != 0 {
        let cb = cb_ptr as *mut cef_callback_t;
        // SAFETY: Valid owned `cef_callback_t`; `cont` resumes CEF's async resource loader,
        // and we release our owned reference afterward.
        unsafe {
            if let Some(cont) = (*cb).cont {
                cont(cb);
            }
            cef_release(cb_ptr as *mut c_void);
        }
    }
}

pub(crate) fn cancel_async_rpc(state: &Arc<Mutex<CefAsyncState>>) {
    let cb_ptr = {
        let mut g = state.lock().unwrap_or_else(|e| e.into_inner());
        g.canceled = true;
        std::mem::take(&mut g.callback)
    };
    if cb_ptr != 0 {
        // SAFETY: Releases unused `cef_callback_t` reference without invoking `cont`.
        unsafe { cef_release(cb_ptr as *mut c_void) };
    }
}

enum ResourceKind {
    Ready(Response),
    PendingRpc {
        endpoint: Arc<IpcEndpoint>,
        method: String,
        arg: Value,
        call: CallSpec,
        state: Arc<Mutex<CefAsyncState>>,
    },
    Async(Arc<Mutex<CefAsyncState>>),
}

struct ResourceHandlerData {
    kind: Mutex<ResourceKind>,
    cursor: AtomicUsize,
}

fn with_ready_response<R>(data: &ResourceHandlerData, f: impl FnOnce(&Response) -> R) -> Option<R> {
    let g = data.kind.lock().unwrap_or_else(|e| e.into_inner());
    match &*g {
        ResourceKind::Ready(resp) => Some(f(resp)),
        ResourceKind::Async(state) | ResourceKind::PendingRpc { state, .. } => {
            let sg = state.lock().unwrap_or_else(|e| e.into_inner());
            sg.response.as_ref().map(f)
        }
    }
}

unsafe extern "C" fn res_open(
    self_ptr: *mut cef_resource_handler_t,
    request: *mut cef_request_t,
    handle_request: *mut c_int,
    callback: *mut cef_callback_t,
) -> c_int {
    // SAFETY: Release incoming `request` reference; we already extracted URL/method/headers/body.
    unsafe { cef_release(request) };

    // SAFETY: `self_ptr` points to live `CefObj<cef_resource_handler_t, ResourceHandlerData>`.
    let data = unsafe { cef_obj_data::<cef_resource_handler_t, ResourceHandlerData>(self_ptr) };
    let pending = {
        let mut kg = data.kind.lock().unwrap_or_else(|e| e.into_inner());
        match &*kg {
            ResourceKind::Ready(_) | ResourceKind::Async(_) => None,
            ResourceKind::PendingRpc { state, .. } => {
                let state_clone = Arc::clone(state);
                let prev = std::mem::replace(&mut *kg, ResourceKind::Async(state_clone));
                if let ResourceKind::PendingRpc {
                    endpoint,
                    method,
                    arg,
                    call,
                    state,
                } = prev
                {
                    Some((endpoint, method, arg, call, state))
                } else {
                    None
                }
            }
        }
    };

    if let Some((endpoint, method, arg, call, state)) = pending {
        {
            let mut sg = state.lock().unwrap_or_else(|e| e.into_inner());
            sg.callback = callback as usize;
        }
        let responder = IpcResponder::new(IpcReplyTarget::Cef(Arc::clone(&state)));
        if let Err(responder) = endpoint.enqueue_rpc(method, arg, call, responder) {
            // Runtime already exited: disarm responder before taking callback & respond synchronously.
            std::mem::forget(responder);
            let cb_ptr = {
                let mut sg = state.lock().unwrap_or_else(|e| e.into_inner());
                sg.response = Some(ipc_binary_response(
                    200,
                    encode_rpc_response(Err("webview closed".to_owned())),
                ));
                std::mem::take(&mut sg.callback)
            };
            if cb_ptr != 0 {
                // SAFETY: Releases unused `cef_callback_t` ref when completing synchronously.
                unsafe { cef_release(cb_ptr as *mut c_void) };
            }
            if !handle_request.is_null() {
                // SAFETY: Valid out-parameter provided by CEF.
                unsafe { *handle_request = 1 };
            }
            return 1;
        }
        if !handle_request.is_null() {
            // SAFETY: Valid out-parameter; `*handle_request = 0` + return `1` tells CEF we will continue async.
            unsafe { *handle_request = 0 };
        }
        return 1;
    }

    if !handle_request.is_null() {
        // SAFETY: Valid out-parameter provided by CEF for synchronous resource handling.
        unsafe { *handle_request = 1 };
    }
    // SAFETY: Release incoming `callback` reference transferred into `open`.
    unsafe { cef_release(callback) };
    1
}

unsafe extern "C" fn res_get_response_headers(
    self_ptr: *mut cef_resource_handler_t,
    response: *mut cef_response_t,
    response_length: *mut i64,
    _redirect_url: *mut cef_string_t,
) {
    if !response.is_null()
        && let Ok(api) = CefApi::get()
    {
        // SAFETY: `self_ptr` was allocated via `alloc_cef_obj::<cef_resource_handler_t, ResourceHandlerData>`.
        let data = unsafe { cef_obj_data::<cef_resource_handler_t, ResourceHandlerData>(self_ptr) };
        with_ready_response(data, |resp| {
            let (mime_base, charset_opt) = match resp.mime.split_once(';') {
                Some((m, rest)) => {
                    let cs = rest
                        .split(';')
                        .find_map(|p| p.trim().strip_prefix("charset="))
                        .map(str::trim);
                    (m.trim(), cs)
                }
                None => (resp.mime.as_str(), None),
            };

            // SAFETY: Calling CEF response setters on valid `response` pointer during callback.
            unsafe {
                if let Some(set_status) = (*response).set_status {
                    set_status(response, c_int::from(resp.status));
                }
                if let Some(set_mime) = (*response).set_mime_type {
                    let mut m = api.cef_string(mime_base);
                    set_mime(response, &m);
                    api.clear_cef_string(&mut m);
                }
                if let Some(cs) = charset_opt
                    && let Some(set_cs) = (*response).set_charset
                {
                    let mut c = api.cef_string(cs);
                    set_cs(response, &c);
                    api.clear_cef_string(&mut c);
                }
                if let Some(set_hdrs) = (*response).set_header_map {
                    let mmap = (api.cef_string_multimap_alloc)();
                    for (k, v) in &resp.headers {
                        let mut ks = api.cef_string(k);
                        let mut vs = api.cef_string(v);
                        (api.cef_string_multimap_append)(mmap, &ks, &vs);
                        api.clear_cef_string(&mut ks);
                        api.clear_cef_string(&mut vs);
                    }
                    set_hdrs(response, mmap);
                    (api.cef_string_multimap_free)(mmap);
                }
            }
            if !response_length.is_null() {
                // SAFETY: Valid out-parameter provided by CEF.
                unsafe { *response_length = resp.body.len() as i64 };
            }
        });
    }
    // SAFETY: Release incoming `response` ref from CEF.
    unsafe { cef_release(response) };
}

unsafe extern "C" fn res_read(
    self_ptr: *mut cef_resource_handler_t,
    data_out: *mut c_void,
    bytes_to_read: c_int,
    bytes_read: *mut c_int,
    callback: *mut c_void,
) -> c_int {
    // SAFETY: Release unused async callback reference transferred by CEF.
    unsafe { cef_release(callback) };

    if data_out.is_null() || bytes_to_read <= 0 || bytes_read.is_null() {
        if !bytes_read.is_null() {
            // SAFETY: Valid out-parameter provided by CEF.
            unsafe { *bytes_read = 0 };
        }
        return 0;
    }

    // SAFETY: `self_ptr` points to live `CefObj<cef_resource_handler_t, ResourceHandlerData>`.
    let data = unsafe { cef_obj_data::<cef_resource_handler_t, ResourceHandlerData>(self_ptr) };
    let copied = with_ready_response(data, |resp| {
        let body = &resp.body;
        let pos = data.cursor.load(Ordering::Acquire);
        if pos >= body.len() {
            return 0;
        }
        let avail = body.len() - pos;
        let n = avail.min(bytes_to_read as usize);
        // SAFETY: `data_out` has capacity for at least `bytes_to_read` bytes; `n <= bytes_to_read`.
        unsafe {
            ptr::copy_nonoverlapping(body.as_ptr().add(pos), data_out.cast::<u8>(), n);
        }
        data.cursor.store(pos + n, Ordering::Release);
        n as c_int
    })
    .unwrap_or(0);

    // SAFETY: Valid out-parameter provided by CEF.
    unsafe { *bytes_read = copied };
    c_int::from(copied > 0)
}

unsafe extern "C" fn res_cancel(self_ptr: *mut cef_resource_handler_t) {
    // SAFETY: `self_ptr` points to live `CefObj<cef_resource_handler_t, ResourceHandlerData>`.
    let data = unsafe { cef_obj_data::<cef_resource_handler_t, ResourceHandlerData>(self_ptr) };
    let state = {
        let kg = data.kind.lock().unwrap_or_else(|e| e.into_inner());
        match &*kg {
            ResourceKind::Async(s) | ResourceKind::PendingRpc { state: s, .. } => {
                Some(Arc::clone(s))
            }
            ResourceKind::Ready(_) => None,
        }
    };
    if let Some(state) = state {
        cancel_async_rpc(&state);
    }
}

fn alloc_resource_handler_with_kind(kind: ResourceKind) -> *mut cef_resource_handler_t {
    alloc_cef_obj::<cef_resource_handler_t, _>(
        ResourceHandlerData {
            kind: Mutex::new(kind),
            cursor: AtomicUsize::new(0),
        },
        |s, base| {
            s.base = base;
            s.open = Some(res_open);
            s.get_response_headers = Some(res_get_response_headers);
            s.read = Some(res_read);
            s.cancel = Some(res_cancel);
        },
    )
}

fn alloc_resource_handler(response: Response) -> *mut cef_resource_handler_t {
    alloc_resource_handler_with_kind(ResourceKind::Ready(response))
}

unsafe fn extract_cef_headers(
    api: &CefApi,
    request: *mut cef_request_t,
) -> BTreeMap<String, String> {
    let mut headers = BTreeMap::new();
    // SAFETY: `request` is non-null during `resolve_scheme_handler`.
    if let Some(get_hdrs) = unsafe { (*request).get_header_map } {
        // SAFETY: Allocates and fills a CEF string multimap, then frees it after extraction.
        unsafe {
            let mmap = (api.cef_string_multimap_alloc)();
            get_hdrs(request, mmap);
            let count = (api.cef_string_multimap_size)(mmap);
            for i in 0..count {
                let mut k = cef_string_t::default();
                let mut v = cef_string_t::default();
                if (api.cef_string_multimap_key)(mmap, i, &mut k) != 0
                    && (api.cef_string_multimap_value)(mmap, i, &mut v) != 0
                {
                    headers.insert(api.from_cef_string(&k), api.from_cef_string(&v));
                }
                api.clear_cef_string(&mut k);
                api.clear_cef_string(&mut v);
            }
            (api.cef_string_multimap_free)(mmap);
        }
    }
    headers
}

unsafe fn extract_cef_post_body(request: *mut cef_request_t) -> Vec<u8> {
    // SAFETY: `request` is valid during `resolve_scheme_handler`; releases returned `post_data` & elements.
    unsafe {
        let Some(get_pd) = (*request).get_post_data else {
            return Vec::new();
        };
        let pd: *mut cef_post_data_t = get_pd(request);
        if pd.is_null() {
            return Vec::new();
        }
        let mut out = Vec::new();
        if let (Some(get_cnt), Some(get_elems)) = ((*pd).get_element_count, (*pd).get_elements) {
            let mut count = get_cnt(pd);
            if count > 0 {
                let mut elems: Vec<*mut cef_post_data_element_t> = vec![ptr::null_mut(); count];
                get_elems(pd, &mut count, elems.as_mut_ptr());
                for elem in elems.into_iter().take(count) {
                    if elem.is_null() {
                        continue;
                    }
                    if let Some(get_type) = (*elem).get_type
                        && get_type(elem) == cef_postdataelement_type_t::PDE_TYPE_BYTES
                        && let (Some(get_len), Some(get_bytes)) =
                            ((*elem).get_bytes_count, (*elem).get_bytes)
                    {
                        let len = get_len(elem);
                        if len > 0 {
                            let start = out.len();
                            out.resize(start + len, 0);
                            let read =
                                get_bytes(elem, len, out[start..].as_mut_ptr().cast::<c_void>());
                            out.truncate(start + read);
                        }
                    }
                    cef_release(elem);
                }
            }
        }
        cef_release(pd);
        out
    }
}

fn resolve_scheme_handler(
    browser: *mut cef_browser_t,
    known_wv: Option<WebViewId>,
    scheme_override: Option<&str>,
    request: *mut cef_request_t,
) -> *mut cef_resource_handler_t {
    let Ok(api) = CefApi::get() else {
        return ptr::null_mut();
    };
    if request.is_null() {
        return ptr::null_mut();
    }

    let url = unsafe {
        // SAFETY: `request` is non-null during callback; `take_userfree_string` frees CEF copy.
        if let Some(get_url) = (*request).get_url {
            api.take_userfree_string(get_url(request))
        } else {
            String::new()
        }
    };
    let method = unsafe {
        // SAFETY: `request` is non-null during callback.
        if let Some(get_m) = (*request).get_method {
            let m = api.take_userfree_string(get_m(request));
            if m.is_empty() { "GET".to_owned() } else { m }
        } else {
            "GET".to_owned()
        }
    };

    let scheme = scheme_override
        .map(str::to_ascii_lowercase)
        .or_else(|| url.split_once("://").map(|(s, _)| s.to_ascii_lowercase()))
        .unwrap_or_default();

    let wv_id = known_wv
        .or_else(|| {
            if browser.is_null() {
                None
            } else {
                // SAFETY: `browser` is valid for this callback duration.
                let bid = unsafe { (*browser).get_identifier.map(|f| f(browser)).unwrap_or(0) };
                with_browser_map(|m| m.get(&bid).copied())
            }
        })
        .or_else(|| {
            let g = PENDING_BROWSER_WVS
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            g.first().copied()
        });

    if scheme == IPC_SCHEME {
        // SAFETY: `request` is valid for the synchronous `resolve_scheme_handler` call.
        let body = unsafe { extract_cef_post_body(request) };
        let endpoint = wv_id.and_then(|id| with_endpoints(|all| all.get(&id).cloned()));
        let Some(endpoint) = endpoint else {
            return alloc_resource_handler(ipc_binary_response(404, Vec::new()));
        };
        return match endpoint.route_request(&url, &method, &body) {
            IpcRouteAction::Immediate(resp) => alloc_resource_handler(resp),
            IpcRouteAction::AsyncRpc {
                method: rpc_method,
                arg,
                call,
            } => alloc_resource_handler_with_kind(ResourceKind::PendingRpc {
                endpoint,
                method: rpc_method,
                arg,
                call,
                state: Arc::new(Mutex::new(CefAsyncState::default())),
            }),
        };
    }

    let closure =
        wv_id.and_then(|id| with_schemes(|all| all.get(&id).and_then(|m| m.get(&scheme).cloned())));

    if let Some(handler) = closure {
        // SAFETY: `request` is valid for this callback duration.
        let headers = unsafe { extract_cef_headers(api, request) };
        // SAFETY: `request` is valid for this callback duration.
        let body = unsafe { extract_cef_post_body(request) };
        let req = Request::new(url, method)
            .with_headers(headers)
            .with_body(body);
        let resp = handler(&req);
        alloc_resource_handler(resp)
    } else if known_wv.is_none() {
        // Scheme registered cleanly on a WebView whose handler returned nothing -> 404.
        alloc_resource_handler(Response::not_found())
    } else {
        ptr::null_mut()
    }
}

unsafe extern "C" fn scheme_factory_create(
    _self: *mut cef_scheme_handler_factory_t,
    browser: *mut cef_browser_t,
    frame: *mut cef_frame_t,
    scheme_name: *const cef_string_t,
    request: *mut cef_request_t,
) -> *mut cef_resource_handler_t {
    let scheme_str = CefApi::get().ok().map(|api| {
        // SAFETY: `scheme_name` is a valid CEF string pointer passed synchronously.
        unsafe { api.from_cef_string(scheme_name) }
    });
    let res = resolve_scheme_handler(browser, None, scheme_str.as_deref(), request);
    // SAFETY: Release incoming CEF references transferred to `create`.
    unsafe {
        cef_release(request);
        cef_release(frame);
        cef_release(browser);
    }
    res
}

fn global_scheme_factory() -> *mut cef_scheme_handler_factory_t {
    SCHEME_FACTORY
        .get_or_init(|| {
            let raw =
                alloc_cef_obj::<cef_scheme_handler_factory_t, _>(SchemeFactoryData, |s, base| {
                    s.base = base;
                    s.create = Some(scheme_factory_create);
                });
            SendPtr(raw)
        })
        .get()
}

// ---------------------------------------------------------------------------
// Per-WebView CefClient + Sub-handlers
// ---------------------------------------------------------------------------

struct ClientData {
    life_span: SendPtr<cef_life_span_handler_t>,
    load: SendPtr<cef_load_handler_t>,
    display: SendPtr<cef_display_handler_t>,
    request: SendPtr<cef_request_handler_t>,
}

struct SubHandlerData {
    wv_id: WebViewId,
    win_id: WindowId,
    resource_request: Option<SendPtr<cef_resource_request_handler_t>>,
}

struct ResourceRequestData {
    wv_id: WebViewId,
}

impl Drop for ClientData {
    fn drop(&mut self) {
        // SAFETY: Each sub-handler pointer was allocated with `rc = 1` for this client.
        unsafe {
            cef_release(self.life_span.get());
            cef_release(self.load.get());
            cef_release(self.display.get());
            cef_release(self.request.get());
        }
    }
}

impl Drop for SubHandlerData {
    fn drop(&mut self) {
        if let Some(rr) = self.resource_request {
            // SAFETY: Balances initial `rc = 1` on `cef_resource_request_handler_t`.
            unsafe { cef_release(rr.get()) };
        }
    }
}

fn nav_decision_for_popup(id: WebViewId, url: &str) -> Nav {
    let (popup_handler, external_links) = with_webviews(|w| {
        let rec = w.get(&id);
        (
            rec.and_then(|r| r.on_popup.clone()),
            rec.is_some_and(|r| r.external_links),
        )
    });
    if let Some(handler) = popup_handler {
        let req = NavRequest::new(url, true);
        if let Ok(mut g) = handler.lock() {
            g(&req)
        } else {
            Nav::Cancel
        }
    } else if external_links {
        Nav::External
    } else {
        Nav::Cancel
    }
}

fn nav_decision_for_browse(id: WebViewId, url: &str, main_frame: bool, user_gesture: bool) -> Nav {
    let (nav_handler, external_links, initial_url) = with_webviews(|w| {
        let rec = w.get(&id);
        (
            rec.and_then(|r| r.on_navigate.clone()),
            rec.is_some_and(|r| r.external_links),
            rec.map(|r| r.initial_url.clone()).unwrap_or_default(),
        )
    });
    if let Some(handler) = nav_handler {
        let req = NavRequest::new(url, main_frame);
        if let Ok(mut g) = handler.lock() {
            g(&req)
        } else {
            Nav::Allow
        }
    } else if external_links && user_gesture && main_frame {
        let req = NavRequest::new(url, main_frame);
        let init_req = NavRequest::new(&initial_url, true);
        if matches!(req.scheme(), Some("http" | "https")) && req.host() != init_req.host() {
            Nav::External
        } else {
            Nav::Allow
        }
    } else {
        Nav::Allow
    }
}

fn is_valid_browser(browser: *mut cef_browser_t) -> bool {
    if browser.is_null() {
        return false;
    }
    // SAFETY: `browser` is valid while inside a CEF callback.
    unsafe { (*browser).is_valid.map(|f| f(browser) != 0).unwrap_or(true) }
}

unsafe extern "C" fn life_on_after_created(
    self_ptr: *mut cef_life_span_handler_t,
    browser: *mut cef_browser_t,
) {
    // SAFETY: `self_ptr` points to live `CefObj<cef_life_span_handler_t, SubHandlerData>`.
    let data = unsafe { cef_obj_data::<cef_life_span_handler_t, SubHandlerData>(self_ptr) };
    let wv_id = data.wv_id;
    let win_id = data.win_id;

    if !browser.is_null() {
        // SAFETY: `browser` is non-null inside callback; add_ref before retaining in `CefHostRecord`.
        let bid = unsafe { (*browser).get_identifier.map(|f| f(browser)).unwrap_or(0) };
        with_live_browsers(|s| {
            s.insert(bid);
        });
        with_browser_map(|m| {
            m.insert(bid, wv_id);
        });
        {
            let mut pend = PENDING_BROWSER_WVS
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(idx) = pend.iter().position(|&id| id == wv_id) {
                pend.remove(idx);
            }
        }
        // SAFETY: Retain a ref on `browser` for `CefHostRecord` while releasing incoming callback ref at end.
        unsafe { cef_add_ref(browser) };
        with_webviews(|w| {
            if let Some(rec) = w.get_mut(&wv_id) {
                if let Some(old) = rec.browser.replace(SendPtr(browser)) {
                    // SAFETY: Releases any previously held browser reference.
                    unsafe { cef_release(old.get()) };
                }
                rec.cef_browser_id = bid;
            } else {
                // Record was already removed before creation completed; close browser cleanly.
                // SAFETY: `browser` is live.
                unsafe {
                    if let Some(get_host) = (*browser).get_host {
                        let host = get_host(browser);
                        if !host.is_null() {
                            if let Some(close) = (*host).close_browser {
                                close(host, 1);
                            }
                            cef_release(host);
                        }
                    }
                    cef_release(browser);
                }
            }
        });

        #[cfg(target_os = "macos")]
        {
            // SAFETY: Fetches native `NSView` handle from Chromium host once while `browser` is valid.
            let view: Id = unsafe {
                let mut v = ptr::null_mut();
                if let Some(get_host) = (*browser).get_host {
                    let host = get_host(browser);
                    if !host.is_null() {
                        if let Some(get_win) = (*host).get_window_handle {
                            v = get_win(host);
                        }
                        cef_release(host);
                    }
                }
                v
            };
            if !view.is_null() {
                // NSViewWidthSizable (2) | NSViewHeightSizable (16)
                msg!((); view, "setAutoresizingMask:", 18usize => usize);
                if let Some((_, cv, _)) = macos_appkit::window_ns_ids(win_id) {
                    let b = rect_of(cv, "bounds");
                    msg!((); view, "setFrame:", b => macos_objc::Rect);
                }
            }
        }
        #[cfg(not(target_os = "macos"))]
        let _ = win_id;
    }

    // SAFETY: Release incoming callback reference on `browser`.
    unsafe { cef_release(browser) };
}

unsafe extern "C" fn life_on_before_popup(
    self_ptr: *mut cef_life_span_handler_t,
    browser: *mut cef_browser_t,
    frame: *mut cef_frame_t,
    _popup_id: c_int,
    target_url: *const cef_string_t,
    _target_frame_name: *const cef_string_t,
    _target_disposition: cef_window_open_disposition_t,
    _user_gesture: c_int,
    _popup_features: *const c_void,
    _window_info: *mut cef_window_info_t,
    _client: *mut *mut cef_client_t,
    _settings: *mut cef_browser_settings_t,
    _extra_info: *mut *mut ui_cef_sys::cef_dictionary_value_t,
    _no_javascript_access: *mut c_int,
) -> c_int {
    // SAFETY: `self_ptr` points to live `CefObj<cef_life_span_handler_t, SubHandlerData>`.
    let data = unsafe { cef_obj_data::<cef_life_span_handler_t, SubHandlerData>(self_ptr) };
    if let Ok(api) = CefApi::get() {
        // SAFETY: `target_url` is a valid CEF string pointer passed synchronously.
        let url = unsafe { api.from_cef_string(target_url) };
        match nav_decision_for_popup(data.wv_id, &url) {
            Nav::Allow => {
                if is_valid_browser(browser) {
                    // SAFETY: Navigate single-page view in place.
                    unsafe {
                        if let Some(get_main) = (*browser).get_main_frame {
                            let mf = get_main(browser);
                            if !mf.is_null() {
                                if let Some(load) = (*mf).load_url {
                                    let mut u = api.cef_string(&url);
                                    load(mf, &u);
                                    api.clear_cef_string(&mut u);
                                }
                                cef_release(mf);
                            }
                        }
                    }
                }
            }
            Nav::External => {
                open_external_url(&url);
            }
            Nav::Cancel => {}
        }
    }
    // SAFETY: Release incoming CEF references. Always cancel separate popup window creation (`1`).
    unsafe {
        cef_release(frame);
        cef_release(browser);
    }
    1
}

unsafe extern "C" fn life_do_close(
    _self: *mut cef_life_span_handler_t,
    browser: *mut cef_browser_t,
) -> c_int {
    // SAFETY: Release incoming CEF reference; return 0 so OS/CEF completes closing cleanly.
    unsafe { cef_release(browser) };
    0
}

unsafe extern "C" fn life_on_before_close(
    self_ptr: *mut cef_life_span_handler_t,
    browser: *mut cef_browser_t,
) {
    // SAFETY: `self_ptr` points to live `CefObj<cef_life_span_handler_t, SubHandlerData>`.
    let data = unsafe { cef_obj_data::<cef_life_span_handler_t, SubHandlerData>(self_ptr) };
    if !browser.is_null() {
        // SAFETY: `browser` is live during `on_before_close`.
        let bid = unsafe { (*browser).get_identifier.map(|f| f(browser)).unwrap_or(0) };
        with_live_browsers(|s| {
            s.remove(&bid);
        });
        with_browser_map(|m| {
            m.remove(&bid);
        });
    }
    with_webviews(|w| {
        if let Some(rec) = w.get_mut(&data.wv_id)
            && let Some(old) = rec.browser.take()
        {
            // SAFETY: Balances the `cef_add_ref` stored in `rec.browser`.
            unsafe { cef_release(old.get()) };
        }
    });
    // SAFETY: Release incoming callback reference on `browser`.
    unsafe { cef_release(browser) };
}

unsafe extern "C" fn load_on_start(
    self_ptr: *mut cef_load_handler_t,
    browser: *mut cef_browser_t,
    frame: *mut cef_frame_t,
    _transition_type: u32,
) {
    // SAFETY: `self_ptr` points to live `CefObj<cef_load_handler_t, SubHandlerData>`.
    let data = unsafe { cef_obj_data::<cef_load_handler_t, SubHandlerData>(self_ptr) };
    let is_main = !frame.is_null()
        && unsafe {
            // SAFETY: `frame` is non-null during callback.
            (*frame).is_main.map(|f| f(frame) != 0).unwrap_or(false)
        };
    if is_main {
        with_webviews(|w| {
            if let Some(rec) = w.get_mut(&data.wv_id) {
                rec.load_state = LoadState::Started;
                rec.committed = false;
            }
        });
        push_notification(CefNotification::Event(
            data.wv_id,
            WebViewEvent::LoadState(LoadState::Started),
        ));
    }
    // SAFETY: Release incoming CEF refs.
    unsafe {
        cef_release(frame);
        cef_release(browser);
    }
}

unsafe extern "C" fn load_on_end(
    self_ptr: *mut cef_load_handler_t,
    browser: *mut cef_browser_t,
    frame: *mut cef_frame_t,
    _http_status_code: c_int,
) {
    // SAFETY: `self_ptr` points to live `CefObj<cef_load_handler_t, SubHandlerData>`.
    let data = unsafe { cef_obj_data::<cef_load_handler_t, SubHandlerData>(self_ptr) };
    let is_main = !frame.is_null()
        && unsafe {
            // SAFETY: `frame` is non-null during callback.
            (*frame).is_main.map(|f| f(frame) != 0).unwrap_or(false)
        };
    if is_main {
        if let Ok(api) = CefApi::get() {
            // SAFETY: Reads current main-frame URL before transitioning to `Finished`.
            let url = unsafe {
                if let Some(get_u) = (*frame).get_url {
                    api.take_userfree_string(get_u(frame))
                } else {
                    String::new()
                }
            };
            with_webviews(|w| {
                if let Some(rec) = w.get_mut(&data.wv_id) {
                    if !url.is_empty() {
                        rec.url = url;
                    }
                    if !rec.committed {
                        rec.committed = true;
                        rec.load_state = LoadState::Committed;
                        push_notification(CefNotification::Event(
                            data.wv_id,
                            WebViewEvent::LoadState(LoadState::Committed),
                        ));
                    }
                    rec.load_state = LoadState::Finished;
                }
            });
        }
        push_notification(CefNotification::Event(
            data.wv_id,
            WebViewEvent::LoadState(LoadState::Finished),
        ));
    }
    // SAFETY: Release incoming CEF refs.
    unsafe {
        cef_release(frame);
        cef_release(browser);
    }
}

unsafe extern "C" fn load_on_error(
    self_ptr: *mut cef_load_handler_t,
    browser: *mut cef_browser_t,
    frame: *mut cef_frame_t,
    error_code: c_int,
    error_text: *const cef_string_t,
    _failed_url: *const cef_string_t,
) {
    // SAFETY: `self_ptr` points to live `CefObj<cef_load_handler_t, SubHandlerData>`.
    let data = unsafe { cef_obj_data::<cef_load_handler_t, SubHandlerData>(self_ptr) };
    let is_main = !frame.is_null()
        && unsafe {
            // SAFETY: `frame` is non-null during callback.
            (*frame).is_main.map(|f| f(frame) != 0).unwrap_or(false)
        };
    // `ERR_ABORTED` (-3) occurs normally on navigation overrides/redirects; do not emit false failure.
    if is_main && error_code != -3 {
        let msg = CefApi::get()
            .ok()
            .map(|api| {
                // SAFETY: `error_text` is valid CEF string pointer passed synchronously.
                unsafe { api.from_cef_string(error_text) }
            })
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| format!("CEF load error {error_code}"));
        with_webviews(|w| {
            if let Some(rec) = w.get_mut(&data.wv_id) {
                rec.load_state = LoadState::Failed(msg.clone());
            }
        });
        push_notification(CefNotification::Event(
            data.wv_id,
            WebViewEvent::LoadState(LoadState::Failed(msg)),
        ));
    }
    // SAFETY: Release incoming CEF refs.
    unsafe {
        cef_release(frame);
        cef_release(browser);
    }
}

unsafe extern "C" fn display_on_address_change(
    self_ptr: *mut cef_display_handler_t,
    browser: *mut cef_browser_t,
    frame: *mut cef_frame_t,
    url: *const cef_string_t,
) {
    // SAFETY: `self_ptr` points to live `CefObj<cef_display_handler_t, SubHandlerData>`.
    let data = unsafe { cef_obj_data::<cef_display_handler_t, SubHandlerData>(self_ptr) };
    let is_main = !frame.is_null()
        && unsafe {
            // SAFETY: `frame` is non-null during callback.
            (*frame).is_main.map(|f| f(frame) != 0).unwrap_or(false)
        };
    if is_main && let Ok(api) = CefApi::get() {
        // SAFETY: `url` is a valid CEF string pointer passed synchronously.
        let url_str = unsafe { api.from_cef_string(url) };
        let emit_committed = with_webviews(|w| {
            if let Some(rec) = w.get_mut(&data.wv_id) {
                rec.url = url_str.clone();
                if !rec.committed && !url_str.is_empty() && url_str != "about:blank" {
                    rec.committed = true;
                    rec.load_state = LoadState::Committed;
                    return true;
                }
            }
            false
        });
        if emit_committed {
            push_notification(CefNotification::Event(
                data.wv_id,
                WebViewEvent::LoadState(LoadState::Committed),
            ));
        }
        push_notification(CefNotification::Event(
            data.wv_id,
            WebViewEvent::UrlChanged(url_str),
        ));
    }
    // SAFETY: Release incoming CEF refs.
    unsafe {
        cef_release(frame);
        cef_release(browser);
    }
}

unsafe extern "C" fn display_on_title_change(
    self_ptr: *mut cef_display_handler_t,
    browser: *mut cef_browser_t,
    title: *const cef_string_t,
) {
    // SAFETY: `self_ptr` points to live `CefObj<cef_display_handler_t, SubHandlerData>`.
    let data = unsafe { cef_obj_data::<cef_display_handler_t, SubHandlerData>(self_ptr) };
    if let Ok(api) = CefApi::get() {
        // SAFETY: `title` is a valid CEF string pointer passed synchronously.
        let title_str = unsafe { api.from_cef_string(title) };
        with_webviews(|w| {
            if let Some(rec) = w.get_mut(&data.wv_id) {
                rec.title = title_str.clone();
            }
        });
        push_notification(CefNotification::Event(
            data.wv_id,
            WebViewEvent::TitleChanged(title_str),
        ));
    }
    // SAFETY: Release incoming CEF ref.
    unsafe { cef_release(browser) };
}

unsafe extern "C" fn display_on_console_message(
    _self: *mut cef_display_handler_t,
    browser: *mut cef_browser_t,
    level: cef_log_severity_t,
    message: *const cef_string_t,
    source: *const cef_string_t,
    line: c_int,
) -> c_int {
    if std::env::var_os("UI_CEF_LOG_CONSOLE").is_some()
        && let Ok(api) = CefApi::get()
    {
        // SAFETY: Synchronous CEF strings from callback.
        let (msg_s, src_s) = unsafe { (api.from_cef_string(message), api.from_cef_string(source)) };
        eprintln!("[ui cef console {level:?}] {src_s}:{line}: {msg_s}");
    }
    // SAFETY: Release incoming CEF ref.
    unsafe { cef_release(browser) };
    0
}

unsafe extern "C" fn rrh_on_before_resource_load(
    _self: *mut cef_resource_request_handler_t,
    browser: *mut cef_browser_t,
    frame: *mut cef_frame_t,
    request: *mut cef_request_t,
    callback: *mut cef_callback_t,
) -> cef_return_value_t {
    // SAFETY: Release incoming CEF refs.
    unsafe {
        cef_release(callback);
        cef_release(request);
        cef_release(frame);
        cef_release(browser);
    }
    cef_return_value_t::RV_CONTINUE
}

unsafe extern "C" fn rrh_get_resource_handler(
    self_ptr: *mut cef_resource_request_handler_t,
    browser: *mut cef_browser_t,
    frame: *mut cef_frame_t,
    request: *mut cef_request_t,
) -> *mut cef_resource_handler_t {
    // SAFETY: `self_ptr` points to live `CefObj<cef_resource_request_handler_t, ResourceRequestData>`.
    let data =
        unsafe { cef_obj_data::<cef_resource_request_handler_t, ResourceRequestData>(self_ptr) };
    let res = resolve_scheme_handler(browser, Some(data.wv_id), None, request);
    // SAFETY: Release incoming CEF refs.
    unsafe {
        cef_release(request);
        cef_release(frame);
        cef_release(browser);
    }
    res
}

unsafe extern "C" fn req_on_before_browse(
    self_ptr: *mut cef_request_handler_t,
    browser: *mut cef_browser_t,
    frame: *mut cef_frame_t,
    request: *mut cef_request_t,
    user_gesture: c_int,
    _is_redirect: c_int,
) -> c_int {
    // SAFETY: `self_ptr` points to live `CefObj<cef_request_handler_t, SubHandlerData>`.
    let data = unsafe { cef_obj_data::<cef_request_handler_t, SubHandlerData>(self_ptr) };
    if !browser.is_null() {
        // SAFETY: Map `browser_id -> wv_id` early in case `on_before_browse` races `on_after_created`.
        let bid = unsafe { (*browser).get_identifier.map(|f| f(browser)).unwrap_or(0) };
        with_browser_map(|m| {
            m.entry(bid).or_insert(data.wv_id);
        });
    }

    let mut cancel = 0;
    if let Ok(api) = CefApi::get()
        && !request.is_null()
        && !frame.is_null()
    {
        // SAFETY: Valid CEF pointers inside callback; `take_userfree_string` frees returned CEF string.
        let url = unsafe {
            if let Some(get_url) = (*request).get_url {
                api.take_userfree_string(get_url(request))
            } else {
                String::new()
            }
        };
        // SAFETY: `frame` is non-null inside callback.
        let main_frame = unsafe { (*frame).is_main.map(|f| f(frame) != 0).unwrap_or(true) };
        match nav_decision_for_browse(data.wv_id, &url, main_frame, user_gesture != 0) {
            Nav::Allow => {}
            Nav::Cancel => cancel = 1,
            Nav::External => {
                open_external_url(&url);
                cancel = 1;
            }
        }
    }

    // SAFETY: Release incoming CEF refs.
    unsafe {
        cef_release(request);
        cef_release(frame);
        cef_release(browser);
    }
    cancel
}

unsafe extern "C" fn req_get_resource_request_handler(
    self_ptr: *mut cef_request_handler_t,
    browser: *mut cef_browser_t,
    frame: *mut cef_frame_t,
    request: *mut cef_request_t,
    _is_navigation: c_int,
    _is_download: c_int,
    _request_initiator: *const cef_string_t,
    _disable_default_handling: *mut c_int,
) -> *mut cef_resource_request_handler_t {
    // SAFETY: `self_ptr` points to live `CefObj<cef_request_handler_t, SubHandlerData>`.
    let data = unsafe { cef_obj_data::<cef_request_handler_t, SubHandlerData>(self_ptr) };
    // SAFETY: Release incoming CEF refs.
    unsafe {
        cef_release(request);
        cef_release(frame);
        cef_release(browser);
    }
    if let Some(rr) = data.resource_request {
        let ptr = rr.get();
        // SAFETY: Transfers +1 ref to CEF caller.
        unsafe { cef_add_ref(ptr) };
        ptr
    } else {
        ptr::null_mut()
    }
}

unsafe extern "C" fn client_get_life_span_handler(
    self_ptr: *mut cef_client_t,
) -> *mut cef_life_span_handler_t {
    // SAFETY: `self_ptr` points to live `CefObj<cef_client_t, ClientData>`.
    let ptr = unsafe { cef_obj_data::<cef_client_t, ClientData>(self_ptr) }
        .life_span
        .get();
    // SAFETY: Transfers +1 ref to CEF caller.
    unsafe { cef_add_ref(ptr) };
    ptr
}

unsafe extern "C" fn client_get_load_handler(
    self_ptr: *mut cef_client_t,
) -> *mut cef_load_handler_t {
    // SAFETY: `self_ptr` points to live `CefObj<cef_client_t, ClientData>`.
    let ptr = unsafe { cef_obj_data::<cef_client_t, ClientData>(self_ptr) }
        .load
        .get();
    // SAFETY: Transfers +1 ref to CEF caller.
    unsafe { cef_add_ref(ptr) };
    ptr
}

unsafe extern "C" fn client_get_display_handler(
    self_ptr: *mut cef_client_t,
) -> *mut cef_display_handler_t {
    // SAFETY: `self_ptr` points to live `CefObj<cef_client_t, ClientData>`.
    let ptr = unsafe { cef_obj_data::<cef_client_t, ClientData>(self_ptr) }
        .display
        .get();
    // SAFETY: Transfers +1 ref to CEF caller.
    unsafe { cef_add_ref(ptr) };
    ptr
}

unsafe extern "C" fn client_get_request_handler(
    self_ptr: *mut cef_client_t,
) -> *mut cef_request_handler_t {
    // SAFETY: `self_ptr` points to live `CefObj<cef_client_t, ClientData>`.
    let ptr = unsafe { cef_obj_data::<cef_client_t, ClientData>(self_ptr) }
        .request
        .get();
    // SAFETY: Transfers +1 ref to CEF caller.
    unsafe { cef_add_ref(ptr) };
    ptr
}

unsafe extern "C" fn client_on_process_message_received(
    _self_ptr: *mut cef_client_t,
    browser: *mut cef_browser_t,
    frame: *mut cef_frame_t,
    _source_process: cef_process_id_t,
    message: *mut cef_process_message_t,
) -> c_int {
    // SAFETY: Release incoming CEF references transferred into `on_process_message_received`.
    unsafe {
        cef_release(message);
        cef_release(frame);
        cef_release(browser);
    }
    0
}

fn alloc_client(wv_id: WebViewId, win_id: WindowId) -> *mut cef_client_t {
    let life_span = alloc_cef_obj::<cef_life_span_handler_t, _>(
        SubHandlerData {
            wv_id,
            win_id,
            resource_request: None,
        },
        |s, base| {
            s.base = base;
            s.on_after_created = Some(life_on_after_created);
            s.on_before_popup = Some(life_on_before_popup);
            s.do_close = Some(life_do_close);
            s.on_before_close = Some(life_on_before_close);
        },
    );
    let load = alloc_cef_obj::<cef_load_handler_t, _>(
        SubHandlerData {
            wv_id,
            win_id,
            resource_request: None,
        },
        |s, base| {
            s.base = base;
            s.on_load_start = Some(load_on_start);
            s.on_load_end = Some(load_on_end);
            s.on_load_error = Some(load_on_error);
        },
    );
    let display = alloc_cef_obj::<cef_display_handler_t, _>(
        SubHandlerData {
            wv_id,
            win_id,
            resource_request: None,
        },
        |s, base| {
            s.base = base;
            s.on_address_change = Some(display_on_address_change);
            s.on_title_change = Some(display_on_title_change);
            s.on_console_message = Some(display_on_console_message);
        },
    );
    let res_req = alloc_cef_obj::<cef_resource_request_handler_t, _>(
        ResourceRequestData { wv_id },
        |s, base| {
            s.base = base;
            s.on_before_resource_load = Some(rrh_on_before_resource_load);
            s.get_resource_handler = Some(rrh_get_resource_handler);
        },
    );
    let request = alloc_cef_obj::<cef_request_handler_t, _>(
        SubHandlerData {
            wv_id,
            win_id,
            resource_request: Some(SendPtr(res_req)),
        },
        |s, base| {
            s.base = base;
            s.on_before_browse = Some(req_on_before_browse);
            s.get_resource_request_handler = Some(req_get_resource_request_handler);
        },
    );

    alloc_cef_obj::<cef_client_t, _>(
        ClientData {
            life_span: SendPtr(life_span),
            load: SendPtr(load),
            display: SendPtr(display),
            request: SendPtr(request),
        },
        |s, base| {
            s.base = base;
            s.get_life_span_handler = Some(client_get_life_span_handler);
            s.get_load_handler = Some(client_get_load_handler);
            s.get_display_handler = Some(client_get_display_handler);
            s.get_request_handler = Some(client_get_request_handler);
            s.on_process_message_received = Some(client_on_process_message_received);
        },
    )
}

// ---------------------------------------------------------------------------
// Lifecycle: subprocess check, initialize, pump, shutdown
// ---------------------------------------------------------------------------

fn build_c_argv() -> (Vec<CString>, Vec<*mut c_char>) {
    let mut cstrings: Vec<CString> = std::env::args_os()
        .map(|s| {
            let bytes = s.to_string_lossy().into_owned().into_bytes();
            CString::new(bytes).unwrap_or_else(|_| CString::new("ui").unwrap())
        })
        .collect();
    if cstrings.is_empty() {
        cstrings.push(CString::new("ui").unwrap());
    }
    let ptrs: Vec<*mut c_char> = cstrings.iter().map(|s| s.as_ptr().cast_mut()).collect();
    (cstrings, ptrs)
}

pub(crate) fn maybe_execute_subprocess() -> Result<Option<i32>> {
    let is_subprocess = std::env::args_os().any(|a| {
        let s = a.to_string_lossy();
        s.starts_with("--type=")
    });
    if !is_subprocess {
        return Ok(None);
    }

    let api = CefApi::get().map_err(Error::Backend)?;
    let (_storage, mut ptrs) = build_c_argv();
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    let main_args = cef_main_args_t {
        argc: ptrs.len() as c_int,
        argv: ptrs.as_mut_ptr(),
    };
    #[cfg(target_os = "windows")]
    let main_args = cef_main_args_t {
        instance: ptr::null_mut(),
    };
    let _ = &mut ptrs;

    let app = create_cef_app();
    // SAFETY: Transfers 1 ref of `app` into `cef_execute_process`.
    let code = unsafe { (api.cef_execute_process)(&main_args, app, ptr::null_mut()) };
    if code >= 0 {
        return Ok(Some(code));
    }
    Ok(None)
}

fn canonical_dir(p: PathBuf) -> PathBuf {
    let _ = std::fs::create_dir_all(&p);
    std::fs::canonicalize(&p).unwrap_or(p)
}

fn singleton_lock_alive(base: &std::path::Path) -> bool {
    let lock_file = base.join("SingletonLock");
    let Ok(target) = std::fs::read_link(&lock_file) else {
        return false;
    };
    let s = target.to_string_lossy();
    let Some((_, pid_str)) = s.rsplit_once('-') else {
        return true;
    };
    let Ok(pid) = pid_str.parse::<i32>() else {
        return true;
    };
    if pid <= 0 {
        return false;
    }
    #[cfg(unix)]
    {
        unsafe extern "C" {
            fn kill(pid: c_int, sig: c_int) -> c_int;
        }
        // SAFETY: `kill(pid, 0)` probes process existence without sending a signal.
        let rc = unsafe { kill(pid as c_int, 0) };
        rc == 0 || std::io::Error::last_os_error().raw_os_error() == Some(1)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        true
    }
}

fn choose_root_cache_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("UI_CEF_CACHE_DIR")
        && !dir.is_empty()
    {
        return canonical_dir(PathBuf::from(dir));
    }

    let app_stem = std::env::current_exe()
        .ok()
        .and_then(|p| p.file_stem().and_then(|s| s.to_str()).map(str::to_owned))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "app".to_owned());

    let home = std::env::var_os("HOME").map(PathBuf::from);
    #[cfg(target_os = "macos")]
    let base = home
        .map(|h| h.join("Library/Application Support/ui-cef").join(&app_stem))
        .unwrap_or_else(|| std::env::temp_dir().join(format!("ui-cef-{app_stem}")));
    #[cfg(not(target_os = "macos"))]
    let base = home
        .map(|h| h.join(".cache/ui/cef-profiles").join(&app_stem))
        .unwrap_or_else(|| std::env::temp_dir().join(format!("ui-cef-{app_stem}")));

    // If a live Chromium instance holds `SingletonLock` for `base`, use an isolated dir for this run.
    if singleton_lock_alive(&base) {
        return canonical_dir(
            std::env::temp_dir().join(format!("ui-cef-run-{}", std::process::id())),
        );
    }
    canonical_dir(base)
}

pub(crate) fn initialize() -> Result<()> {
    if SHUTDOWN_DONE.load(Ordering::Acquire) {
        return Err(Error::InvalidState(
            "CEF runtime cannot be re-initialized after shutdown in the same process".into(),
        ));
    }
    if INITIALIZED.load(Ordering::Acquire) {
        return Ok(());
    }

    let api = CefApi::get().map_err(Error::Backend)?;
    let root_cache = choose_root_cache_dir();
    *ROOT_CACHE_PATH.lock().unwrap_or_else(|e| e.into_inner()) = Some(root_cache.clone());

    let root_cache_str = root_cache.to_string_lossy().into_owned();
    let mut settings = cef_settings_t {
        no_sandbox: 1,
        external_message_pump: 1,
        multi_threaded_message_loop: 0,
        persist_session_cookies: 1,
        log_severity: cef_log_severity_t::LOGSEVERITY_ERROR,
        root_cache_path: api.cef_string(&root_cache_str),
        cache_path: api.cef_string(&root_cache_str),
        ..Default::default()
    };

    #[cfg(target_os = "macos")]
    {
        let paths = ui_cef_sys::locate_macos_paths().map_err(Error::Backend)?;
        settings.framework_dir_path = api.cef_string(&paths.framework_dir.to_string_lossy());
        settings.browser_subprocess_path = api.cef_string(&paths.helper_exe.to_string_lossy());
        let main_bundle = msg!(Id; macos_objc::class("NSBundle"), "mainBundle");
        let main_bid = msg!(Id; main_bundle, "bundleIdentifier");
        if main_bid.is_null()
            && let Some(bundle_root) = paths.helper_exe.ancestors().nth(3)
        {
            settings.main_bundle_path = api.cef_string(&bundle_root.to_string_lossy());
        }
    }

    let (_storage, mut ptrs) = build_c_argv();
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    let main_args = cef_main_args_t {
        argc: ptrs.len() as c_int,
        argv: ptrs.as_mut_ptr(),
    };
    #[cfg(target_os = "windows")]
    let main_args = cef_main_args_t {
        instance: ptr::null_mut(),
    };
    let _ = &mut ptrs;

    let app = create_cef_app();
    // SAFETY: `main_args` and `settings` are valid for the duration of `cef_initialize`;
    // ownership of one `app` reference transfers into `cef_initialize`.
    let ok = unsafe { (api.cef_initialize)(&main_args, &settings, app, ptr::null_mut()) };

    // SAFETY: Clear allocated temporary CEF UTF-16 strings from `settings`.
    unsafe {
        api.clear_cef_string(&mut settings.root_cache_path);
        api.clear_cef_string(&mut settings.cache_path);
        api.clear_cef_string(&mut settings.framework_dir_path);
        api.clear_cef_string(&mut settings.browser_subprocess_path);
        api.clear_cef_string(&mut settings.main_bundle_path);
    }

    if ok == 0 {
        return Err(Error::Backend("cef_initialize() returned false (0)".into()));
    }
    INITIALIZED.store(true, Ordering::Release);
    Ok(())
}

pub(crate) fn do_message_loop_work() {
    CEF_WAKE_POSTED.store(false, Ordering::Release);
    if INITIALIZED.load(Ordering::Acquire)
        && !SHUTDOWN_DONE.load(Ordering::Acquire)
        && let Ok(api) = CefApi::get()
    {
        // SAFETY: Called on the main UI thread when `external_message_pump = 1`.
        unsafe { (api.cef_do_message_loop_work)() };
    }
}

pub(crate) fn shutdown() {
    if !INITIALIZED.load(Ordering::Acquire) || SHUTDOWN_DONE.swap(true, Ordering::AcqRel) {
        return;
    }

    let ids: Vec<WebViewId> = with_webviews(|w| w.keys().copied().collect());
    for id in ids {
        cleanup_webview(id);
    }

    if let Ok(api) = CefApi::get() {
        for _ in 0..80 {
            let remaining = with_live_browsers(|s| s.len());
            // SAFETY: Pumps closing browser callbacks before `cef_shutdown`.
            unsafe { (api.cef_do_message_loop_work)() };
            if remaining == 0 {
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        // SAFETY: Matches prior `cef_initialize` after active browsers close.
        unsafe { (api.cef_shutdown)() };
    }
}

// ---------------------------------------------------------------------------
// WebView mounting & operations
// ---------------------------------------------------------------------------

fn html_to_data_url(html: &str, base: Option<&str>) -> String {
    let final_html = if let Some(b) = base {
        let escaped = b.replace('"', "&quot;");
        let base_tag = format!("<base href=\"{escaped}\">");
        if html.contains("<head>") {
            html.replacen("<head>", &format!("<head>{base_tag}"), 1)
        } else {
            format!("{base_tag}{html}")
        }
    } else {
        html.to_owned()
    };
    let encoded = encode_base64(final_html.as_bytes());
    format!("data:text/html;charset=utf-8;base64,{encoded}")
}

fn sanitize_profile_dir_name(name: &str) -> String {
    let clean: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if clean.is_empty() {
        "profile".to_owned()
    } else {
        clean
    }
}

fn create_profile_request_context(
    api: &CefApi,
    profile: &Profile,
    schemes: &[String],
) -> *mut cef_request_context_t {
    let mut ctx_settings = cef_request_context_settings_t::default();
    let mut owned_path = None;
    let ctx = match profile {
        Profile::Default => ptr::null_mut(),
        Profile::Ephemeral => {
            ctx_settings.persist_session_cookies = 0;
            // SAFETY: Empty `cache_path` creates an isolated incognito/in-memory RequestContext.
            unsafe { (api.cef_request_context_create_context)(&ctx_settings, ptr::null_mut()) }
        }
        Profile::Persistent(name) => {
            let base = ROOT_CACHE_PATH
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
                .unwrap_or_else(choose_root_cache_dir);
            let dir = base.join(format!("p_{}", sanitize_profile_dir_name(name)));
            let _ = std::fs::create_dir_all(&base);
            ctx_settings.persist_session_cookies = 1;
            ctx_settings.cache_path = api.cef_string(&dir.to_string_lossy());
            owned_path = Some(dir);
            // SAFETY: Valid `ctx_settings` with `cache_path` under `root_cache_path`.
            let c =
                unsafe { (api.cef_request_context_create_context)(&ctx_settings, ptr::null_mut()) };
            // SAFETY: Clear temporary CEF UTF-16 string.
            unsafe { api.clear_cef_string(&mut ctx_settings.cache_path) };
            c
        }
    };
    let _ = owned_path;

    if !ctx.is_null() {
        let factory = global_scheme_factory();
        for scheme in schemes {
            let mut s = api.cef_string(scheme);
            // SAFETY: Registers scheme handler factory on custom RequestContext (`+1` ref per call).
            unsafe {
                if let Some(reg) = (*ctx).register_scheme_handler_factory {
                    cef_add_ref(factory);
                    reg(ctx, &s, ptr::null(), factory);
                }
                api.clear_cef_string(&mut s);
            }
        }
    }
    ctx
}

pub(crate) fn mount_cef_webview(
    id: WebViewId,
    win_id: WindowId,
    spec: WebViewSpec,
    ipc_endpoint: Arc<IpcEndpoint>,
) -> Result<()> {
    let api = CefApi::get().map_err(Error::Backend)?;

    #[cfg(target_os = "macos")]
    let (content_view, win_backdrop, bounds_rect) = {
        let Some((_, cv, backdrop)) = macos_appkit::window_ns_ids(win_id) else {
            return Err(Error::InvalidState(format!(
                "window {win_id:?} does not exist"
            )));
        };
        let b = rect_of(cv, "bounds");
        (
            cv,
            backdrop,
            cef_rect_t {
                x: 0,
                y: 0,
                width: b.size.width.max(1.0).round() as c_int,
                height: b.size.height.max(1.0).round() as c_int,
            },
        )
    };
    #[cfg(not(target_os = "macos"))]
    let bounds_rect = cef_rect_t {
        x: 0,
        y: 0,
        width: 800,
        height: 600,
    };

    let mut schemes: Vec<String> = spec
        .scheme_handlers
        .keys()
        .map(|s| s.to_ascii_lowercase())
        .collect();
    if !schemes.iter().any(|s| s == IPC_SCHEME) {
        schemes.push(IPC_SCHEME.to_owned());
    }
    if !schemes.is_empty() {
        let factory = global_scheme_factory();
        let mut reg = REGISTERED_SCHEMES.lock().unwrap_or_else(|e| e.into_inner());
        for s in &schemes {
            if !reg.contains(s) {
                reg.push(s.clone());
                let mut sc = api.cef_string(s);
                // SAFETY: Registers newly observed custom scheme factory on global context.
                unsafe {
                    cef_add_ref(factory);
                    (api.cef_register_scheme_handler_factory)(&sc, ptr::null(), factory);
                    api.clear_cef_string(&mut sc);
                }
            }
        }
    }

    with_schemes(|all| {
        if !spec.scheme_handlers.is_empty() {
            all.insert(id, spec.scheme_handlers);
        }
    });
    with_endpoints(|all| {
        all.insert(id, ipc_endpoint);
    });

    let start_url = match &spec.initial {
        InitialContent::Url(u) => u.clone(),
        InitialContent::Html { html, base_url } => html_to_data_url(html, base_url.as_deref()),
        InitialContent::None => "about:blank".to_owned(),
    };

    let req_ctx = create_profile_request_context(api, &spec.profile, &schemes);
    let retained_ctx = if !req_ctx.is_null() {
        // SAFETY: Retain a copy for `clear_data()` in `CefHostRecord` while transferring 1 ref to `create_browser`.
        unsafe { cef_add_ref(req_ctx) };
        Some(SendPtr(req_ctx))
    } else {
        None
    };

    with_webviews(|w| {
        w.insert(
            id,
            CefHostRecord {
                window_id: win_id,
                cef_browser_id: 0,
                browser: None,
                request_context: retained_ctx,
                external_links: spec.external_links,
                on_navigate: spec.on_navigate,
                on_popup: spec.on_popup,
                initial_url: start_url.clone(),
                url: start_url.clone(),
                title: String::new(),
                load_state: LoadState::Started,
                committed: false,
            },
        );
    });
    PENDING_BROWSER_WVS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(id);

    let mut win_info = cef_window_info_t {
        bounds: bounds_rect,
        runtime_style: cef_runtime_style_t::CEF_RUNTIME_STYLE_ALLOY,
        ..Default::default()
    };
    #[cfg(target_os = "macos")]
    {
        win_info.parent_view = content_view;
    }

    let mut browser_settings = cef_browser_settings_t::default();
    #[cfg(target_os = "macos")]
    if spec.transparent || win_backdrop != Backdrop::None {
        browser_settings.background_color = 0x0000_0000;
    }
    #[cfg(not(target_os = "macos"))]
    if spec.transparent {
        browser_settings.background_color = 0x0000_0000;
    }

    // Populate `extra_info` passed straight to renderer `on_browser_created` for document-start JS shim + init scripts.
    // SAFETY: `cef_dictionary_value_create` returns an owned `+1` ref dictionary transferred into `create_browser`.
    let extra_info = unsafe { (api.cef_dictionary_value_create)() };
    if !extra_info.is_null() {
        // SAFETY: Populating owned `extra_info` dictionary and transferring list ref cleanly.
        unsafe {
            if let Some(set_str) = (*extra_info).set_string {
                let mut k = api.cef_string("ui_shim");
                let mut v = api.cef_string(SHIM_JS);
                set_str(extra_info, &k, &v);
                api.clear_cef_string(&mut k);
                api.clear_cef_string(&mut v);
            }
            if !spec.init_scripts.is_empty()
                && let Some(set_list) = (*extra_info).set_list
            {
                let list = (api.cef_list_value_create)();
                if !list.is_null() {
                    if let Some(list_set_str) = (*list).set_string {
                        for (i, script) in spec.init_scripts.iter().enumerate() {
                            let mut sv = api.cef_string(script);
                            list_set_str(list, i, &sv);
                            api.clear_cef_string(&mut sv);
                        }
                    }
                    let mut lk = api.cef_string("ui_init_scripts");
                    set_list(extra_info, &lk, list);
                    api.clear_cef_string(&mut lk);
                }
            }
        }
    }

    let client = alloc_client(id, win_id);
    let mut url_cef = api.cef_string(&start_url);
    // SAFETY: Transfers 1 ref of `client`, `extra_info`, and `req_ctx` (when non-null) to CEF.
    let ok = unsafe {
        (api.cef_browser_host_create_browser)(
            &win_info,
            client,
            &url_cef,
            &browser_settings,
            extra_info,
            req_ctx,
        )
    };
    // SAFETY: Clear stack CEF string `url_cef`.
    unsafe { api.clear_cef_string(&mut url_cef) };

    if ok == 0 {
        cleanup_webview(id);
        return Err(Error::Backend(
            "cef_browser_host_create_browser() returned false (0)".into(),
        ));
    }
    Ok(())
}

fn with_browser<R>(
    id: WebViewId,
    f: impl FnOnce(&CefApi, *mut cef_browser_t, *mut cef_browser_host_t) -> R,
) -> Option<R> {
    let api = CefApi::get().ok()?;
    let browser = with_webviews(|w| w.get(&id).and_then(|r| r.browser.map(|b| b.get())))?;
    if !is_valid_browser(browser) {
        return None;
    }
    // SAFETY: Temporarily add a ref for the duration of `f`, get `host`, then release both.
    unsafe {
        cef_add_ref(browser);
        let host = (*browser)
            .get_host
            .map(|gh| gh(browser))
            .unwrap_or(ptr::null_mut());
        let out = f(api, browser, host);
        cef_release(host);
        cef_release(browser);
        Some(out)
    }
}

pub(crate) fn load_url(id: WebViewId, url: &str) {
    with_webviews(|w| {
        if let Some(rec) = w.get_mut(&id) {
            rec.url = url.to_owned();
        }
    });
    with_browser(id, |api, browser, _| {
        // SAFETY: `browser` is valid and retained by `with_browser`.
        unsafe {
            if let Some(get_main) = (*browser).get_main_frame {
                let mf = get_main(browser);
                if !mf.is_null() {
                    if let Some(load) = (*mf).load_url {
                        let mut u = api.cef_string(url);
                        load(mf, &u);
                        api.clear_cef_string(&mut u);
                    }
                    cef_release(mf);
                }
            }
        }
    });
}

pub(crate) fn load_html(id: WebViewId, html: &str, base: Option<&str>) {
    let url = html_to_data_url(html, base);
    load_url(id, &url);
}

pub(crate) fn reload(id: WebViewId) {
    with_browser(id, |_, browser, _| {
        // SAFETY: `browser` is valid and retained by `with_browser`.
        unsafe {
            if let Some(r) = (*browser).reload {
                r(browser);
            }
        }
    });
}

pub(crate) fn go_back(id: WebViewId) {
    with_browser(id, |_, browser, _| {
        // SAFETY: `browser` is valid and retained by `with_browser`.
        unsafe {
            if (*browser)
                .can_go_back
                .map(|f| f(browser) != 0)
                .unwrap_or(false)
                && let Some(gb) = (*browser).go_back
            {
                gb(browser);
            }
        }
    });
}

pub(crate) fn go_forward(id: WebViewId) {
    with_browser(id, |_, browser, _| {
        // SAFETY: `browser` is valid and retained by `with_browser`.
        unsafe {
            if (*browser)
                .can_go_forward
                .map(|f| f(browser) != 0)
                .unwrap_or(false)
                && let Some(gf) = (*browser).go_forward
            {
                gf(browser);
            }
        }
    });
}

pub(crate) fn set_zoom(id: WebViewId, zoom: f64) {
    // CEF `zoomLevel` uses logarithmic scale: `1.2 ^ level == zoom`.
    let level = if zoom <= 0.0 {
        0.0
    } else {
        zoom.ln() / 1.2_f64.ln()
    };
    with_browser(id, |_, _, host| {
        if !host.is_null() {
            // SAFETY: `host` is valid and retained by `with_browser`.
            unsafe {
                if let Some(sz) = (*host).set_zoom_level {
                    sz(host, level);
                }
            }
        }
    });
}

pub(crate) fn set_inspectable(_id: WebViewId, _inspectable: bool) {
    // Chromium/CEF supports DevTools inspection unconditionally via `open_devtools()`.
}

pub(crate) fn open_devtools(id: WebViewId) -> Result<()> {
    let launched = with_browser(id, |api, _, host| {
        if host.is_null() {
            return false;
        }
        let mut win_info = cef_window_info_t {
            window_name: api.cef_string("DevTools"),
            ..Default::default()
        };
        let dev_settings = cef_browser_settings_t::default();
        // SAFETY: Opens a standalone CEF DevTools window for this browser host.
        unsafe {
            if let Some(show) = (*host).show_dev_tools {
                show(host, &win_info, ptr::null_mut(), &dev_settings, ptr::null());
            }
            api.clear_cef_string(&mut win_info.window_name);
        }
        true
    });
    if launched == Some(true) {
        Ok(())
    } else {
        Err(Error::InvalidState(format!("webview {id:?} is not ready")))
    }
}

pub(crate) fn eval(id: WebViewId, js: &str) -> Result<()> {
    let ran = with_browser(id, |api, browser, _| {
        // SAFETY: `browser` is valid and retained by `with_browser`.
        unsafe {
            if let Some(get_main) = (*browser).get_main_frame {
                let mf = get_main(browser);
                if !mf.is_null() {
                    if let Some(exec) = (*mf).execute_java_script {
                        let mut code = api.cef_string(js);
                        let mut src = api.cef_string("ui://eval.js");
                        exec(mf, &code, &src, 1);
                        api.clear_cef_string(&mut code);
                        api.clear_cef_string(&mut src);
                    }
                    cef_release(mf);
                    return true;
                }
            }
        }
        false
    });
    if ran == Some(true) {
        Ok(())
    } else {
        Err(Error::InvalidState(format!(
            "webview {id:?} is not mounted"
        )))
    }
}

pub(crate) fn set_visible(id: WebViewId, visible: bool) {
    with_browser(id, |_, _, host| {
        if !host.is_null() {
            // SAFETY: `host` is valid and retained by `with_browser`.
            let view: Id = unsafe {
                if let Some(wh) = (*host).was_hidden {
                    wh(host, c_int::from(!visible));
                }
                #[cfg(target_os = "macos")]
                if let Some(get_win) = (*host).get_window_handle {
                    get_win(host)
                } else {
                    ptr::null_mut()
                }
                #[cfg(not(target_os = "macos"))]
                {
                    ptr::null_mut()
                }
            };
            #[cfg(target_os = "macos")]
            if !view.is_null() {
                msg!((); view, "setHidden:", !visible => bool);
            }
            #[cfg(not(target_os = "macos"))]
            let _ = view;
        }
    });
}

pub(crate) fn focus(id: WebViewId) {
    with_browser(id, |_, _, host| {
        if !host.is_null() {
            // SAFETY: `host` is valid and retained by `with_browser`.
            unsafe {
                if let Some(sf) = (*host).set_focus {
                    sf(host, 1);
                }
            }
        }
    });
}

pub(crate) fn clear_data(id: WebViewId) {
    let Ok(_api) = CefApi::get() else {
        return;
    };
    let req_ctx = with_webviews(|w| w.get(&id).and_then(|r| r.request_context));
    if let Some(ctx) = req_ctx {
        let ctx = ctx.get();
        // SAFETY: `ctx` has an owned reference held by `CefHostRecord`.
        unsafe {
            if let Some(get_cm) = (*ctx).get_cookie_manager {
                let cm: *mut cef_cookie_manager_t = get_cm(ctx, ptr::null_mut());
                if !cm.is_null() {
                    if let Some(del) = (*cm).delete_cookies {
                        del(cm, ptr::null(), ptr::null(), ptr::null_mut());
                    }
                    cef_release(cm);
                }
            }
            if let Some(clear_cache) = (*ctx).clear_http_cache {
                clear_cache(ctx, ptr::null_mut());
            }
        }
    }
}

pub(crate) fn snapshot_for(id: WebViewId) -> Option<(LoadState, String, String)> {
    with_webviews(|w| {
        w.get(&id)
            .map(|r| (r.load_state.clone(), r.url.clone(), r.title.clone()))
    })
}

pub(crate) fn cef_browser_id_for(id: WebViewId) -> i32 {
    with_webviews(|w| w.get(&id).map(|r| r.cef_browser_id).unwrap_or(0))
}

fn cleanup_webview(id: WebViewId) {
    let rec = with_webviews(|w| w.remove(&id));
    with_schemes(|all| {
        all.remove(&id);
    });
    with_endpoints(|all| {
        all.remove(&id);
    });
    if let Some(mut rec) = rec {
        if rec.cef_browser_id != 0 {
            with_browser_map(|m| {
                m.remove(&rec.cef_browser_id);
            });
        }
        if let Some(b) = rec.browser.take() {
            let browser = b.get();
            if is_valid_browser(browser) {
                // SAFETY: Forces closing the CEF child browser before window teardown completes.
                unsafe {
                    if let Some(get_host) = (*browser).get_host {
                        let host = get_host(browser);
                        if !host.is_null() {
                            if let Some(close) = (*host).close_browser {
                                close(host, 1);
                            }
                            cef_release(host);
                        }
                    }
                }
            }
            // SAFETY: Releases owned ref stored in `CefHostRecord`.
            unsafe { cef_release(browser) };
        }
        if let Some(ctx) = rec.request_context.take() {
            // SAFETY: Releases owned `request_context` ref stored in `CefHostRecord`.
            unsafe { cef_release(ctx.get()) };
        }
    }
}

pub(crate) fn cleanup_webviews_for_window(win_id: WindowId) {
    let ids: Vec<WebViewId> = with_webviews(|w| {
        w.iter()
            .filter_map(|(id, rec)| {
                if rec.window_id == win_id {
                    Some(*id)
                } else {
                    None
                }
            })
            .collect()
    });
    for id in ids {
        cleanup_webview(id);
    }
}
