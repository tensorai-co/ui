//! Engine dispatch (`webkit` on macOS default; `cef` on `--features cef`, Windows, Linux)
//! and thread-safe cross-thread main-loop coordinator.

#[cfg(target_os = "macos")]
pub(crate) mod macos_appkit;
#[cfg(target_os = "macos")]
pub(crate) mod macos_objc;
#[cfg(target_os = "macos")]
pub(crate) mod macos_webkit;

#[cfg(feature = "cef")]
pub(crate) mod cef;

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak, mpsc};
use std::thread::{self, ThreadId};
use std::time::Duration;

use crate::app::App;
use crate::call::{CallState, STREAM_OPEN_PREFIX};
use crate::dialog::{FileDialogKind, FileDialogSpec, MessageDialogSpec, MessageResult};
use crate::event::{Event, WebViewEvent, WindowEvent};
use crate::ipc::{
    CallSpec, ClientSendPacket, Value, decode_base64, decode_rpc_request, decode_send_packet, encode_base64,
    encode_event_batch, encode_event_frame, encode_rpc_response, parse,
};
use crate::menu::{ContextMenuItem, Menu, MenuItemUpdate, Role};
use crate::platform::{RawWebViewHandle, RawWindowHandle};
use crate::webview::{
    Ctx, EventHandler, LoadState, Response, RpcHandler, RpcKind, RpcOutcome, WebView, WebViewEventHandler, WebViewId,
    WebViewSpec,
};
use crate::window::{Bounds, Display, Theme, Window, WindowId, WindowSpec, WindowState};
use crate::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EngineKind {
    #[cfg(target_os = "macos")]
    WebKit,
    #[cfg(feature = "cef")]
    Cef,
}

impl EngineKind {
    pub fn resolve() -> Self {
        #[cfg(target_os = "macos")]
        {
            #[cfg(feature = "cef")]
            {
                if let Ok(val) = std::env::var("UI_BACKEND")
                    && val.eq_ignore_ascii_case("webkit")
                {
                    return Self::WebKit;
                }
                Self::Cef
            }
            #[cfg(not(feature = "cef"))]
            {
                Self::WebKit
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            Self::Cef
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            #[cfg(target_os = "macos")]
            Self::WebKit => "webkit",
            #[cfg(feature = "cef")]
            Self::Cef => "cef",
        }
    }
}

const MAX_QUEUED_EVENTS: usize = 4096;

pub(crate) fn ipc_binary_response(status: u16, body: Vec<u8>) -> Response {
    Response {
        status,
        mime: "application/octet-stream".to_owned(),
        headers: BTreeMap::from([
            (
                "Content-Type".to_owned(),
                "application/octet-stream".to_owned(),
            ),
            ("Access-Control-Allow-Origin".to_owned(), "*".to_owned()),
            ("Cache-Control".to_owned(), "no-store".to_owned()),
        ]),
        body,
    }
}

pub(crate) fn ipc_cors_preflight_response() -> Response {
    Response {
        status: 204,
        mime: "text/plain".to_owned(),
        headers: BTreeMap::from([
            ("Access-Control-Allow-Origin".to_owned(), "*".to_owned()),
            (
                "Access-Control-Allow-Methods".to_owned(),
                "GET, POST, OPTIONS".to_owned(),
            ),
            ("Access-Control-Allow-Headers".to_owned(), "*".to_owned()),
            ("Access-Control-Max-Age".to_owned(), "86400".to_owned()),
        ]),
        body: Vec::new(),
    }
}

fn extract_scheme_path(url: &str) -> &str {
    if let Some((_, rest)) = url.split_once("://") {
        if let Some(slash) = rest.find('/') {
            return rest[slash..].split(['?', '#']).next().unwrap_or("/");
        }
        return "/";
    }
    url.split(['?', '#']).next().unwrap_or(url)
}

pub(crate) enum IpcRouteAction {
    Immediate(Response),
    AsyncRpc {
        method: String,
        arg: Value,
        call: CallSpec,
    },
}

pub(crate) enum IpcReplyTarget {
    #[cfg(target_os = "macos")]
    WebKit(u64),
    #[cfg(target_os = "macos")]
    WkFallback { webview_id: WebViewId, call_id: i64 },
    #[cfg(feature = "cef")]
    Cef(Arc<Mutex<cef::CefAsyncState>>),
}

pub(crate) struct IpcResponder(Option<IpcReplyTarget>);

impl IpcResponder {
    pub(crate) fn new(target: IpcReplyTarget) -> Self {
        Self(Some(target))
    }

    pub(crate) fn finish(mut self, resp: Response) {
        let Some(target) = self.0.take() else {
            return;
        };
        match target {
            #[cfg(target_os = "macos")]
            IpcReplyTarget::WebKit(token) => {
                macos_webkit::finish_wk_task(token, resp);
            }
            #[cfg(target_os = "macos")]
            IpcReplyTarget::WkFallback {
                webview_id,
                call_id,
            } => {
                let b64 = encode_base64(&resp.body);
                let js = format!(
                    "window.__ipc_wk_rpc && window.__ipc_wk_rpc({call_id}, {});",
                    Value::String(b64).to_json()
                );
                let _ = macos_webkit::eval(webview_id, &js);
            }
            #[cfg(feature = "cef")]
            IpcReplyTarget::Cef(state) => {
                cef::finish_async_rpc(&state, resp);
            }
        }
    }
}

impl Drop for IpcResponder {
    fn drop(&mut self) {
        let Some(target) = self.0.take() else {
            return;
        };
        match target {
            #[cfg(target_os = "macos")]
            IpcReplyTarget::WebKit(token) => {
                if macos_objc::is_main_thread() {
                    macos_webkit::abandon_wk_task(token);
                }
            }
            #[cfg(target_os = "macos")]
            IpcReplyTarget::WkFallback { .. } => {}
            #[cfg(feature = "cef")]
            IpcReplyTarget::Cef(state) => cef::cancel_async_rpc(&state),
        }
    }
}

pub(crate) enum IncomingIpcJob {
    Send {
        id: WebViewId,
        packet: ClientSendPacket,
    },
    Rpc {
        id: WebViewId,
        method: String,
        arg: Value,
        call: CallSpec,
        responder: IpcResponder,
    },
}

pub(crate) struct IpcEndpoint {
    id: WebViewId,
    runtime: Weak<Runtime>,
    outbound: Mutex<VecDeque<Vec<u8>>>,
}

impl IpcEndpoint {
    fn new(id: WebViewId, runtime: Weak<Runtime>) -> Self {
        Self {
            id,
            runtime,
            outbound: Mutex::new(VecDeque::new()),
        }
    }

    pub(crate) fn push_outbound(&self, frame: Vec<u8>) -> bool {
        let mut q = self.outbound.lock().unwrap_or_else(|e| e.into_inner());
        let was_empty = q.is_empty();
        if q.len() >= MAX_QUEUED_EVENTS {
            q.pop_front();
        }
        q.push_back(frame);
        was_empty
    }

    pub(crate) fn drain_outbound_batch(&self) -> Vec<u8> {
        let frames: Vec<Vec<u8>> = {
            let mut q = self.outbound.lock().unwrap_or_else(|e| e.into_inner());
            q.drain(..).collect()
        };
        encode_event_batch(&frames)
    }

