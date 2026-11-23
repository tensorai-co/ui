//! Single-page webview surface (`WKWebView` on macOS default, Spotify CEF on `--features cef`/Win/Linux),
//! custom URL scheme assets, typed IPC wiring, navigation policy, and callback `Ctx`.

use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::app::App;
use crate::backend::Runtime;
use crate::call::{CallState, STREAM_DATA_PREFIX, STREAM_END_PREFIX, Stream};
use crate::event::WebViewEvent;
use crate::ipc::{FromIpc, IntoIpc, RpcResult, Value};
use crate::platform::RawWebViewHandle;
use crate::window::Window;
use crate::{Error, Result};

/// Reserved custom URL scheme used by `ui` for binary IPC (`ui-ipc://ipc/...`).
pub(crate) const IPC_SCHEME: &str = "ui-ipc";

/// Opaque unique webview identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WebViewId(pub u64);

/// WebView navigation lifecycle state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadState {
    Started,
    Committed,
    Finished,
    Failed(String),
}

/// Navigation/popup decision returned from `.on_navigate` or `.on_popup`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Nav {
    #[default]
    Allow,
    Cancel,
    /// Cancels in-webview navigation and opens the URL in the OS default browser.
    External,
}

/// WebView storage and cache profile.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Profile {
    #[default]
    Default,
    Persistent(String),
    Ephemeral,
}

impl Profile {
    pub fn persistent(name: impl Into<String>) -> Self {
        Self::Persistent(name.into())
    }

    pub fn ephemeral() -> Self {
        Self::Ephemeral
    }
}

fn split_url_scheme(url: &str) -> Option<&str> {
    url.split_once("://")
        .map(|(s, _)| s)
        .filter(|s| !s.is_empty())
}

fn split_url_host(url: &str) -> Option<&str> {
    let (_, rest) = url.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = if host_port.starts_with('[') {
        host_port
            .split(']')
            .next()
            .map(|h| h.trim_start_matches('['))
    } else {
        host_port.split(':').next()
    }?;
    if host.is_empty() { None } else { Some(host) }
}

fn split_url_path(url: &str) -> &str {
    if let Some((_, rest)) = url.split_once("://") {
        match rest.find(['/', '?', '#']) {
            Some(idx) if rest.as_bytes()[idx] == b'/' => {
                let after_slash = &rest[idx..];
                after_slash.split(['?', '#']).next().unwrap_or("/")
            }
            _ => "/",
        }
    } else {
        url
    }
}

/// Parsed navigation/popup URL inspection helper passed to `.on_navigate` / `.on_popup`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavRequest {
    url: String,
    main_frame: bool,
}

impl NavRequest {
    pub fn new(url: impl Into<String>, main_frame: bool) -> Self {
        Self {
            url: url.into(),
            main_frame,
        }
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn scheme(&self) -> Option<&str> {
        split_url_scheme(&self.url)
    }

    pub fn host(&self) -> Option<&str> {
        split_url_host(&self.url)
    }

    pub fn path(&self) -> &str {
        split_url_path(&self.url)
    }

    pub fn is_main_frame(&self) -> bool {
        self.main_frame
    }
}

/// Custom-scheme asset request passed to `.assets("app", handler)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    url: String,
    method: String,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

impl Request {
    pub fn new(url: impl Into<String>, method: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            method: method.into(),
            headers: BTreeMap::new(),
            body: Vec::new(),
        }
    }

    pub fn with_headers(mut self, headers: BTreeMap<String, String>) -> Self {
        self.headers = headers;
        self
    }

