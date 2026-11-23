//! Pure-Rust `WKWebView` engine backend via `libobjc` + `WebKit.framework`.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use crate::backend::macos_appkit::{self, window_ns_ids};
use crate::backend::macos_objc::{
    Id, NIL, Rect, Sel, call_block_isize, class, define_class, msg, ns_string, release,
    rust_string, sel,
};
use crate::backend::{
    IpcEndpoint, IpcReplyTarget, IpcResponder, IpcRouteAction, ipc_binary_response,
};
use crate::event::WebViewEvent;
use crate::ipc::encode_rpc_response;
use crate::js_shim::SHIM_JS;
use crate::webview::{
    AssetHandler, IPC_SCHEME, InitialContent, LoadState, Nav, NavHandler, NavRequest, Profile,
    Request, Response, WebViewId, WebViewSpec,
};
use crate::window::{Backdrop, WindowId};
use crate::{Error, Result};

const NS_VIEW_WIDTH_SIZABLE: usize = 2;
const NS_VIEW_HEIGHT_SIZABLE: usize = 16;

#[allow(dead_code)]
pub(crate) struct WebKitHostRecord {
    pub window_id: WindowId,
    pub wk_webview: Id,
    pub data_store: Id,
    pub msg_handler: Id,
    pub nav_delegate: Id,
    pub ui_delegate: Id,
    pub kvo_observer: Id,
    pub scheme_handlers_obj: Vec<Id>,
    pub scheme_closures: BTreeMap<String, AssetHandler>,
    pub ipc_endpoint: Arc<IpcEndpoint>,
    pub external_links: bool,
    pub on_navigate: Option<NavHandler>,
    pub on_popup: Option<NavHandler>,
    pub load_state: LoadState,
    pub url: String,
    pub title: String,
}

#[derive(Debug, Clone)]
pub(crate) enum WebKitNotification {
    WkFallback(WebViewId, String),
    Event(WebViewId, WebViewEvent),
}

struct ActiveWkTask {
    wv_id: WebViewId,
    task: Id,
}

static WK_TASK_SEQ: AtomicU64 = AtomicU64::new(1);

thread_local! {
    static WEBVIEWS: RefCell<HashMap<WebViewId, WebKitHostRecord>> = RefCell::new(HashMap::new());
    static WK_TO_ID: RefCell<HashMap<usize, WebViewId>> = RefCell::new(HashMap::new());
    static DELEGATE_TO_ID: RefCell<HashMap<usize, WebViewId>> = RefCell::new(HashMap::new());
    static SCHEME_TARGET: RefCell<HashMap<usize, (WebViewId, String)>> = RefCell::new(HashMap::new());
    static ACTIVE_WK_TASKS: RefCell<HashMap<u64, ActiveWkTask>> = RefCell::new(HashMap::new());
    static WK_TASK_PTR_TO_TOKEN: RefCell<HashMap<usize, u64>> = RefCell::new(HashMap::new());
}

static NOTIFY_QUEUE: Mutex<Vec<WebKitNotification>> = Mutex::new(Vec::new());

pub(crate) fn drain_notifications() -> Vec<WebKitNotification> {
    let mut g = NOTIFY_QUEUE.lock().unwrap_or_else(|e| e.into_inner());
    std::mem::take(&mut *g)
}

fn push_notification(n: WebKitNotification) {
    if let Ok(mut g) = NOTIFY_QUEUE.lock() {
        g.push(n);
    }
}

pub(crate) fn snapshot_for(id: WebViewId) -> Option<(LoadState, String, String)> {
    WEBVIEWS.with(|w| {
        w.borrow()
            .get(&id)
            .map(|r| (r.load_state.clone(), r.url.clone(), r.title.clone()))
    })
}

pub(crate) fn wk_ptr_for(id: WebViewId) -> Id {
    WEBVIEWS.with(|w| w.borrow().get(&id).map(|r| r.wk_webview).unwrap_or(NIL))
}

#[allow(dead_code)]
pub(crate) fn window_for(id: WebViewId) -> Option<WindowId> {
    WEBVIEWS.with(|w| w.borrow().get(&id).map(|r| r.window_id))
}

pub(crate) fn is_https_url(id: WebViewId) -> bool {
    WEBVIEWS.with(|w| {
        let borrow = w.borrow();
        let Some(rec) = borrow.get(&id) else {
            return false;
        };
        if !rec.wk_webview.is_null() {
            let live_url = msg!(Id; rec.wk_webview, "URL");
            if !live_url.is_null() {
                let s = rust_string(msg!(Id; live_url, "absoluteString"));
                if !s.is_empty() {
                    return s.len() >= 8 && s[..8].eq_ignore_ascii_case("https://");
                }
            }
        }
        rec.url.len() >= 8 && rec.url[..8].eq_ignore_ascii_case("https://")
    })
}