    pub(crate) fn route_request(
        &self,
        url: &str,
        http_method: &str,
        body: &[u8],
    ) -> IpcRouteAction {
        if http_method.eq_ignore_ascii_case("OPTIONS") {
            return IpcRouteAction::Immediate(ipc_cors_preflight_response());
        }
        if !http_method.eq_ignore_ascii_case("POST") {
            return IpcRouteAction::Immediate(ipc_binary_response(405, Vec::new()));
        }
        match extract_scheme_path(url) {
            "/drain" => {
                IpcRouteAction::Immediate(ipc_binary_response(200, self.drain_outbound_batch()))
            }
            "/send" => match decode_send_packet(body) {
                Ok(packet) => {
                    self.enqueue_job(IncomingIpcJob::Send {
                        id: self.id,
                        packet,
                    });
                    IpcRouteAction::Immediate(ipc_binary_response(200, Vec::new()))
                }
                Err(_) => IpcRouteAction::Immediate(ipc_binary_response(400, Vec::new())),
            },
            "/rpc" => match decode_rpc_request(body) {
                Ok((method, arg, call)) => IpcRouteAction::AsyncRpc { method, arg, call },
                Err(err) => IpcRouteAction::Immediate(ipc_binary_response(
                    200,
                    encode_rpc_response(Err(err.to_string())),
                )),
            },
            _ => IpcRouteAction::Immediate(ipc_binary_response(404, Vec::new())),
        }
    }

    pub(crate) fn enqueue_rpc(
        &self,
        method: String,
        arg: Value,
        call: CallSpec,
        responder: IpcResponder,
    ) -> std::result::Result<(), IpcResponder> {
        let Some(rt) = self.runtime.upgrade() else {
            return Err(responder);
        };
        if let Ok(mut q) = rt.ipc_jobs.lock() {
            q.push_back(IncomingIpcJob::Rpc {
                id: self.id,
                method,
                arg,
                call,
                responder,
            });
        }
        rt.wake();
        Ok(())
    }

    fn enqueue_job(&self, job: IncomingIpcJob) {
        if let Some(rt) = self.runtime.upgrade() {
            if let Ok(mut q) = rt.ipc_jobs.lock() {
                q.push_back(job);
            }
            rt.wake();
        }
    }
}

struct WebViewDispatchRecord {
    window_id: WindowId,
    endpoint: Arc<IpcEndpoint>,
    rpcs: BTreeMap<String, RpcHandler>,
    events: BTreeMap<String, EventHandler>,
    msg_handlers: Vec<EventHandler>,
    on_event: Option<WebViewEventHandler>,
    /// Last page zoom set through `set_zoom` or the View menu.
    zoom: f64,
}

/// Page zoom steps for the View menu's Zoom In / Zoom Out, as browsers use.
const ZOOM_STEPS: [f64; 13] = [
    0.5, 0.67, 0.75, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0,
];

pub(crate) fn next_zoom_step(current: f64, zoom_in: bool) -> f64 {
    const EPS: f64 = 1e-3;
    if zoom_in {
        ZOOM_STEPS
            .iter()
            .copied()
            .find(|z| *z > current + EPS)
            .unwrap_or(ZOOM_STEPS[ZOOM_STEPS.len() - 1])
    } else {
        ZOOM_STEPS
            .iter()
            .rev()
            .copied()
            .find(|z| *z < current - EPS)
            .unwrap_or(ZOOM_STEPS[0])
    }
}

/// Built-in page calls handled by the runtime, ahead of app RPCs.
const BUILTIN_MENU_SET: &str = "ui:menu.set";
const BUILTIN_CHROME: &str = "ui:chrome";
const BUILTIN_SET_THEME: &str = "ui:window.setTheme";
const BUILTIN_CONTEXT_MENU: &str = "ui:contextMenu";
/// Event the shim turns into `--ui-*` CSS variables and `data-ui-*` attributes.
const CHROME_EVENT: &str = "__ui_chrome";

type AppEventSlot = Mutex<Option<Box<dyn FnMut(Event) + Send>>>;

pub(crate) struct Runtime {
    engine: EngineKind,
    main_thread: ThreadId,
    next_win_id: AtomicU64,
    next_wv_id: AtomicU64,
    wake_posted: AtomicBool,
    queue: Mutex<VecDeque<Box<dyn FnOnce() + Send>>>,
    ipc_jobs: Mutex<VecDeque<IncomingIpcJob>>,
    app_event: AppEventSlot,
    webviews: Mutex<HashMap<WebViewId, WebViewDispatchRecord>>,
    /// Calls running on worker threads, keyed by webview and page call id.
    calls: Mutex<HashMap<(WebViewId, u32), Arc<CallState>>>,
}

struct WebViewDispatchSnapshot {
    window_id: WindowId,
    rpcs: BTreeMap<String, RpcHandler>,
    events: BTreeMap<String, EventHandler>,
    msg_handlers: Vec<EventHandler>,
}

impl Runtime {
    fn new(engine: EngineKind) -> Arc<Self> {
        Arc::new(Self {
            engine,
            main_thread: thread::current().id(),
            next_win_id: AtomicU64::new(1),
            next_wv_id: AtomicU64::new(1),
            wake_posted: AtomicBool::new(false),
            queue: Mutex::new(VecDeque::new()),
            ipc_jobs: Mutex::new(VecDeque::new()),
            app_event: Mutex::new(None),
            webviews: Mutex::new(HashMap::new()),
            calls: Mutex::new(HashMap::new()),
        })
    }

    pub fn backend_name(&self) -> &'static str {
        self.engine.name()
    }

    pub fn is_main(&self) -> bool {
        thread::current().id() == self.main_thread
    }

    pub fn wake(&self) {
        #[cfg(target_os = "macos")]
        macos_objc::wake_app_loop(&self.wake_posted);
    }

    pub fn dispatch_on_main(&self, f: Box<dyn FnOnce() + Send>) {
        if let Ok(mut q) = self.queue.lock() {
            q.push_back(f);
        }
        self.wake();
    }