    pub fn with_body(mut self, body: Vec<u8>) -> Self {
        self.body = body;
        self
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn scheme(&self) -> Option<&str> {
        split_url_scheme(&self.url)
    }

    pub fn host(&self) -> Option<&str> {
        split_url_host(&self.url)
    }

    pub fn path(&self) -> &str {
        split_url_path(&self.url)
    }

    pub fn query(&self) -> Option<&str> {
        let (_, after_q) = self.url.split_once('?')?;
        Some(after_q.split('#').next().unwrap_or(after_q))
    }

    pub fn method(&self) -> &str {
        &self.method
    }

    pub fn headers(&self) -> &BTreeMap<String, String> {
        &self.headers
    }

    pub fn body(&self) -> &[u8] {
        &self.body
    }
}

/// MIME lookup helper for embedded web bundles.
pub struct Mime;

impl Mime {
    pub fn from_path(path: &str) -> &'static str {
        let clean = path.split(['?', '#']).next().unwrap_or(path);
        let ext = Path::new(clean)
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase);
        match ext.as_deref() {
            Some("html" | "htm") => "text/html; charset=utf-8",
            Some("js" | "mjs") => "text/javascript; charset=utf-8",
            Some("css") => "text/css; charset=utf-8",
            Some("wasm") => "application/wasm",
            Some("svg") => "image/svg+xml",
            Some("png") => "image/png",
            Some("jpg" | "jpeg") => "image/jpeg",
            Some("gif") => "image/gif",
            Some("webp") => "image/webp",
            Some("ico") => "image/x-icon",
            Some("woff") => "font/woff",
            Some("woff2") => "font/woff2",
            Some("ttf") => "font/ttf",
            Some("otf") => "font/otf",
            Some("json" | "map") => "application/json; charset=utf-8",
            Some("txt") => "text/plain; charset=utf-8",
            Some("mp3") => "audio/mpeg",
            Some("mp4") => "video/mp4",
            Some("webm") => "video/webm",
            _ => "application/octet-stream",
        }
    }
}

/// Response returned from custom-scheme asset handlers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub mime: String,
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
}

impl Response {
    /// Builds a `200 OK` asset response inferring MIME type from `path`.
    pub fn asset(path: &str, bytes: impl Into<Vec<u8>>) -> Self {
        let mime = Mime::from_path(path).to_owned();
        let mut headers = BTreeMap::new();
        headers.insert("Content-Type".to_owned(), mime.clone());
        Self {
            status: 200,
            mime,
            headers,
            body: bytes.into(),
        }
    }

    pub fn html(html: impl Into<String>) -> Self {
        Self::asset("index.html", html.into().into_bytes())
    }

    pub fn text(text: impl Into<String>) -> Self {
        Self::asset("text.txt", text.into().into_bytes())
    }

    pub fn not_found() -> Self {
        Self {
            status: 404,
            mime: "text/plain; charset=utf-8".to_owned(),
            headers: BTreeMap::from([(
                "Content-Type".to_owned(),
                "text/plain; charset=utf-8".to_owned(),
            )]),
            body: b"404 Not Found".to_vec(),
        }
    }

    pub fn with_status(mut self, status: u16) -> Self {
        self.status = status;
        self
    }

    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        let name = name.into();
        let value = value.into();
        if name.eq_ignore_ascii_case("content-type") {
            self.mime = value.clone();
        }
        self.headers.insert(name, value);
        self
    }

    pub fn mime(&self) -> &str {
        &self.mime
    }
}

/// Context passed to every `.invoke`, `.on`, `.on_msg`, and lifecycle handler.
#[derive(Clone)]
pub struct Ctx {
    pub(crate) window: Window,
    pub(crate) webview: WebView,
    pub(crate) app: App,
    /// Call id and state, for calls the page made with an id (cancellable or streams).
    pub(crate) call: Option<(u32, Arc<CallState>)>,
}

impl Ctx {
    pub fn window(&self) -> Window {
        self.window.clone()
    }

    pub fn webview(&self) -> WebView {
        self.webview.clone()
    }

    pub fn app(&self) -> App {
        self.app.clone()
    }

    pub fn emit(&self, event: &str, payload: impl IntoIpc) {
        self.webview.emit(event, payload);
    }