fn register_wk_task(wv_id: WebViewId, task: Id) -> u64 {
    let token = WK_TASK_SEQ.fetch_add(1, Ordering::Relaxed);
    let retained = msg!(Id; task, "retain");
    let ptr = retained as usize;
    ACTIVE_WK_TASKS.with(|m| {
        m.borrow_mut().insert(
            token,
            ActiveWkTask {
                wv_id,
                task: retained,
            },
        )
    });
    WK_TASK_PTR_TO_TOKEN.with(|m| m.borrow_mut().insert(ptr, token));
    token
}

fn take_wk_task(token: u64) -> Option<Id> {
    let entry = ACTIVE_WK_TASKS.with(|m| m.borrow_mut().remove(&token))?;
    WK_TASK_PTR_TO_TOKEN.with(|m| m.borrow_mut().remove(&(entry.task as usize)));
    Some(entry.task)
}

pub(crate) fn abandon_wk_task(token: u64) {
    if let Some(task) = take_wk_task(token) {
        release(task);
    }
}

pub(crate) fn finish_wk_task(token: u64, resp: Response) {
    let Some(task) = take_wk_task(token) else {
        return;
    };
    let ns_req = msg!(Id; task, "request");
    let ns_url = if ns_req.is_null() {
        NIL
    } else {
        msg!(Id; ns_req, "URL")
    };
    send_wk_scheme_response(task, ns_url, &resp);
    release(task);
}

fn send_wk_scheme_response(task: Id, ns_url: Id, resp: &Response) {
    if task.is_null() || ns_url.is_null() {
        return;
    }
    let hdrs = msg!(Id; class("NSMutableDictionary"), "dictionary");
    for (k, v) in &resp.headers {
        msg!((); hdrs, "setObject:forKey:", ns_string(v) => Id, ns_string(k) => Id);
    }
    if resp.status != 204 {
        let len_str = resp.body.len().to_string();
        msg!((); hdrs, "setObject:forKey:", ns_string(&len_str) => Id, ns_string("Content-Length") => Id);
    }
    let ns_resp = msg!(Id; msg!(Id; class("NSHTTPURLResponse"), "alloc"),
        "initWithURL:statusCode:HTTPVersion:headerFields:",
        ns_url => Id, resp.status as isize => isize, ns_string("HTTP/1.1") => Id, hdrs => Id);
    if ns_resp.is_null() {
        return;
    }
    msg!((); task, "didReceiveResponse:", ns_resp => Id);
    release(ns_resp);
    if !resp.body.is_empty() {
        let ns_data = msg!(Id; class("NSData"), "dataWithBytes:length:",
            resp.body.as_ptr() as *const c_void => *const c_void, resp.body.len() => usize);
        msg!((); task, "didReceiveData:", ns_data => Id);
    }
    msg!((); task, "didFinish");
}

fn read_nsurl_request_headers(ns_req: Id) -> BTreeMap<String, String> {
    let mut headers = BTreeMap::new();
    let dict = msg!(Id; ns_req, "allHTTPHeaderFields");
    if dict.is_null() {
        return headers;
    }
    let keys = msg!(Id; dict, "allKeys");
    if keys.is_null() {
        return headers;
    }
    let count = msg!(usize; keys, "count");
    for i in 0..count {
        let k = msg!(Id; keys, "objectAtIndex:", i => usize);
        let v = msg!(Id; dict, "objectForKey:", k => Id);
        if !k.is_null() && !v.is_null() {
            headers.insert(rust_string(k), rust_string(v));
        }
    }
    headers
}

fn read_nsurl_request_body(ns_req: Id) -> Vec<u8> {
    let ns_data = msg!(Id; ns_req, "HTTPBody");
    if !ns_data.is_null() {
        let len = msg!(usize; ns_data, "length");
        if len == 0 {
            return Vec::new();
        }
        let ptr = msg!(*const u8; ns_data, "bytes");
        if ptr.is_null() {
            return Vec::new();
        }
        // SAFETY: `NSData::bytes` returns a valid byte slice of `length` bytes while `ns_data` is live.
        return unsafe { std::slice::from_raw_parts(ptr, len).to_vec() };
    }
    let stream = msg!(Id; ns_req, "HTTPBodyStream");
    if stream.is_null() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut buf = [0u8; 65536];
    msg!((); stream, "open");
    loop {
        let n =
            msg!(isize; stream, "read:maxLength:", buf.as_mut_ptr() => *mut u8, buf.len() => usize);
        if n <= 0 {
            break;
        }
        out.extend_from_slice(&buf[..n as usize]);
    }
    msg!((); stream, "close");
    out
}