    pub(crate) fn call_main_sync<T: Send + 'static>(&self, f: impl FnOnce() -> T + Send + 'static) -> T {
        if self.is_main() {
            return f();
        }
        let (tx, rx) = mpsc::sync_channel(1);
        self.dispatch_on_main(Box::new(move || {
            let _ = tx.send(f());
        }));
        rx.recv()
            .expect("UI main thread exited while waiting on synchronous handle call")
    }

    fn drain_queue(&self) {
        loop {
            let task = self.queue.lock().ok().and_then(|mut q| q.pop_front());
            let Some(task) = task else {
                break;
            };
            task();
        }
    }

    fn emit_app_event(&self, ev: Event) {
        if let Ok(mut g) = self.app_event.lock()
            && let Some(ref mut f) = *g
        {
            f(ev);
        }
    }

    pub fn set_app_event_handler(&self, f: Box<dyn FnMut(Event) + Send>) {
        if let Ok(mut g) = self.app_event.lock() {
            *g = Some(f);
        }
    }

    pub fn quit(self: &Arc<Self>) {
        self.dispatch_on_main(Box::new(|| {
            #[cfg(target_os = "macos")]
            macos_appkit::request_quit();
        }));
    }

    pub fn notify(&self, title: String, body: String) {
        #[cfg(target_os = "macos")]
        macos_appkit::notify(&title, &body);
        #[cfg(not(target_os = "macos"))]
        let _ = (title, body);
    }

    pub fn open_external(&self, url: &str) -> Result<()> {
        #[cfg(target_os = "macos")]
        return macos_appkit::open_external(url);
        #[cfg(not(target_os = "macos"))]
        {
            let _ = url;
            Err(Error::Unsupported(
                "open_external not implemented on this OS yet",
            ))
        }
    }

    pub fn app_info(self: &Arc<Self>) -> crate::app::AppInfo {
        self.call_main_sync(|| {
            #[cfg(target_os = "macos")]
            {
                crate::app::AppInfo {
                    name: macos_appkit::app_name(),
                    version: macos_appkit::bundle_version(),
                }
            }
            #[cfg(not(target_os = "macos"))]
            {
                crate::app::AppInfo {
                    name: std::env::current_exe()
                        .ok()
                        .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
                        .unwrap_or_default(),
                    version: None,
                }
            }
        })
    }

    pub fn popup_menu(
        self: &Arc<Self>,
        win_id: WindowId,
        items: Vec<ContextMenuItem>,
        x: f64,
        y: f64,
    ) -> Option<String> {
        self.call_main_sync(move || {
            #[cfg(target_os = "macos")]
            {
                macos_appkit::popup_context_menu(win_id, &items, x, y)
            }
            #[cfg(not(target_os = "macos"))]
            {
                let _ = (win_id, items, x, y);
                None
            }
        })
    }

    pub fn clipboard_text(self: &Arc<Self>) -> Option<String> {
        self.call_main_sync(|| {
            #[cfg(target_os = "macos")]
            {
                macos_appkit::clipboard_text()
            }
            #[cfg(not(target_os = "macos"))]
            {
                None
            }
        })
    }

    pub fn set_clipboard_text(self: &Arc<Self>, text: String) {
        self.call_main_sync(move || {
            #[cfg(target_os = "macos")]
            macos_appkit::set_clipboard_text(&text);
            #[cfg(not(target_os = "macos"))]
            let _ = text;
        });
    }

    pub fn app_theme(self: &Arc<Self>) -> Theme {
        self.call_main_sync(|| {
            #[cfg(target_os = "macos")]
            {
                macos_appkit::app_theme()
            }
            #[cfg(not(target_os = "macos"))]
            {
                Theme::Light
            }
        })
    }

    pub fn displays(self: &Arc<Self>) -> Vec<Display> {
        self.call_main_sync(|| {
            #[cfg(target_os = "macos")]
            {
                macos_appkit::displays()
            }
            #[cfg(not(target_os = "macos"))]
            {
                Vec::new()
            }
        })
    }

    pub fn update_menu_item(self: &Arc<Self>, id: String, update: MenuItemUpdate) {
        let f = move || {
            #[cfg(target_os = "macos")]
            macos_appkit::update_menu_item(&id, &update);
            #[cfg(not(target_os = "macos"))]
            let _ = (&id, &update);
        };
        if self.is_main() {
            f();
        } else {
            self.dispatch_on_main(Box::new(f));
        }
    }

    /// Webviews in the window that has keyboard focus.
    fn focused_webviews(&self) -> Vec<WebViewId> {
        #[cfg(target_os = "macos")]
        let focused = macos_appkit::key_window_id();
        #[cfg(not(target_os = "macos"))]
        let focused: Option<WindowId> = None;
        let Some(win) = focused else {
            return Vec::new();
        };
        let mut ids: Vec<WebViewId> = self
            .webviews
            .lock()
            .map(|g| {
                g.iter()
                    .filter(|(_, r)| r.window_id == win)
                    .map(|(id, _)| *id)
                    .collect()
            })
            .unwrap_or_default();
        ids.sort();
        ids
    }

    #[cfg(target_os = "macos")]
    fn handle_menu_command(self: &Arc<Self>, cmd: macos_appkit::MenuCommand) {
        use macos_appkit::MenuCommand;
        let targets = self.focused_webviews();
        match cmd {
            MenuCommand::Emit(id) => {
                for wv in &targets {
                    self.webview_emit(*wv, format!("menu:{id}"), Value::Null);
                }
                self.emit_app_event(Event::Menu(id));
            }
            MenuCommand::Role(role) => {
                let Some(&wv) = targets.first() else {
                    return;
                };
                match role {
                    Role::Reload => self.webview_reload(wv),
                    Role::Back => self.webview_go_back(wv),
                    Role::Forward => self.webview_go_forward(wv),
                    Role::ActualSize => self.webview_set_zoom(wv, 1.0),
                    Role::ZoomIn | Role::ZoomOut => {
                        let next = next_zoom_step(self.webview_zoom(wv), role == Role::ZoomIn);
                        self.webview_set_zoom(wv, next);
                    }
                    Role::ToggleDevTools => {
                        let _ = self.webview_open_devtools(wv);
                    }
                    _ => {}
                }
            }
        }
    }

    pub fn set_menu(self: &Arc<Self>, menu: Menu) {
        if self.is_main() {
            #[cfg(target_os = "macos")]
            macos_appkit::set_app_menu(&menu);
        } else {
            self.dispatch_on_main(Box::new(move || {
                #[cfg(target_os = "macos")]
                macos_appkit::set_app_menu(&menu);
            }));
        }
    }

    pub fn open_window(self: &Arc<Self>, spec: WindowSpec) -> Result<Window> {
        if !self.is_main() {
            return Err(Error::InvalidState(
                "Window::open() must be called on the UI thread (or inside ui::run / app.dispatch)"
                    .into(),
            ));
        }
        let id = WindowId(self.next_win_id.fetch_add(1, Ordering::Relaxed));
        #[cfg(target_os = "macos")]
        macos_appkit::open_ns_window(id, &spec)?;
        Ok(Window {
            id,
            runtime: self.clone(),
        })
    }

    pub fn set_window_title(self: &Arc<Self>, id: WindowId, title: String) {
        if self.is_main() {
            #[cfg(target_os = "macos")]
            macos_appkit::set_window_title(id, &title);
        } else {
            self.dispatch_on_main(Box::new(move || {
                #[cfg(target_os = "macos")]
                macos_appkit::set_window_title(id, &title);
            }));
        }
    }

    pub fn window_title(self: &Arc<Self>, id: WindowId) -> String {
        self.call_main_sync(move || {
            #[cfg(target_os = "macos")]
            {
                macos_appkit::window_title(id)
            }
            #[cfg(not(target_os = "macos"))]
            {
                String::new()
            }
        })
    }

    pub fn set_window_size(self: &Arc<Self>, id: WindowId, width: u32, height: u32) {
        if self.is_main() {
            #[cfg(target_os = "macos")]
            macos_appkit::set_window_size(id, width, height);
        } else {
            self.dispatch_on_main(Box::new(move || {
                #[cfg(target_os = "macos")]
                macos_appkit::set_window_size(id, width, height);
            }));
        }
    }

    pub fn window_bounds(self: &Arc<Self>, id: WindowId) -> Bounds {
        self.call_main_sync(move || {
            #[cfg(target_os = "macos")]
            {
                macos_appkit::window_bounds(id)
            }
            #[cfg(not(target_os = "macos"))]
            {
                Bounds::default()
            }
        })
    }

    pub fn set_window_visible(self: &Arc<Self>, id: WindowId, visible: bool) {
        if self.is_main() {
            #[cfg(target_os = "macos")]
            macos_appkit::set_window_visible(id, visible);
        } else {
            self.dispatch_on_main(Box::new(move || {
                #[cfg(target_os = "macos")]
                macos_appkit::set_window_visible(id, visible);
            }));
        }
    }

    pub fn focus_window(self: &Arc<Self>, id: WindowId) {
        if self.is_main() {
            #[cfg(target_os = "macos")]
            macos_appkit::focus_window(id);
        } else {
            self.dispatch_on_main(Box::new(move || {
                #[cfg(target_os = "macos")]
                macos_appkit::focus_window(id);
            }));
        }
    }

    pub fn minimize_window(self: &Arc<Self>, id: WindowId) {
        if self.is_main() {
            #[cfg(target_os = "macos")]
            macos_appkit::minimize_window(id);
        } else {
            self.dispatch_on_main(Box::new(move || {
                #[cfg(target_os = "macos")]
                macos_appkit::minimize_window(id);
            }));
        }
    }

    pub fn maximize_window(self: &Arc<Self>, id: WindowId) {
        if self.is_main() {
            #[cfg(target_os = "macos")]
            macos_appkit::maximize_window(id);
        } else {
            self.dispatch_on_main(Box::new(move || {
                #[cfg(target_os = "macos")]
                macos_appkit::maximize_window(id);
            }));
        }
    }

    pub fn set_window_fullscreen(self: &Arc<Self>, id: WindowId, fs: bool) {
        if self.is_main() {
            #[cfg(target_os = "macos")]
            macos_appkit::set_window_fullscreen(id, fs);
        } else {
            self.dispatch_on_main(Box::new(move || {
                #[cfg(target_os = "macos")]
                macos_appkit::set_window_fullscreen(id, fs);
            }));
        }
    }

    pub fn window_state(self: &Arc<Self>, id: WindowId) -> WindowState {
        self.call_main_sync(move || {
            #[cfg(target_os = "macos")]
            {
                macos_appkit::window_state(id)
            }
            #[cfg(not(target_os = "macos"))]
            {
                WindowState::Normal
            }
        })
    }

    pub fn window_theme(self: &Arc<Self>, id: WindowId) -> Theme {
        self.call_main_sync(move || {
            #[cfg(target_os = "macos")]
            {
                macos_appkit::window_theme(id)
            }
            #[cfg(not(target_os = "macos"))]
            {
                let _ = id;
                Theme::Light
            }
        })
    }

    pub fn set_window_theme(self: &Arc<Self>, id: WindowId, theme: Option<Theme>) {
        let rt = Arc::clone(self);
        let f = move || {
            #[cfg(target_os = "macos")]
            macos_appkit::set_window_theme(id, theme);
            #[cfg(not(target_os = "macos"))]
            let _ = theme;
            rt.push_chrome(id);
        };
        if self.is_main() {
            f();
        } else {
            self.dispatch_on_main(Box::new(f));
        }
    }

    /// Title-bar layout and window state for the shim's `--ui-*` CSS variables.
    fn chrome_value(&self, id: WindowId) -> Value {
        let platform = if cfg!(target_os = "macos") {
            "macos"
        } else if cfg!(windows) {
            "windows"
        } else {
            "linux"
        };
        let mut m = BTreeMap::from([("platform".to_owned(), Value::String(platform.into()))]);
        #[cfg(target_os = "macos")]
        if let Some(c) = macos_appkit::window_chrome(id) {
            m.insert("titlebar_height".into(), Value::Float(c.titlebar_height));
            m.insert("controls_left".into(), Value::Float(c.controls_left));
            m.insert("controls_right".into(), Value::Float(0.0));
            m.insert("fullscreen".into(), Value::Bool(c.fullscreen));
            m.insert("focused".into(), Value::Bool(c.focused));
            let theme = match macos_appkit::window_theme(id) {
                Theme::Dark => "dark",
                Theme::Light => "light",
            };
            m.insert("theme".into(), Value::String(theme.into()));
        }
        #[cfg(not(target_os = "macos"))]
        let _ = id;
        Value::Object(m)
    }

    /// Sends the window's current chrome to each of its webviews.
    pub(crate) fn push_chrome(self: &Arc<Self>, win_id: WindowId) {
        let ids: Vec<WebViewId> = self
            .webviews
            .lock()
            .map(|g| {
                g.iter()
                    .filter(|(_, r)| r.window_id == win_id)
                    .map(|(id, _)| *id)
                    .collect()
            })
            .unwrap_or_default();
        if ids.is_empty() {
            return;
        }
        let chrome = self.chrome_value(win_id);
        for id in ids {
            self.webview_emit(id, CHROME_EVENT.to_owned(), chrome.clone());
        }
    }

    pub fn begin_window_drag(self: &Arc<Self>, id: WindowId) {
        if self.is_main() {
            #[cfg(target_os = "macos")]
            macos_appkit::begin_window_drag(id);
        } else {
            self.dispatch_on_main(Box::new(move || {
                #[cfg(target_os = "macos")]
                macos_appkit::begin_window_drag(id);
            }));
        }
    }

    pub fn close_window(self: &Arc<Self>, id: WindowId) {
        let engine = self.engine;
        let rt = self.clone();
        let close_fn = move || {
            rt.cleanup_webviews_for_window(id);
            #[cfg(target_os = "macos")]
            macos_appkit::close_window(id);
            let _ = engine;
        };
        if self.is_main() {
            close_fn();
        } else {
            self.dispatch_on_main(Box::new(close_fn));
        }
    }

    fn cleanup_webviews_for_window(&self, win_id: WindowId) {
        let mut removed = Vec::new();
        if let Ok(mut g) = self.webviews.lock() {
            g.retain(|id, rec| {
                let keep = rec.window_id != win_id;
                if !keep {
                    removed.push(*id);
                }
                keep
            });
        }
        if let Ok(mut calls) = self.calls.lock() {
            calls.retain(|(wv, _), state| {
                let keep = !removed.contains(wv);
                if !keep {
                    state.cancel();
                }
                keep
            });
        }
        match self.engine {
            #[cfg(target_os = "macos")]
            EngineKind::WebKit => macos_webkit::cleanup_webviews_for_window(win_id),
            #[cfg(feature = "cef")]
            EngineKind::Cef => cef::cleanup_webviews_for_window(win_id),
        }
    }

    pub fn raw_window_handle(self: &Arc<Self>, id: WindowId) -> RawWindowHandle {
        self.call_main_sync(move || {
            #[cfg(target_os = "macos")]
            {
                if let Some((ns_window, content_view, _)) = macos_appkit::window_ns_ids(id) {
                    return RawWindowHandle::AppKit {
                        ns_window,
                        content_view,
                    };
                }
            }
            RawWindowHandle::Unknown
        })
    }

    pub fn run_file_dialog(
        self: &Arc<Self>,
        win_id: WindowId,
        kind: FileDialogKind,
        spec: FileDialogSpec,
    ) -> Vec<PathBuf> {
        let rt = self.clone();
        self.call_main_sync(move || {
            #[cfg(target_os = "macos")]
            {
                macos_appkit::run_file_dialog(win_id, kind, &spec, &rt.wake_posted)
            }
            #[cfg(not(target_os = "macos"))]
            {
                let _ = (win_id, kind, spec, rt);
                Vec::new()
            }
        })
    }

    pub fn run_message_dialog(
        self: &Arc<Self>,
        win_id: WindowId,
        spec: MessageDialogSpec,
    ) -> MessageResult {
        let rt = self.clone();
        self.call_main_sync(move || {
            #[cfg(target_os = "macos")]
            {
                macos_appkit::run_message_dialog(win_id, &spec, &rt.wake_posted)
            }
            #[cfg(not(target_os = "macos"))]
            {
                let _ = (win_id, spec, rt);
                MessageResult::Cancel
            }
        })
    }

    pub fn mount_webview(self: &Arc<Self>, win_id: WindowId, spec: WebViewSpec) -> Result<WebView> {
        if !self.is_main() {
            return Err(Error::InvalidState(
                "WebView::mount() must be called on the UI thread".into(),
            ));
        }
        let id = WebViewId(self.next_wv_id.fetch_add(1, Ordering::Relaxed));
        let endpoint = Arc::new(IpcEndpoint::new(id, Arc::downgrade(self)));
        if let Ok(mut g) = self.webviews.lock() {
            g.insert(
                id,
                WebViewDispatchRecord {
                    window_id: win_id,
                    endpoint: endpoint.clone(),
                    rpcs: spec.rpcs.clone(),
                    events: spec.events.clone(),
                    msg_handlers: spec.msg_handlers.clone(),
                    on_event: spec.on_event.clone(),
                    zoom: 1.0,
                },
            );
        }

        let res = match self.engine {
            #[cfg(target_os = "macos")]
            EngineKind::WebKit => macos_webkit::mount_webkit_webview(id, win_id, spec, endpoint),
            #[cfg(feature = "cef")]
            EngineKind::Cef => cef::mount_cef_webview(id, win_id, spec, endpoint),
        };
        if let Err(err) = res {
            if let Ok(mut g) = self.webviews.lock() {
                g.remove(&id);
            }
            return Err(err);
        }

        Ok(WebView {
            id,
            window: Window {
                id: win_id,
                runtime: self.clone(),
            },
            runtime: self.clone(),
        })
    }

    pub fn webview_load_url(self: &Arc<Self>, id: WebViewId, url: String) {
        let engine = self.engine;
        let f = move || match engine {
            #[cfg(target_os = "macos")]
            EngineKind::WebKit => macos_webkit::load_url(id, &url),
            #[cfg(feature = "cef")]
            EngineKind::Cef => cef::load_url(id, &url),
        };
        if self.is_main() {
            f();
        } else {
            self.dispatch_on_main(Box::new(f));
        }
    }

    pub fn webview_load_html(self: &Arc<Self>, id: WebViewId, html: String, base: Option<String>) {
        let engine = self.engine;
        let f = move || match engine {
            #[cfg(target_os = "macos")]
            EngineKind::WebKit => macos_webkit::load_html(id, &html, base.as_deref()),
            #[cfg(feature = "cef")]
            EngineKind::Cef => cef::load_html(id, &html, base.as_deref()),
        };
        if self.is_main() {
            f();
        } else {
            self.dispatch_on_main(Box::new(f));
        }
    }

    pub fn webview_reload(self: &Arc<Self>, id: WebViewId) {
        let engine = self.engine;
        let f = move || match engine {
            #[cfg(target_os = "macos")]
            EngineKind::WebKit => macos_webkit::reload(id),
            #[cfg(feature = "cef")]
            EngineKind::Cef => cef::reload(id),
        };
        if self.is_main() {
            f();
        } else {
            self.dispatch_on_main(Box::new(f));
        }
    }

    pub fn webview_go_back(self: &Arc<Self>, id: WebViewId) {
        let engine = self.engine;
        let f = move || match engine {
            #[cfg(target_os = "macos")]
            EngineKind::WebKit => macos_webkit::go_back(id),
            #[cfg(feature = "cef")]
            EngineKind::Cef => cef::go_back(id),
        };
        if self.is_main() {
            f();
        } else {
            self.dispatch_on_main(Box::new(f));
        }
    }

    pub fn webview_go_forward(self: &Arc<Self>, id: WebViewId) {
        let engine = self.engine;
        let f = move || match engine {
            #[cfg(target_os = "macos")]
            EngineKind::WebKit => macos_webkit::go_forward(id),
            #[cfg(feature = "cef")]
            EngineKind::Cef => cef::go_forward(id),
        };
        if self.is_main() {
            f();
        } else {
            self.dispatch_on_main(Box::new(f));
        }
    }

    pub fn webview_zoom(&self, id: WebViewId) -> f64 {
        self.webviews
            .lock()
            .ok()
            .and_then(|g| g.get(&id).map(|r| r.zoom))
            .unwrap_or(1.0)
    }

    pub fn webview_set_zoom(self: &Arc<Self>, id: WebViewId, zoom: f64) {
        if let Ok(mut g) = self.webviews.lock()
            && let Some(rec) = g.get_mut(&id)
        {
            rec.zoom = zoom;
        }
        let engine = self.engine;
        let f = move || match engine {
            #[cfg(target_os = "macos")]
            EngineKind::WebKit => macos_webkit::set_zoom(id, zoom),
            #[cfg(feature = "cef")]
            EngineKind::Cef => cef::set_zoom(id, zoom),
        };
        if self.is_main() {
            f();
        } else {
            self.dispatch_on_main(Box::new(f));
        }
    }

    pub fn webview_set_inspectable(self: &Arc<Self>, id: WebViewId, inspectable: bool) {
        let engine = self.engine;
        let f = move || match engine {
            #[cfg(target_os = "macos")]
            EngineKind::WebKit => macos_webkit::set_inspectable(id, inspectable),
            #[cfg(feature = "cef")]
            EngineKind::Cef => cef::set_inspectable(id, inspectable),
        };
        if self.is_main() {
            f();
        } else {
            self.dispatch_on_main(Box::new(f));
        }
    }

    pub fn webview_open_devtools(self: &Arc<Self>, _id: WebViewId) -> Result<()> {
        let engine = self.engine;
        let id = _id;
        self.call_main_sync(move || match engine {
            #[cfg(target_os = "macos")]
            EngineKind::WebKit => {
                let _ = id;
                Err(Error::Unsupported("open_devtools() is unsupported on webkit; use inspectable(true) with Safari Web Inspector"))
            }
            #[cfg(feature = "cef")]
            EngineKind::Cef => cef::open_devtools(id),
        })
    }

    pub fn webview_eval(self: &Arc<Self>, id: WebViewId, js: String) -> Result<()> {
        let engine = self.engine;
        self.call_main_sync(move || match engine {
            #[cfg(target_os = "macos")]
            EngineKind::WebKit => macos_webkit::eval(id, &js),
            #[cfg(feature = "cef")]
            EngineKind::Cef => cef::eval(id, &js),
        })
    }

    pub fn webview_emit(self: &Arc<Self>, id: WebViewId, event: String, payload: Value) {
        let frame = encode_event_frame(&event, &payload);
        let endpoint = self
            .webviews
            .lock()
            .ok()
            .and_then(|g| g.get(&id).map(|r| r.endpoint.clone()));
        let Some(endpoint) = endpoint else {
            return;
        };
        if !endpoint.push_outbound(frame) {
            return;
        }
        let engine = self.engine;
        let wake = move || match engine {
            #[cfg(target_os = "macos")]
            EngineKind::WebKit => {
                if macos_webkit::is_https_url(id) {
                    let batch = endpoint.drain_outbound_batch();
                    if !batch.is_empty() {
                        let b64 = Value::String(encode_base64(&batch)).to_json();
                        let js =
                            format!("window.__ipc_deliver_b64 && window.__ipc_deliver_b64({b64});");
                        let _ = macos_webkit::eval(id, &js);
                    }
                } else {
                    let _ = macos_webkit::eval(id, "window.__ipc_wake && window.__ipc_wake();");
                }
            }
            #[cfg(feature = "cef")]
            EngineKind::Cef => {
                let _ = &endpoint;
                let _ = cef::eval(id, "window.__ipc_wake && window.__ipc_wake();");
            }
        };
        if self.is_main() {
            wake();
        } else {
            self.dispatch_on_main(Box::new(wake));
        }
    }

    pub fn webview_set_visible(self: &Arc<Self>, id: WebViewId, visible: bool) {
        let engine = self.engine;
        let f = move || match engine {
            #[cfg(target_os = "macos")]
            EngineKind::WebKit => macos_webkit::set_visible(id, visible),
            #[cfg(feature = "cef")]
            EngineKind::Cef => cef::set_visible(id, visible),
        };
        if self.is_main() {
            f();
        } else {
            self.dispatch_on_main(Box::new(f));
        }
    }

    pub fn webview_focus(self: &Arc<Self>, id: WebViewId) {
        let engine = self.engine;
        let f = move || match engine {
            #[cfg(target_os = "macos")]
            EngineKind::WebKit => macos_webkit::focus(id),
            #[cfg(feature = "cef")]
            EngineKind::Cef => cef::focus(id),
        };
        if self.is_main() {
            f();
        } else {
            self.dispatch_on_main(Box::new(f));
        }
    }

    pub fn webview_clear_data(self: &Arc<Self>, id: WebViewId) {
        let engine = self.engine;
        let f = move || match engine {
            #[cfg(target_os = "macos")]
            EngineKind::WebKit => macos_webkit::clear_data(id),
            #[cfg(feature = "cef")]
            EngineKind::Cef => cef::clear_data(id),
        };
        if self.is_main() {
            f();
        } else {
            self.dispatch_on_main(Box::new(f));
        }
    }

    pub fn webview_load_state(self: &Arc<Self>, id: WebViewId) -> LoadState {
        let engine = self.engine;
        self.call_main_sync(move || match engine {
            #[cfg(target_os = "macos")]
            EngineKind::WebKit => macos_webkit::snapshot_for(id)
                .map(|s| s.0)
                .unwrap_or(LoadState::Started),
            #[cfg(feature = "cef")]
            EngineKind::Cef => cef::snapshot_for(id)
                .map(|s| s.0)
                .unwrap_or(LoadState::Started),
        })
    }

    pub fn webview_url(self: &Arc<Self>, id: WebViewId) -> String {
        let engine = self.engine;
        self.call_main_sync(move || match engine {
            #[cfg(target_os = "macos")]
            EngineKind::WebKit => macos_webkit::snapshot_for(id)
                .map(|s| s.1)
                .unwrap_or_default(),
            #[cfg(feature = "cef")]
            EngineKind::Cef => cef::snapshot_for(id).map(|s| s.1).unwrap_or_default(),
        })
    }

    pub fn webview_title(self: &Arc<Self>, id: WebViewId) -> String {
        let engine = self.engine;
        self.call_main_sync(move || match engine {
            #[cfg(target_os = "macos")]
            EngineKind::WebKit => macos_webkit::snapshot_for(id)
                .map(|s| s.2)
                .unwrap_or_default(),
            #[cfg(feature = "cef")]
            EngineKind::Cef => cef::snapshot_for(id).map(|s| s.2).unwrap_or_default(),
        })
    }

    pub fn raw_webview_handle(self: &Arc<Self>, id: WebViewId) -> RawWebViewHandle {
        let engine = self.engine;
        self.call_main_sync(move || match engine {
            #[cfg(target_os = "macos")]
            EngineKind::WebKit => RawWebViewHandle::WkWebView {
                wk_webview: macos_webkit::wk_ptr_for(id),
            },
            #[cfg(feature = "cef")]
            EngineKind::Cef => RawWebViewHandle::Cef {
                browser_id: cef::cef_browser_id_for(id),
            },
        })
    }

    fn dispatch_info(&self, id: WebViewId) -> Option<WebViewDispatchSnapshot> {
        self.webviews.lock().ok().and_then(|g| {
            g.get(&id).map(|r| WebViewDispatchSnapshot {
                window_id: r.window_id,
                rpcs: r.rpcs.clone(),
                events: r.events.clone(),
                msg_handlers: r.msg_handlers.clone(),
            })
        })
    }

    fn make_ctx(self: &Arc<Self>, win_id: WindowId, id: WebViewId) -> Ctx {
        let win = Window {
            id: win_id,
            runtime: self.clone(),
        };
        Ctx {
            window: win.clone(),
            webview: WebView {
                id,
                window: win,
                runtime: self.clone(),
            },
            app: App {
                runtime: self.clone(),
            },
            call: None,
        }
    }

    pub(crate) fn drain_ipc_jobs(self: &Arc<Self>) {
        loop {
            let job = self.ipc_jobs.lock().ok().and_then(|mut q| q.pop_front());
            let Some(job) = job else { break };
            match job {
                IncomingIpcJob::Send { id, packet } => self.handle_send_packet(id, packet),
                IncomingIpcJob::Rpc {
                    id,
                    method,
                    arg,
                    call,
                    responder,
                } => {
                    self.handle_rpc_job(id, method, arg, call, responder);
                }
            }
        }
    }

    fn handle_send_packet(self: &Arc<Self>, id: WebViewId, packet: ClientSendPacket) {
        let Some(snap) = self.dispatch_info(id) else {
            return;
        };
        let ctx = self.make_ctx(snap.window_id, id);
        match packet {
            ClientSendPacket::Drag => {
                self.begin_window_drag(snap.window_id);
            }
            ClientSendPacket::TitlebarDoubleClick => {
                // Zoom or minimize, per "Double-click a window's title bar to…".
                #[cfg(target_os = "macos")]
                macos_appkit::titlebar_double_click(snap.window_id);
            }
            ClientSendPacket::Cancel(call_id) => {
                if let Some(state) = self.call_state(id, call_id) {
                    state.cancel();
                }
            }
            ClientSendPacket::StreamData(call_id, data) => {
                if let Some(state) = self.call_state(id, call_id) {
                    state.push_inbound(data);
                }
            }
            ClientSendPacket::Event(name, data) => {
                if let Some(handler) = snap.events.get(&name) {
                    if let Ok(mut guard) = handler.lock() {
                        guard(ctx, data);
                    }
                } else if !snap.msg_handlers.is_empty() && !name.is_empty() {
                    let mut obj = match data {
                        Value::Object(m) => m,
                        Value::Null => BTreeMap::new(),
                        other => BTreeMap::from([("value".to_owned(), other)]),
                    };
                    obj.entry("type".to_owned())
                        .or_insert_with(|| Value::String(name));
                    let wrapped = Value::Object(obj);
                    for h in snap.msg_handlers {
                        if let Ok(mut guard) = h.lock() {
                            guard(ctx.clone(), wrapped.clone());
                        }
                    }
                }
            }
            ClientSendPacket::Msg(data) => {
                for h in snap.msg_handlers {
                    if let Ok(mut guard) = h.lock() {
                        guard(ctx.clone(), data.clone());
                    }
                }
            }
        }
    }

    fn call_state(&self, id: WebViewId, call_id: u32) -> Option<Arc<CallState>> {
        self.calls
            .lock()
            .ok()
            .and_then(|g| g.get(&(id, call_id)).cloned())
    }

    fn handle_rpc_job(
        self: &Arc<Self>,
        id: WebViewId,
        method: String,
        arg: Value,
        call: CallSpec,
        responder: IpcResponder,
    ) {
        let reply = |responder: IpcResponder, result: std::result::Result<Value, String>| {
            responder.finish(ipc_binary_response(200, encode_rpc_response(result)));
        };
        let Some(snap) = self.dispatch_info(id) else {
            reply(responder, Err("webview closed".to_owned()));
            return;
        };
        if method == BUILTIN_MENU_SET {
            let update = || -> std::result::Result<Value, String> {
                let id = arg
                    .get("id")
                    .and_then(Value::as_str)
                    .ok_or("ipc.menu.set needs an \"id\"")?
                    .to_owned();
                let update = MenuItemUpdate {
                    title: arg.get("title").and_then(Value::as_str).map(str::to_owned),
                    enabled: arg.get("enabled").and_then(Value::as_bool),
                    checked: arg.get("checked").and_then(Value::as_bool),
                };
                #[cfg(target_os = "macos")]
                let found = macos_appkit::update_menu_item(&id, &update);
                #[cfg(not(target_os = "macos"))]
                let found = {
                    let _ = update;
                    false
                };
                Ok(Value::Bool(found))
            };
            return reply(responder, update());
        }
        if method == BUILTIN_CONTEXT_MENU {
            let items = match arg.get("items").map(ContextMenuItem::from_page) {
                Some(Ok(items)) => items,
                Some(Err(e)) => return reply(responder, Err(e)),
                None => return reply(responder, Err("contextMenu needs items".into())),
            };
            // Page CSS pixels to window points.
            let zoom = self.webview_zoom(id);
            let coord = |k: &str| {
                arg.get(k)
                    .and_then(|v| v.as_f64().or_else(|| v.as_i64().map(|i| i as f64)))
                    .unwrap_or(0.0)
                    * zoom
            };
            let (x, y) = (coord("x"), coord("y"));
            let chosen = self.popup_menu(snap.window_id, items, x, y);
            return reply(responder, Ok(chosen.map_or(Value::Null, Value::String)));
        }
        if method == BUILTIN_CHROME {
            return reply(responder, Ok(self.chrome_value(snap.window_id)));
        }
        if method == BUILTIN_SET_THEME {
            let theme = match arg.as_str() {
                Some("light") => Some(Theme::Light),
                Some("dark") => Some(Theme::Dark),
                Some("system") | None => None,
                Some(other) => {
                    return reply(
                        responder,
                        Err(format!("unknown theme \"{other}\"; use light, dark or system")),
                    );
                }
            };
            self.set_window_theme(snap.window_id, theme);
            return reply(responder, Ok(Value::Null));
        }
        let Some(handler) = snap.rpcs.get(&method) else {
            reply(responder, Err(format!("unknown RPC method: {method}")));
            return;
        };
        match (handler.kind, call.stream) {
            (RpcKind::Stream, false) => {
                return reply(
                    responder,
                    Err(format!("\"{method}\" is a stream; open it with ipc.stream()")),
                );
            }
            (RpcKind::Main | RpcKind::Blocking, true) => {
                return reply(
                    responder,
                    Err(format!("\"{method}\" is not a stream; call it with ipc.invoke()")),
                );
            }
            _ => {}
        }
        let mut ctx = self.make_ctx(snap.window_id, id);
        let state = call.id.map(|call_id| (call_id, Arc::new(CallState::new())));
        ctx.call = state.clone();
        let outcome = match handler.func.lock() {
            Ok(mut f) => f(ctx, arg),
            Err(_) => RpcOutcome::Ready(Err("RPC handler mutex poisoned".to_owned())),
        };
        let job = match outcome {
            RpcOutcome::Ready(result) => return reply(responder, result),
            RpcOutcome::Worker(job) => job,
        };

        let key = state.as_ref().map(|(call_id, _)| (id, *call_id));
        if let (Some(key), Some((_, st))) = (key, &state)
            && let Ok(mut calls) = self.calls.lock()
        {
            calls.insert(key, Arc::clone(st));
        }
        if handler.kind == RpcKind::Stream
            && let Some((_, call_id)) = key
        {
            self.webview_emit(id, format!("{STREAM_OPEN_PREFIX}{call_id}"), Value::Null);
        }
        let rt = Arc::clone(self);
        // The responder is finished on the UI thread: WebKit scheme tasks live there.
        let finish = move |responder: IpcResponder, result: std::result::Result<Value, String>| {
            let main_rt = Arc::clone(&rt);
            rt.dispatch_on_main(Box::new(move || {
                if let Some(key) = key
                    && let Ok(mut calls) = main_rt.calls.lock()
                {
                    calls.remove(&key);
                }
                responder.finish(ipc_binary_response(200, encode_rpc_response(result)));
            }));
        };
        // `spawn` hands the closure back only on success, so keep the responder
        // reachable from here for the failure path.
        let slot = Arc::new(Mutex::new(Some((responder, job))));
        let worker_slot = Arc::clone(&slot);
        let worker_finish = finish.clone();
        let spawned = thread::Builder::new()
            .name(format!("ui-invoke:{method}"))
            .spawn(move || {
                let Some((responder, job)) =
                    worker_slot.lock().unwrap_or_else(|e| e.into_inner()).take()
                else {
                    return;
                };
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job))
                    .unwrap_or_else(|_| Err(format!("\"{method}\" handler panicked")));
                worker_finish(responder, result);
            });
        if let Err(err) = spawned
            && let Some((responder, _)) = slot.lock().unwrap_or_else(|e| e.into_inner()).take()
        {
            finish(responder, Err(format!("could not start worker thread: {err}")));
        }
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn handle_wk_fallback_message(self: &Arc<Self>, id: WebViewId, raw: &str) {
        let Ok(val) = parse(raw) else {
            return;
        };
        let kind = val.get("kind").and_then(Value::as_str).unwrap_or("");
        let data_b64 = val.get("data").and_then(Value::as_str).unwrap_or("");
        let Ok(bytes) = decode_base64(data_b64) else {
            return;
        };
        match kind {
            "send" => {
                if let Ok(packet) = decode_send_packet(&bytes) {
                    self.handle_send_packet(id, packet);
                }
            }
            "rpc" => {
                let call_id = val.get("id").and_then(Value::as_i64).unwrap_or(0);
                let responder = IpcResponder::new(IpcReplyTarget::WkFallback {
                    webview_id: id,
                    call_id,
                });
                match decode_rpc_request(&bytes) {
                    Ok((method, arg, call)) => {
                        self.handle_rpc_job(id, method, arg, call, responder)
                    }
                    Err(err) => responder.finish(ipc_binary_response(
                        200,
                        encode_rpc_response(Err(err.to_string())),
                    )),
                }
            }
            _ => {}
        }
    }

    pub(crate) fn handle_webview_event(self: &Arc<Self>, id: WebViewId, ev: WebViewEvent) {
        let handler = self
            .webviews
            .lock()
            .ok()
            .and_then(|g| g.get(&id).and_then(|r| r.on_event.clone()));
        if let Some(h) = handler
            && let Ok(mut guard) = h.lock()
        {
            guard(ev.clone());
        }
        self.emit_app_event(Event::WebView(id, ev));
    }
}