    /// True once the page gave up on this call: its `AbortSignal` fired, its
    /// `timeout` passed, it closed the stream, or the webview went away.
    ///
    /// Long `invoke_blocking` / `invoke_stream` handlers should check this
    /// between steps and return early. Always false for `.invoke` handlers and
    /// for calls made without `{ signal }` / `{ timeout }`.
    pub fn is_cancelled(&self) -> bool {
        self.call.as_ref().is_some_and(|(_, c)| c.is_cancelled())
    }
}

pub(crate) type AssetHandler = Arc<dyn Fn(&Request) -> Response + Send + Sync>;

/// The dev-server URL from `UI_DEV_URL`, if it is set and points at this machine.
pub(crate) fn dev_url_from(value: Option<&str>) -> Option<String> {
    let url = value?.trim();
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = if let Some(v6) = authority.strip_prefix('[') {
        v6.split(']').next().map(|h| format!("[{h}]"))?
    } else {
        authority.split(':').next().unwrap_or("").to_ascii_lowercase()
    };
    matches!(host.as_str(), "localhost" | "127.0.0.1" | "[::1]").then(|| url.to_owned())
}

/// How a registered RPC runs, checked against how the page called it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RpcKind {
    /// `.invoke`: runs on the UI thread.
    Main,
    /// `.invoke_blocking`: runs on a worker thread.
    Blocking,
    /// `.invoke_stream`: runs on a worker thread with a `Stream`.
    Stream,
}

/// What an RPC handler produced on the UI thread.
pub(crate) enum RpcOutcome {
    /// The reply, computed on the UI thread.
    Ready(std::result::Result<Value, String>),
    /// Work to run on a worker thread; its result is the reply.
    Worker(Box<dyn FnOnce() -> std::result::Result<Value, String> + Send>),
}

type RpcFn = Box<dyn FnMut(Ctx, Value) -> RpcOutcome + Send>;

pub(crate) struct RpcEntry {
    pub kind: RpcKind,
    pub func: Mutex<RpcFn>,
}

pub(crate) type RpcHandler = Arc<RpcEntry>;
pub(crate) type EventHandler = Arc<Mutex<Box<dyn FnMut(Ctx, Value) + Send>>>;
pub(crate) type NavHandler = Arc<Mutex<Box<dyn FnMut(&NavRequest) -> Nav + Send>>>;
pub(crate) type WebViewEventHandler = Arc<Mutex<Box<dyn FnMut(WebViewEvent) + Send>>>;

#[derive(Clone)]
pub(crate) enum InitialContent {
    None,
    Url(String),
    Html {
        html: String,
        base_url: Option<String>,
    },
}

pub(crate) struct WebViewSpec {
    pub initial: InitialContent,
    /// The start page came from `assets(..)`, so `UI_DEV_URL` may replace it.
    pub dev_url_allowed: bool,
    pub scheme_handlers: BTreeMap<String, AssetHandler>,
    pub profile: Profile,
    pub inspectable: bool,
    pub transparent: bool,
    pub user_agent: Option<String>,
    pub init_scripts: Vec<String>,
    pub external_links: bool,
    pub on_navigate: Option<NavHandler>,
    pub on_popup: Option<NavHandler>,
    pub on_event: Option<WebViewEventHandler>,
    pub rpcs: BTreeMap<String, RpcHandler>,
    pub events: BTreeMap<String, EventHandler>,
    pub msg_handlers: Vec<EventHandler>,
}

/// Builder for configuring and mounting a single-page `WebView` into a `Window`.
pub struct WebViewBuilder {
    spec: WebViewSpec,
}

impl WebViewBuilder {
    pub fn new() -> Self {
        Self {
            spec: WebViewSpec {
                initial: InitialContent::None,
                dev_url_allowed: false,
                scheme_handlers: BTreeMap::new(),
                profile: Profile::Default,
                inspectable: false,
                transparent: false,
                user_agent: None,
                init_scripts: Vec::new(),
                external_links: false,
                on_navigate: None,
                on_popup: None,
                on_event: None,
                rpcs: BTreeMap::new(),
                events: BTreeMap::new(),
                msg_handlers: Vec::new(),
            },
        }
    }