// ---------------------------------------------------------------------------
// Objective-C delegate callbacks
// ---------------------------------------------------------------------------

extern "C" fn did_receive_script_message(_this: Id, _sel: Sel, _ucc: Id, message: Id) {
    let wk = msg!(Id; message, "webView");
    let body = msg!(Id; message, "body");
    let raw = rust_string(body);
    let Some(id) = WK_TO_ID.with(|m| m.borrow().get(&(wk as usize)).copied()) else {
        return;
    };
    push_notification(WebKitNotification::WkFallback(id, raw));
}

extern "C" fn start_url_scheme_task(this: Id, _sel: Sel, _wk: Id, task: Id) {
    let Some((id, scheme)) = SCHEME_TARGET.with(|m| m.borrow().get(&(this as usize)).cloned())
    else {
        return;
    };
    let ns_req = msg!(Id; task, "request");
    let ns_url = msg!(Id; ns_req, "URL");
    let url = rust_string(msg!(Id; ns_url, "absoluteString"));
    let method_id = msg!(Id; ns_req, "HTTPMethod");
    let method = if method_id.is_null() {
        "GET".to_owned()
    } else {
        rust_string(method_id)
    };
    let body = read_nsurl_request_body(ns_req);

    if scheme == IPC_SCHEME {
        let endpoint = WEBVIEWS.with(|w| w.borrow().get(&id).map(|r| r.ipc_endpoint.clone()));
        let Some(endpoint) = endpoint else {
            send_wk_scheme_response(task, ns_url, &ipc_binary_response(404, Vec::new()));
            return;
        };
        match endpoint.route_request(&url, &method, &body) {
            IpcRouteAction::Immediate(resp) => {
                send_wk_scheme_response(task, ns_url, &resp);
            }
            IpcRouteAction::AsyncRpc {
                method: rpc_method,
                arg,
                call,
            } => {
                let token = register_wk_task(id, task);
                let responder = IpcResponder::new(IpcReplyTarget::WebKit(token));
                if let Err(responder) = endpoint.enqueue_rpc(rpc_method, arg, call, responder) {
                    responder.finish(ipc_binary_response(
                        200,
                        encode_rpc_response(Err("webview closed".to_owned())),
                    ));
                }
            }
        }
        return;
    }

    let handler = WEBVIEWS.with(|w| {
        w.borrow()
            .get(&id)
            .and_then(|r| r.scheme_closures.get(&scheme).cloned())
    });
    let Some(handler) = handler else {
        send_wk_scheme_response(task, ns_url, &Response::not_found());
        return;
    };
    let req = Request::new(url, method)
        .with_headers(read_nsurl_request_headers(ns_req))
        .with_body(body);
    let token = register_wk_task(id, task);
    let resp = handler(&req);
    finish_wk_task(token, resp);
}

extern "C" fn stop_url_scheme_task(_this: Id, _sel: Sel, _wk: Id, task: Id) {
    let token = WK_TASK_PTR_TO_TOKEN.with(|m| m.borrow_mut().remove(&(task as usize)));
    if let Some(token) = token
        && let Some(entry) = ACTIVE_WK_TASKS.with(|m| m.borrow_mut().remove(&token))
    {
        release(entry.task);
    }
}