pub(crate) fn run<F>(init: F) -> Result<()>
where
    F: FnOnce(&App) -> Result<()>,
{
    let engine = EngineKind::resolve();

    #[cfg(feature = "cef")]
    if matches!(engine, EngineKind::Cef)
        && let Some(exit_code) = cef::maybe_execute_subprocess()?
    {
        std::process::exit(exit_code);
    }

    #[cfg(target_os = "macos")]
    {
        if !macos_objc::is_main_thread() {
            return Err(Error::InvalidState(
                "ui::run() must be called on the main thread".into(),
            ));
        }

        let runtime = Runtime::new(engine);
        let app_handle = App {
            runtime: runtime.clone(),
        };

        #[cfg(feature = "cef")]
        let with_cef = matches!(engine, EngineKind::Cef);
        #[cfg(not(feature = "cef"))]
        let with_cef = false;

        let ns_app = macos_objc::pooled(|| macos_appkit::init_ns_app(with_cef))?;

        #[cfg(feature = "cef")]
        if matches!(engine, EngineKind::Cef) {
            cef::initialize()?;
        }

        let init_res = macos_objc::pooled(|| init(&app_handle));
        #[cfg(not(feature = "cef"))]
        init_res?;
        #[cfg(feature = "cef")]
        if let Err(err) = init_res {
            if matches!(engine, EngineKind::Cef) {
                cef::shutdown();
            }
            return Err(err);
        }

        let default_mode = unsafe {
            // SAFETY: Constant Foundation NSString.
            macos_objc::NSDefaultRunLoopMode
        };

        while !macos_appkit::should_quit_flag() {
            macos_objc::pooled(|| {
                let wait = if with_cef {
                    Duration::from_millis(8)
                } else {
                    Duration::from_millis(25)
                };
                macos_objc::pump_once(ns_app, wait, default_mode, &runtime.wake_posted, |ev| {
                    macos_appkit::record_event_if_mouse_down(ev);
                    false
                });

                #[cfg(feature = "cef")]
                if with_cef {
                    cef::do_message_loop_work();
                }

                runtime.drain_queue();
                runtime.drain_ipc_jobs();

                for cmd in macos_appkit::drain_menu_commands() {
                    runtime.handle_menu_command(cmd);
                }

                macos_appkit::drain_queued_events(|ev| {
                    match &ev {
                        Event::Window(win_id, WindowEvent::Closed) => {
                            runtime.cleanup_webviews_for_window(*win_id);
                        }
                        Event::Window(
                            win_id,
                            WindowEvent::Focused(_)
                            | WindowEvent::FullscreenChanged(_)
                            | WindowEvent::ThemeChanged(_),
                        ) => runtime.push_chrome(*win_id),
                        _ => {}
                    }
                    runtime.emit_app_event(ev);
                });

                for notif in macos_webkit::drain_notifications() {
                    match notif {
                        macos_webkit::WebKitNotification::WkFallback(wv_id, raw) => {
                            runtime.handle_wk_fallback_message(wv_id, &raw);
                        }
                        macos_webkit::WebKitNotification::Event(wv_id, ev) => {
                            runtime.handle_webview_event(wv_id, ev);
                        }
                    }
                }
                runtime.drain_ipc_jobs();

                #[cfg(feature = "cef")]
                if with_cef {
                    for notif in cef::drain_notifications() {
                        let cef::CefNotification::Event(wv_id, ev) = notif;
                        runtime.handle_webview_event(wv_id, ev);
                    }
                }
            });
        }

        #[cfg(feature = "cef")]
        if with_cef {
            cef::shutdown();
        }
        Ok(())
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = (init, engine);
        Err(Error::Unsupported(
            "native host window loop not yet implemented for this OS",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::next_zoom_step;

    #[test]
    fn zoom_steps_move_one_step_and_clamp() {
        assert_eq!(next_zoom_step(1.0, true), 1.1);
        assert_eq!(next_zoom_step(1.0, false), 0.9);
        // Off-grid values snap to the next step in that direction.
        assert_eq!(next_zoom_step(1.05, true), 1.1);
        assert_eq!(next_zoom_step(1.05, false), 1.0);
        assert_eq!(next_zoom_step(3.0, true), 3.0);
        assert_eq!(next_zoom_step(0.5, false), 0.5);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn titlebar_double_click_follows_system_setting() {
        use super::macos_appkit::{TitlebarAction, titlebar_action_for};
        assert_eq!(titlebar_action_for(None, false), TitlebarAction::Zoom);
        assert_eq!(titlebar_action_for(None, true), TitlebarAction::Minimize);
        assert_eq!(titlebar_action_for(Some("Maximize"), true), TitlebarAction::Zoom);
        assert_eq!(titlebar_action_for(Some("Fill"), false), TitlebarAction::Zoom);
        assert_eq!(titlebar_action_for(Some("Minimize"), false), TitlebarAction::Minimize);
        assert_eq!(titlebar_action_for(Some("None"), false), TitlebarAction::None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn role_titles_include_the_app_name() {
        use super::macos_appkit::role_title_with_app;
        use crate::menu::Role;
        assert_eq!(role_title_with_app(Role::Quit, "Quit", "Notes").as_deref(), Some("Quit Notes"));
        assert_eq!(role_title_with_app(Role::Help, "Help", "Notes").as_deref(), Some("Notes Help"));
        // Custom titles are left alone.
        assert_eq!(role_title_with_app(Role::Quit, "Exit", "Notes"), None);
        assert_eq!(role_title_with_app(Role::Copy, "Copy", "Notes"), None);
    }
}