    pub fn url(mut self, url: impl Into<String>) -> Self {
        self.spec.initial = InitialContent::Url(url.into());
        self
    }

    pub fn html(mut self, html: impl Into<String>) -> Self {
        self.spec.initial = InitialContent::Html {
            html: html.into(),
            base_url: None,
        };
        self
    }

    pub fn html_with_base(mut self, html: impl Into<String>, base_url: impl Into<String>) -> Self {
        self.spec.initial = InitialContent::Html {
            html: html.into(),
            base_url: Some(base_url.into()),
        };
        self
    }

    /// Registers a custom URL scheme (for example `"app"`) serving `app://localhost/...`
    /// and navigates to `<scheme>://localhost/` if no URL/HTML was explicitly set.
    ///
    /// In debug builds, `UI_DEV_URL=http://localhost:5173` loads that dev server
    /// in place of this start page (IPC keeps working), for hot reload without
    /// rebuilding the bundle. Only `localhost` / `127.0.0.1` / `[::1]` URLs are used.
    pub fn assets<F>(self, scheme: impl Into<String>, handler: F) -> Self
    where
        F: Fn(&Request) -> Response + Send + Sync + 'static,
    {
        self.scheme_handler(scheme.into(), Arc::new(handler))
    }

    /// Serves a web build embedded by `ui-build` from a custom scheme, like
    /// [`assets`](Self::assets) (including `UI_DEV_URL`):
    ///
    /// ```ignore
    /// WebView::new().bundle("app", ui::embedded!().spa("index.html").immutable("assets/"))
    /// ```
    pub fn bundle(self, scheme: impl Into<String>, files: crate::embed::Embedded) -> Self {
        self.scheme_handler(scheme.into(), Arc::new(move |req: &Request| files.serve(req)))
    }

    fn scheme_handler(mut self, scheme: String, handler: AssetHandler) -> Self {
        let scheme = scheme.to_ascii_lowercase();
        if matches!(self.spec.initial, InitialContent::None) {
            self.spec.initial = InitialContent::Url(format!("{scheme}://localhost/"));
            self.spec.dev_url_allowed = true;
        }
        self.spec.scheme_handlers.insert(scheme, handler);
        self
    }

    pub fn profile(mut self, profile: Profile) -> Self {
        self.spec.profile = profile;
        self
    }

    pub fn inspectable(mut self, inspectable: bool) -> Self {
        self.spec.inspectable = inspectable;
        self
    }

    pub fn transparent(mut self, transparent: bool) -> Self {
        self.spec.transparent = transparent;
        self
    }

    pub fn user_agent(mut self, ua: impl Into<String>) -> Self {
        self.spec.user_agent = Some(ua.into());
        self
    }

    pub fn init_script(mut self, script: impl Into<String>) -> Self {
        self.spec.init_scripts.push(script.into());
        self
    }

    pub fn external_links(mut self, enabled: bool) -> Self {
        self.spec.external_links = enabled;
        self
    }

    pub fn on_navigate(mut self, f: impl FnMut(&NavRequest) -> Nav + Send + 'static) -> Self {
        self.spec.on_navigate = Some(Arc::new(Mutex::new(Box::new(f))));
        self
    }

    pub fn on_popup(mut self, f: impl FnMut(&NavRequest) -> Nav + Send + 'static) -> Self {
        self.spec.on_popup = Some(Arc::new(Mutex::new(Box::new(f))));
        self
    }

    pub fn on_event(mut self, f: impl FnMut(WebViewEvent) + Send + 'static) -> Self {
        self.spec.on_event = Some(Arc::new(Mutex::new(Box::new(f))));
        self
    }

    pub fn on_load(mut self, mut f: impl FnMut(LoadState) + Send + 'static) -> Self {
        let prev = self.spec.on_event.take();
        self.spec.on_event = Some(Arc::new(Mutex::new(Box::new(move |ev| {
            if let WebViewEvent::LoadState(ref st) = ev {
                f(st.clone());
            }
            if let Some(ref p) = prev
                && let Ok(mut guard) = p.lock()
            {
                guard(ev);
            }
        }))));
        self
    }