extern "C" fn decide_nav_policy(_this: Id, _sel: Sel, wk: Id, action: Id, decision_handler: Id) {
    let ns_req = msg!(Id; action, "request");
    let ns_url = msg!(Id; ns_req, "URL");
    let url = rust_string(msg!(Id; ns_url, "absoluteString"));
    let target_frame = msg!(Id; action, "targetFrame");
    let main_frame = if target_frame.is_null() {
        true
    } else {
        msg!(bool; target_frame, "isMainFrame")
    };
    let nav_type = msg!(isize; action, "navigationType"); // 0 == WKNavigationTypeLinkActivated

    let Some(id) = WK_TO_ID.with(|m| m.borrow().get(&(wk as usize)).copied()) else {
        call_block_isize(decision_handler, 1);
        return;
    };

    let (handler, external_links, initial_url) = WEBVIEWS.with(|w| {
        let borrow = w.borrow();
        let rec = borrow.get(&id);
        (
            rec.and_then(|r| r.on_navigate.clone()),
            rec.is_some_and(|r| r.external_links),
            rec.map(|r| r.url.clone()).unwrap_or_default(),
        )
    });

    let decision = if let Some(handler) = handler {
        // Never cancel initial custom-scheme / about:blank / data boot loads unintentionally unless callback inspects.
        let req = NavRequest::new(&url, main_frame);
        if let Ok(mut g) = handler.lock() {
            g(&req)
        } else {
            Nav::Allow
        }
    } else if external_links && nav_type == 0 {
        let req = NavRequest::new(&url, main_frame);
        let init_req = NavRequest::new(&initial_url, true);
        if matches!(req.scheme(), Some("http" | "https")) && req.host() != init_req.host() {
            Nav::External
        } else {
            Nav::Allow
        }
    } else {
        Nav::Allow
    };

    match decision {
        Nav::Allow => call_block_isize(decision_handler, 1),
        Nav::Cancel => call_block_isize(decision_handler, 0),
        Nav::External => {
            let _ = macos_appkit::open_external(&url);
            call_block_isize(decision_handler, 0);
        }
    }
}

extern "C" fn did_start_nav(_this: Id, _sel: Sel, wk: Id, _nav: Id) {
    emit_load_state(wk, LoadState::Started);
}

extern "C" fn did_commit_nav(_this: Id, _sel: Sel, wk: Id, _nav: Id) {
    sync_url_and_title(wk);
    emit_load_state(wk, LoadState::Committed);
}

extern "C" fn did_finish_nav(_this: Id, _sel: Sel, wk: Id, _nav: Id) {
    sync_url_and_title(wk);
    emit_load_state(wk, LoadState::Finished);
}

extern "C" fn did_fail_nav(_this: Id, _sel: Sel, wk: Id, _nav: Id, error: Id) {
    let msg_str = rust_string(msg!(Id; error, "localizedDescription"));
    emit_load_state(wk, LoadState::Failed(msg_str));
}

extern "C" fn create_popup_webview(
    _this: Id,
    _sel: Sel,
    wk: Id,
    _cfg: Id,
    action: Id,
    _features: Id,
) -> Id {
    let ns_req = msg!(Id; action, "request");
    let ns_url = msg!(Id; ns_req, "URL");
    let url = rust_string(msg!(Id; ns_url, "absoluteString"));

    if let Some(id) = WK_TO_ID.with(|m| m.borrow().get(&(wk as usize)).copied()) {
        let (popup_handler, external_links) = WEBVIEWS.with(|w| {
            let b = w.borrow();
            let rec = b.get(&id);
            (
                rec.and_then(|r| r.on_popup.clone()),
                rec.is_some_and(|r| r.external_links),
            )
        });
        let nav = if let Some(handler) = popup_handler {
            let req = NavRequest::new(&url, true);
            if let Ok(mut g) = handler.lock() {
                g(&req)
            } else {
                Nav::Cancel
            }
        } else if external_links {
            Nav::External
        } else {
            Nav::Cancel
        };
        match nav {
            Nav::External => {
                let _ = macos_appkit::open_external(&url);
            }
            Nav::Allow => {
                msg!(Id; wk, "loadRequest:", ns_req => Id);
            }
            Nav::Cancel => {}
        }
    }
    NIL
}

extern "C" fn observe_kvo(
    _this: Id,
    _sel: Sel,
    key_path: Id,
    object: Id,
    _change: Id,
    _ctx: *mut c_void,
) {
    let key = rust_string(key_path);
    let Some(id) = WK_TO_ID.with(|m| m.borrow().get(&(object as usize)).copied()) else {
        return;
    };
    if key == "title" {
        let title = rust_string(msg!(Id; object, "title"));
        WEBVIEWS.with(|w| {
            if let Some(rec) = w.borrow_mut().get_mut(&id) {
                rec.title = title.clone();
            }
        });
        push_notification(WebKitNotification::Event(
            id,
            WebViewEvent::TitleChanged(title),
        ));
    } else if key == "URL" {
        let url_obj = msg!(Id; object, "URL");
        let url = if url_obj.is_null() {
            String::new()
        } else {
            rust_string(msg!(Id; url_obj, "absoluteString"))
        };
        WEBVIEWS.with(|w| {
            if let Some(rec) = w.borrow_mut().get_mut(&id) {
                rec.url = url.clone();
            }
        });
        push_notification(WebKitNotification::Event(id, WebViewEvent::UrlChanged(url)));
    }
}