    pub fn on_title(mut self, mut f: impl FnMut(String) + Send + 'static) -> Self {
        let prev = self.spec.on_event.take();
        self.spec.on_event = Some(Arc::new(Mutex::new(Box::new(move |ev| {
            if let WebViewEvent::TitleChanged(ref t) = ev {
                f(t.clone());
            }
            if let Some(ref p) = prev
                && let Ok(mut guard) = p.lock()
            {
                guard(ev);
            }
        }))));
        self
    }

    /// Registers a typed request/response call exposed to JS as `ipc.invoke(name, ...)`.
    ///
    /// The handler runs on the UI thread, so it must return quickly. Use
    /// [`invoke_blocking`](Self::invoke_blocking) for slow work.
    pub fn invoke<A, R, F>(mut self, name: impl Into<String>, mut handler: F) -> Self
    where
        A: FromIpc,
        R: RpcResult,
        F: FnMut(Ctx, A) -> R + Send + 'static,
    {
        let func: RpcFn = Box::new(move |ctx, val| {
            RpcOutcome::Ready(
                A::from_ipc(&val)
                    .map_err(|e| e.to_string())
                    .and_then(|arg| handler(ctx, arg).into_rpc_result()),
            )
        });
        self.insert_rpc(name.into(), RpcKind::Main, func);
        self
    }

    /// Same as [`invoke`](Self::invoke); kept under its v1 name.
    pub fn rpc<A, R, F>(self, name: impl Into<String>, handler: F) -> Self
    where
        A: FromIpc,
        R: RpcResult,
        F: FnMut(Ctx, A) -> R + Send + 'static,
    {
        self.invoke(name, handler)
    }

    /// Registers a call that runs on a worker thread, exposed to JS as
    /// `ipc.invoke(name, ...)`. The page's promise resolves when the handler returns.
    ///
    /// Use it for anything slow (builds, scans, network). Calls run concurrently,
    /// each on its own thread, so the handler is `Fn + Sync`. The page can pass
    /// `{ signal, timeout }` as the last argument; check
    /// [`Ctx::is_cancelled`] to stop early.
    ///
    /// ```ignore
    /// .invoke_blocking("export_report", |ctx, req: ExportRequest| reports.export(&ctx, req))
    /// ```
    pub fn invoke_blocking<A, R, F>(mut self, name: impl Into<String>, handler: F) -> Self
    where
        A: FromIpc + Send + 'static,
        R: RpcResult,
        F: Fn(Ctx, A) -> R + Send + Sync + 'static,
    {
        let handler = Arc::new(handler);
        let func: RpcFn = Box::new(move |ctx, val| match A::from_ipc(&val) {
            Ok(arg) => {
                let handler = Arc::clone(&handler);
                RpcOutcome::Worker(Box::new(move || handler(ctx, arg).into_rpc_result()))
            }
            Err(e) => RpcOutcome::Ready(Err(e.to_string())),
        });
        self.insert_rpc(name.into(), RpcKind::Blocking, func);
        self
    }

    /// Registers a streaming call, opened from JS with `ipc.stream(name, ...)`.
    ///
    /// The handler runs on a worker thread with a [`Stream`]: `stream.send(..)`
    /// delivers frames to the page's `on("data")` listeners in order, and
    /// `stream.recv()` reads what the page sends back. The return value
    /// resolves the page's `stream.closed`.
    ///
    /// ```ignore
    /// .invoke_stream("tail", |_ctx, path: String, stream| {
    ///     for line in follow(&path) {
    ///         if stream.send(line).is_err() { break } // page closed it
    ///     }
    ///     stream.close_with(0)
    /// })
    /// ```
    pub fn invoke_stream<A, R, F>(mut self, name: impl Into<String>, handler: F) -> Self
    where
        A: FromIpc + Send + 'static,
        R: RpcResult,
        F: Fn(Ctx, A, Stream) -> R + Send + Sync + 'static,
    {
        let handler = Arc::new(handler);
        let func: RpcFn = Box::new(move |ctx, val| {
            let Some((id, state)) = ctx.call.clone() else {
                return RpcOutcome::Ready(Err("stream call without a call id".into()));
            };
            let Some(inbound) = state.take_inbound() else {
                return RpcOutcome::Ready(Err("stream already opened".into()));
            };
            let arg = match A::from_ipc(&val) {
                Ok(arg) => arg,
                Err(e) => return RpcOutcome::Ready(Err(e.to_string())),
            };
            let handler = Arc::clone(&handler);
            RpcOutcome::Worker(Box::new(move || {
                let webview = ctx.webview();
                let stream = Stream {
                    webview: webview.clone(),
                    data_channel: format!("{STREAM_DATA_PREFIX}{id}"),
                    state,
                    inbound,
                };
                let end = match handler(ctx, arg, stream).into_rpc_result() {
                    Ok(v) => BTreeMap::from([("ok".to_owned(), v)]),
                    Err(e) => BTreeMap::from([("error".to_owned(), Value::String(e))]),
                };
                // Sent through the same event queue as the data, so it arrives last.
                webview.emit(&format!("{STREAM_END_PREFIX}{id}"), Value::Object(end));
                Ok(Value::Null)
            }))
        });
        self.insert_rpc(name.into(), RpcKind::Stream, func);
        self
    }

    fn insert_rpc(&mut self, name: String, kind: RpcKind, func: RpcFn) {
        self.spec.rpcs.insert(
            name,
            Arc::new(RpcEntry {
                kind,
                func: Mutex::new(func),
            }),
        );
    }

    /// Registers a typed fire-and-forget event listener for `ipc.send(event, ...)`.
    pub fn on<A, F>(mut self, event: impl Into<String>, mut handler: F) -> Self
    where
        A: FromIpc,
        F: FnMut(Ctx, A) + Send + 'static,
    {
        let boxed: EventHandler = Arc::new(Mutex::new(Box::new(move |ctx, val| {
            if let Ok(arg) = A::from_ipc(&val) {
                handler(ctx, arg);
            }
        })));
        self.spec.events.insert(event.into(), boxed);
        self
    }

    /// Registers a strongly typed enum/message handler for `ipc.send({ type: "...", ... })`.
    pub fn on_msg<T: FromIpc>(mut self, mut handler: impl FnMut(Ctx, T) + Send + 'static) -> Self {
        let boxed: EventHandler = Arc::new(Mutex::new(Box::new(move |ctx, val| {
            if let Ok(msg) = T::from_ipc(&val) {
                handler(ctx, msg);
            }
        })));
        self.spec.msg_handlers.push(boxed);
        self
    }

    /// Mounts the webview filling the target window's content area and starts loading.
    pub fn mount(self, win: &Window) -> Result<WebView> {
        if self.spec.scheme_handlers.contains_key(IPC_SCHEME) {
            return Err(Error::InvalidState(format!(
                "custom scheme \"{IPC_SCHEME}\" is reserved for ui binary IPC"
            )));
        }
        let mut spec = self.spec;
        if cfg!(debug_assertions) && spec.dev_url_allowed {
            let env = std::env::var("UI_DEV_URL").ok();
            if let Some(url) = dev_url_from(env.as_deref()) {
                spec.initial = InitialContent::Url(url);
            } else if env.is_some_and(|v| !v.trim().is_empty()) {
                eprintln!("ui: ignoring UI_DEV_URL: only http(s)://localhost URLs are used");
            }
        }
        win.runtime.mount_webview(win.id, spec)
    }
}

impl Default for WebViewBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Cheap, `Clone + Send + Sync` handle to a mounted webview.
#[derive(Clone)]
pub struct WebView {
    pub(crate) id: WebViewId,
    pub(crate) window: Window,
    pub(crate) runtime: Arc<Runtime>,
}