fn emit_load_state(wk: Id, state: LoadState) {
    let Some(id) = WK_TO_ID.with(|m| m.borrow().get(&(wk as usize)).copied()) else {
        return;
    };
    WEBVIEWS.with(|w| {
        if let Some(rec) = w.borrow_mut().get_mut(&id) {
            rec.load_state = state.clone();
        }
    });
    push_notification(WebKitNotification::Event(
        id,
        WebViewEvent::LoadState(state),
    ));
}

fn sync_url_and_title(wk: Id) {
    let Some(id) = WK_TO_ID.with(|m| m.borrow().get(&(wk as usize)).copied()) else {
        return;
    };
    let url_obj = msg!(Id; wk, "URL");
    let url = if url_obj.is_null() {
        String::new()
    } else {
        rust_string(msg!(Id; url_obj, "absoluteString"))
    };
    let title = rust_string(msg!(Id; wk, "title"));
    WEBVIEWS.with(|w| {
        if let Some(rec) = w.borrow_mut().get_mut(&id) {
            if !url.is_empty() {
                rec.url = url.clone();
            }
            rec.title = title.clone();
        }
    });
}

fn ensure_webkit_classes() -> (Id, Id, Id, Id, Id) {
    static CLASSES: OnceLock<(usize, usize, usize, usize, usize)> = OnceLock::new();
    let &(msg_h, scheme_h, nav_d, ui_d, kvo) = CLASSES.get_or_init(|| {
        let msg_cls = define_class(
            "UiWKScriptMessageHandler",
            "NSObject",
            &[(
                "userContentController:didReceiveScriptMessage:",
                did_receive_script_message as *const c_void,
                "v@:@@",
            )],
            &["WKScriptMessageHandler"],
        );
        let scheme_cls = define_class(
            "UiWKURLSchemeHandler",
            "NSObject",
            &[
                (
                    "webView:startURLSchemeTask:",
                    start_url_scheme_task as *const c_void,
                    "v@:@@",
                ),
                (
                    "webView:stopURLSchemeTask:",
                    stop_url_scheme_task as *const c_void,
                    "v@:@@",
                ),
            ],
            &["WKURLSchemeHandler"],
        );
        let nav_cls = define_class(
            "UiWKNavigationDelegate",
            "NSObject",
            &[
                (
                    "webView:decidePolicyForNavigationAction:decisionHandler:",
                    decide_nav_policy as *const c_void,
                    "v@:@@@",
                ),
                (
                    "webView:didStartProvisionalNavigation:",
                    did_start_nav as *const c_void,
                    "v@:@@",
                ),
                (
                    "webView:didCommitNavigation:",
                    did_commit_nav as *const c_void,
                    "v@:@@",
                ),
                (
                    "webView:didFinishNavigation:",
                    did_finish_nav as *const c_void,
                    "v@:@@",
                ),
                (
                    "webView:didFailNavigation:withError:",
                    did_fail_nav as *const c_void,
                    "v@:@@@",
                ),
                (
                    "webView:didFailProvisionalNavigation:withError:",
                    did_fail_nav as *const c_void,
                    "v@:@@@",
                ),
            ],
            &["WKNavigationDelegate"],
        );
        let ui_cls = define_class(
            "UiWKUIDelegate",
            "NSObject",
            &[(
                "webView:createWebViewWithConfiguration:forNavigationAction:windowFeatures:",
                create_popup_webview as *const c_void,
                "@@:@@@@",
            )],
            &["WKUIDelegate"],
        );
        let kvo_cls = define_class(
            "UiWKKvoObserver",
            "NSObject",
            &[(
                "observeValueForKeyPath:ofObject:change:context:",
                observe_kvo as *const c_void,
                "v@:@@@^v",
            )],
            &[],
        );
        (
            msg_cls as usize,
            scheme_cls as usize,
            nav_cls as usize,
            ui_cls as usize,
            kvo_cls as usize,
        )
    });
    (
        msg_h as Id,
        scheme_h as Id,
        nav_d as Id,
        ui_d as Id,
        kvo as Id,
    )
}