impl WebView {
    /// Starts building an empty `WebView`.
    #[allow(clippy::new_ret_no_self)]
    pub fn new() -> WebViewBuilder {
        WebViewBuilder::new()
    }

    /// Starts building a `WebView` that immediately navigates to `url`.
    pub fn url(url: impl Into<String>) -> WebViewBuilder {
        WebViewBuilder::new().url(url)
    }

    /// Starts building a `WebView` populated with an inline HTML string.
    pub fn html(html: impl Into<String>) -> WebViewBuilder {
        WebViewBuilder::new().html(html)
    }

    pub fn id(&self) -> WebViewId {
        self.id
    }

    pub fn window(&self) -> Window {
        self.window.clone()
    }

    pub fn load_url(&self, url: &str) {
        self.runtime.webview_load_url(self.id, url.to_owned());
    }

    pub fn load_html(&self, html: &str) {
        self.runtime
            .webview_load_html(self.id, html.to_owned(), None);
    }

    pub fn load_html_with_base(&self, html: &str, base_url: &str) {
        self.runtime
            .webview_load_html(self.id, html.to_owned(), Some(base_url.to_owned()));
    }

    pub fn reload(&self) {
        self.runtime.webview_reload(self.id);
    }

    pub fn back(&self) {
        self.runtime.webview_go_back(self.id);
    }

    pub fn forward(&self) {
        self.runtime.webview_go_forward(self.id);
    }

    pub fn set_zoom(&self, zoom: f64) {
        self.runtime.webview_set_zoom(self.id, zoom);
    }

    /// The page zoom last set with `set_zoom` or the View menu (1.0 = 100%).
    pub fn zoom(&self) -> f64 {
        self.runtime.webview_zoom(self.id)
    }

    pub fn set_inspectable(&self, inspectable: bool) {
        self.runtime.webview_set_inspectable(self.id, inspectable);
    }

    pub fn open_devtools(&self) -> Result<()> {
        self.runtime.webview_open_devtools(self.id)
    }

    pub fn emit(&self, event: &str, payload: impl IntoIpc) {
        self.runtime
            .webview_emit(self.id, event.to_owned(), payload.into_ipc());
    }

    pub fn eval(&self, js: &str) -> Result<()> {
        self.runtime.webview_eval(self.id, js.to_owned())
    }

    pub fn set_visible(&self, visible: bool) {
        self.runtime.webview_set_visible(self.id, visible);
    }

    pub fn focus(&self) {
        self.runtime.webview_focus(self.id);
    }

    pub fn clear_data(&self) {
        self.runtime.webview_clear_data(self.id);
    }

    pub fn load_state(&self) -> LoadState {
        self.runtime.webview_load_state(self.id)
    }

    pub fn current_url(&self) -> String {
        self.runtime.webview_url(self.id)
    }

    pub fn title(&self) -> String {
        self.runtime.webview_title(self.id)
    }

    pub fn raw_handle(&self) -> RawWebViewHandle {
        self.runtime.raw_webview_handle(self.id)
    }
}

impl std::fmt::Debug for WebView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebView").field("id", &self.id).finish()
    }
}

impl PartialEq for WebView {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for WebView {}

impl Hash for WebView {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

#[cfg(test)]
mod tests {
    use super::dev_url_from;

    #[test]
    fn dev_url_only_accepts_this_machine() {
        for ok in [
            "http://localhost:5173",
            "http://localhost:5173/app/",
            "https://LOCALHOST",
            "http://127.0.0.1:3000",
            "http://[::1]:8080/",
        ] {
            assert_eq!(dev_url_from(Some(ok)).as_deref(), Some(ok), "{ok}");
        }
        for bad in [
            "",
            "localhost:5173",
            "file:///tmp/index.html",
            "http://example.com",
            "http://localhost.evil.com",
            "http://evil.com/?localhost",
        ] {
            assert_eq!(dev_url_from(Some(bad)), None, "{bad}");
        }
        assert_eq!(dev_url_from(None), None);
    }
}