fn uuid_bytes_for_name(name: &str) -> [u8; 16] {
    let mut h1: u64 = 0xcbf29ce484222325;
    let mut h2: u64 = 0xaf63bd4c8601b7df;
    for &b in name.as_bytes() {
        h1 ^= u64::from(b);
        h1 = h1.wrapping_mul(0x100000001b3);
        h2 = h2.wrapping_mul(0x100000001b3) ^ u64::from(b);
    }
    let mut bytes = [0u8; 16];
    bytes[..8].copy_from_slice(&h1.to_le_bytes());
    bytes[8..].copy_from_slice(&h2.to_le_bytes());
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    bytes
}

fn build_data_store(profile: &Profile) -> Id {
    match profile {
        Profile::Ephemeral => msg!(Id; class("WKWebsiteDataStore"), "nonPersistentDataStore"),
        Profile::Default => msg!(Id; class("WKWebsiteDataStore"), "defaultDataStore"),
        Profile::Persistent(name) => {
            if msg!(bool; class("WKWebsiteDataStore"), "respondsToSelector:", sel("dataStoreForIdentifier:") => Sel)
            {
                let bytes = uuid_bytes_for_name(name);
                let uuid = msg!(Id; msg!(Id; class("NSUUID"), "alloc"), "initWithUUIDBytes:", bytes.as_ptr() => *const u8);
                let store =
                    msg!(Id; class("WKWebsiteDataStore"), "dataStoreForIdentifier:", uuid => Id);
                release(uuid);
                store
            } else {
                msg!(Id; class("WKWebsiteDataStore"), "defaultDataStore")
            }
        }
    }
}

pub(crate) fn mount_webkit_webview(
    id: WebViewId,
    win_id: WindowId,
    spec: WebViewSpec,
    ipc_endpoint: Arc<IpcEndpoint>,
) -> Result<()> {
    let Some((_, content_view, win_backdrop)) = window_ns_ids(win_id) else {
        return Err(Error::InvalidState(format!(
            "window {win_id:?} is not open"
        )));
    };
    let (msg_cls, scheme_cls, nav_cls, ui_cls, kvo_cls) = ensure_webkit_classes();

    let cfg = msg!(Id; msg!(Id; class("WKWebViewConfiguration"), "alloc"), "init");
    if cfg.is_null() {
        return Err(Error::Backend(
            "failed to create WKWebViewConfiguration".into(),
        ));
    }

    let data_store = build_data_store(&spec.profile);
    msg!((); cfg, "setWebsiteDataStore:", data_store => Id);

    let mut scheme_handlers_obj = Vec::new();
    for scheme in spec
        .scheme_handlers
        .keys()
        .map(String::as_str)
        .chain(std::iter::once(IPC_SCHEME))
    {
        let sh = msg!(Id; msg!(Id; scheme_cls, "alloc"), "init");
        SCHEME_TARGET.with(|m| m.borrow_mut().insert(sh as usize, (id, scheme.to_owned())));
        msg!((); cfg, "setURLSchemeHandler:forURLScheme:", sh => Id, ns_string(scheme) => Id);
        scheme_handlers_obj.push(sh);
    }

    let ucc = msg!(Id; cfg, "userContentController");
    let msg_handler = msg!(Id; msg!(Id; msg_cls, "alloc"), "init");
    msg!((); ucc, "addScriptMessageHandler:name:", msg_handler => Id, ns_string("ui") => Id);

    let shim_script = msg!(Id; msg!(Id; class("WKUserScript"), "alloc"),
        "initWithSource:injectionTime:forMainFrameOnly:",
        ns_string(SHIM_JS) => Id, 0isize => isize, true => bool);
    msg!((); ucc, "addUserScript:", shim_script => Id);
    release(shim_script);

    for extra in &spec.init_scripts {
        let s = msg!(Id; msg!(Id; class("WKUserScript"), "alloc"),
            "initWithSource:injectionTime:forMainFrameOnly:",
            ns_string(extra) => Id, 0isize => isize, true => bool);
        msg!((); ucc, "addUserScript:", s => Id);
        release(s);
    }

    let bounds = msg_bounds(content_view);
    let wk = msg!(Id; msg!(Id; class("WKWebView"), "alloc"), "initWithFrame:configuration:",
        bounds => Rect, cfg => Id);
    release(cfg);
    if wk.is_null() {
        return Err(Error::Backend("failed to create WKWebView".into()));
    }

    msg!((); wk, "setAutoresizingMask:", NS_VIEW_WIDTH_SIZABLE | NS_VIEW_HEIGHT_SIZABLE => usize);
    if spec.transparent || win_backdrop != Backdrop::None {
        macos_appkit::set_key_value_bool(wk, "drawsBackground", false);
    }
    if let Some(ref ua) = spec.user_agent {
        msg!((); wk, "setCustomUserAgent:", ns_string(ua) => Id);
    }
    if msg!(bool; wk, "respondsToSelector:", sel("setInspectable:") => Sel) {
        msg!((); wk, "setInspectable:", spec.inspectable => bool);
    }

    let nav_delegate = msg!(Id; msg!(Id; nav_cls, "alloc"), "init");
    let ui_delegate = msg!(Id; msg!(Id; ui_cls, "alloc"), "init");
    let kvo_observer = msg!(Id; msg!(Id; kvo_cls, "alloc"), "init");
    msg!((); wk, "setNavigationDelegate:", nav_delegate => Id);
    msg!((); wk, "setUIDelegate:", ui_delegate => Id);

    // NSKeyValueObservingOptionNew = 1
    msg!((); wk, "addObserver:forKeyPath:options:context:", kvo_observer => Id, ns_string("title") => Id, 1usize => usize, std::ptr::null_mut::<c_void>() => *mut c_void);
    msg!((); wk, "addObserver:forKeyPath:options:context:", kvo_observer => Id, ns_string("URL") => Id, 1usize => usize, std::ptr::null_mut::<c_void>() => *mut c_void);

    msg!((); content_view, "addSubview:", wk => Id);

    let initial_url = match &spec.initial {
        InitialContent::Url(u) => u.clone(),
        InitialContent::Html {
            base_url: Some(b), ..
        } => b.clone(),
        _ => String::new(),
    };

    WK_TO_ID.with(|m| m.borrow_mut().insert(wk as usize, id));
    DELEGATE_TO_ID.with(|m| {
        let mut g = m.borrow_mut();
        g.insert(nav_delegate as usize, id);
        g.insert(ui_delegate as usize, id);
    });

    WEBVIEWS.with(|w| {
        w.borrow_mut().insert(
            id,
            WebKitHostRecord {
                window_id: win_id,
                wk_webview: wk,
                data_store,
                msg_handler,
                nav_delegate,
                ui_delegate,
                kvo_observer,
                scheme_handlers_obj,
                scheme_closures: spec.scheme_handlers,
                ipc_endpoint,
                external_links: spec.external_links,
                on_navigate: spec.on_navigate,
                on_popup: spec.on_popup,
                load_state: LoadState::Started,
                url: initial_url,
                title: String::new(),
            },
        );
    });

    match spec.initial {
        InitialContent::None => {}
        InitialContent::Url(url) => load_url(id, &url),
        InitialContent::Html { html, base_url } => load_html(id, &html, base_url.as_deref()),
    }

    Ok(())
}

fn msg_bounds(view: Id) -> Rect {
    crate::backend::macos_objc::rect_of(view, "bounds")
}

pub(crate) fn load_url(id: WebViewId, url: &str) {
    let wk = wk_ptr_for(id);
    if wk.is_null() {
        return;
    }
    let ns_url = msg!(Id; class("NSURL"), "URLWithString:", ns_string(url) => Id);
    if ns_url.is_null() {
        return;
    }
    let req = msg!(Id; class("NSURLRequest"), "requestWithURL:", ns_url => Id);
    msg!(Id; wk, "loadRequest:", req => Id);
}

pub(crate) fn load_html(id: WebViewId, html: &str, base_url: Option<&str>) {
    let wk = wk_ptr_for(id);
    if wk.is_null() {
        return;
    }
    let ns_base = match base_url {
        Some(b) => msg!(Id; class("NSURL"), "URLWithString:", ns_string(b) => Id),
        None => NIL,
    };
    msg!(Id; wk, "loadHTMLString:baseURL:", ns_string(html) => Id, ns_base => Id);
}

pub(crate) fn reload(id: WebViewId) {
    let wk = wk_ptr_for(id);
    if !wk.is_null() {
        msg!(Id; wk, "reload");
    }
}

pub(crate) fn go_back(id: WebViewId) {
    let wk = wk_ptr_for(id);
    if !wk.is_null() && msg!(bool; wk, "canGoBack") {
        msg!(Id; wk, "goBack");
    }
}

pub(crate) fn go_forward(id: WebViewId) {
    let wk = wk_ptr_for(id);
    if !wk.is_null() && msg!(bool; wk, "canGoForward") {
        msg!(Id; wk, "goForward");
    }
}

pub(crate) fn set_zoom(id: WebViewId, zoom: f64) {
    let wk = wk_ptr_for(id);
    if !wk.is_null() && msg!(bool; wk, "respondsToSelector:", sel("setPageZoom:") => Sel) {
        msg!((); wk, "setPageZoom:", zoom => f64);
    }
}

pub(crate) fn set_inspectable(id: WebViewId, inspectable: bool) {
    let wk = wk_ptr_for(id);
    if !wk.is_null() && msg!(bool; wk, "respondsToSelector:", sel("setInspectable:") => Sel) {
        msg!((); wk, "setInspectable:", inspectable => bool);
    }
}

pub(crate) fn eval(id: WebViewId, js: &str) -> Result<()> {
    let wk = wk_ptr_for(id);
    if wk.is_null() {
        return Err(Error::InvalidState(format!(
            "webview {id:?} is not mounted"
        )));
    }
    msg!((); wk, "evaluateJavaScript:completionHandler:", ns_string(js) => Id, NIL => Id);
    Ok(())
}

pub(crate) fn set_visible(id: WebViewId, visible: bool) {
    let wk = wk_ptr_for(id);
    if !wk.is_null() {
        msg!((); wk, "setHidden:", !visible => bool);
    }
}

pub(crate) fn focus(id: WebViewId) {
    let wk = wk_ptr_for(id);
    if !wk.is_null() {
        let win = msg!(Id; wk, "window");
        if !win.is_null() {
            msg!(bool; win, "makeFirstResponder:", wk => Id);
        }
    }
}

pub(crate) fn clear_data(id: WebViewId) {
    let store = WEBVIEWS.with(|w| w.borrow().get(&id).map(|r| r.data_store));
    if let Some(store) = store
        && !store.is_null()
    {
        let all_types = msg!(Id; class("WKWebsiteDataStore"), "allWebsiteDataTypes");
        let past = msg!(Id; class("NSDate"), "distantPast");
        msg!((); store, "removeDataOfTypes:modifiedSince:completionHandler:", all_types => Id, past => Id, NIL => Id);
    }
}

pub(crate) fn cleanup_webviews_for_window(win_id: WindowId) {
    let ids: Vec<WebViewId> = WEBVIEWS.with(|w| {
        w.borrow()
            .iter()
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

pub(crate) fn cleanup_webview(id: WebViewId) {
    let Some(rec) = WEBVIEWS.with(|w| w.borrow_mut().remove(&id)) else {
        return;
    };
    let pending_tasks: Vec<(u64, Id)> = ACTIVE_WK_TASKS.with(|m| {
        let mut map = m.borrow_mut();
        let tokens: Vec<u64> = map
            .iter()
            .filter_map(|(&tok, entry)| if entry.wv_id == id { Some(tok) } else { None })
            .collect();
        tokens
            .into_iter()
            .filter_map(|tok| map.remove(&tok).map(|entry| (tok, entry.task)))
            .collect()
    });
    for (_tok, task) in pending_tasks {
        WK_TASK_PTR_TO_TOKEN.with(|m| m.borrow_mut().remove(&(task as usize)));
        release(task);
    }
    WK_TO_ID.with(|m| m.borrow_mut().remove(&(rec.wk_webview as usize)));
    DELEGATE_TO_ID.with(|m| {
        let mut g = m.borrow_mut();
        g.remove(&(rec.nav_delegate as usize));
        g.remove(&(rec.ui_delegate as usize));
    });
    for sh in rec.scheme_handlers_obj {
        SCHEME_TARGET.with(|m| m.borrow_mut().remove(&(sh as usize)));
        release(sh);
    }
    msg!((); rec.wk_webview, "removeObserver:forKeyPath:", rec.kvo_observer => Id, ns_string("title") => Id);
    msg!((); rec.wk_webview, "removeObserver:forKeyPath:", rec.kvo_observer => Id, ns_string("URL") => Id);
    let cfg = msg!(Id; rec.wk_webview, "configuration");
    if !cfg.is_null() {
        let ucc = msg!(Id; cfg, "userContentController");
        if !ucc.is_null() {
            msg!((); ucc, "removeScriptMessageHandlerForName:", ns_string("ui") => Id);
        }
    }
    msg!((); rec.wk_webview, "setNavigationDelegate:", NIL => Id);
    msg!((); rec.wk_webview, "setUIDelegate:", NIL => Id);
    msg!((); rec.wk_webview, "removeFromSuperview");
    release(rec.kvo_observer);
    release(rec.nav_delegate);
    release(rec.ui_delegate);
    release(rec.msg_handler);
    release(rec.wk_webview);
}
