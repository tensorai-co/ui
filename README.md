# ui

**A pure-`std` Rust framework for building native desktop applications with web technologies.**

`ui` provides native application windows, menus, dialogs, single-instance locking, embedded webviews, custom `app://` asset serving, and low-overhead binary and JSON IPC between Rust and JavaScript — with zero third-party Rust crate dependencies.

---

## Installation

Add `ui` to your `Cargo.toml`:

```toml
[dependencies]
ui = { git = "https://github.com/tensorai-co/ui" }

# Optional: enable the Chromium Embedded Framework (CEF) backend
# ui = { git = "https://github.com/tensorai-co/ui", features = ["cef"] }
```

To install the application bundler CLI (`ui-pack`):

```sh
cargo install --git https://github.com/tensorai-co/ui ui-pack
```

---

## Quickstart

Applications share one Rust and JavaScript API across `ui::run`, windows, dialogs, menus, custom schemes, and IPC. Complete runnable examples live in [crates/ui/examples/web_bundle.rs](crates/ui/examples/web_bundle.rs) and [crates/ui/examples/url_app.rs](crates/ui/examples/url_app.rs).

### Single-Page Web Bundle (`app://localhost/`)

```rust
use ui::{Backdrop, Bytes, Request, Response, TitleBar, WebView, Window, json};

fn serve(req: &Request) -> Response {
    match req.path() {
        "/" | "/index.html" => Response::asset("index.html", b"<!doctype html>..."),
        "/app.mjs" => Response::asset("app.mjs", b"export {};"),
        _ => Response::not_found(),
    }
}

fn main() -> ui::Result<()> {
    ui::run(|app| {
        // Menu::standard() is active by default so Cmd+C/V/X/A/Z work out of the box.
        let win = Window::new("My App")
            .size(1280, 840)
            .min_size(760, 520)
            .title_bar(TitleBar::inset(38).traffic_lights(14, 12))
            .backdrop(Backdrop::Sidebar)
            .open(app)?;

        WebView::new()
            .assets("app", serve) // serves app://localhost/index.html with MIME inference
            .inspectable(cfg!(debug_assertions))
            .rpc("open_project", |ctx, ()| ctx.window().dialog().pick_folder())
            .rpc("transform", |_ctx, (name, raw): (String, Bytes)| {
                let flipped: Vec<u8> = raw.0.into_iter().map(|b| !b).collect();
                (name, Bytes(flipped))
            })
            .on("ready", |ctx, ()| {
                ctx.emit("app_info", json!({ "version": "0.1.0" }));
            })
            .mount(&win)?;

        Ok(())
    })
}
```

### Hosted URL App & Typed Commands

```rust
use ui::{Nav, Profile, Tabbing, TitleBar, WebView, Window};

ui::ipc_enum! {
    enum AppCmd {
        #[tag = "set_title"]
        SetTitle { title: String },
        #[tag = "notify"]
        Notify { title: String, body: String },
    }
}

fn main() -> ui::Result<()> {
    ui::run(|app| {
        let win = Window::new("Example App")
            .size(1200, 800)
            .title_bar(TitleBar::Native)
            .tabbing(Tabbing::Disallowed)
            .open(app)?;

        WebView::url("https://example.com")
            .profile(Profile::persistent("default"))
            .on_navigate(|req| match req.host() {
                Some("example.com") => Nav::Allow,
                Some(_) => Nav::External,
                None => Nav::Allow,
            })
            .on_popup(|_req| Nav::External)
            .on_msg::<AppCmd>(|ctx, cmd| match cmd {
                AppCmd::SetTitle { title } => ctx.window().set_title(&title),
                AppCmd::Notify { title, body } => ctx.app().notify(&title, &body),
            })
            .mount(&win)?;

        Ok(())
    })
}
```

### JavaScript IPC (`window.ipc`)

Every webview initializes a frozen `window.ipc` (`ipc`) global alongside `data-ui-drag` / `-webkit-app-region: drag` window drag support (`no-drag` descendants opt back out):

```js
// Async request/response RPC (JSON, raw binary, or mixed tuples)
const folder = await ipc.invoke("open_project");
const [name, outBytes] = await ipc.invoke("transform", "buf", new Uint8Array([1, 2, 3]));
// outBytes is a zero-offset Uint8Array backed by the decoded binary payload

// Fire-and-forget renderer -> host messages
ipc.send("ready");
ipc.send("upload", arrayBufferOrUint8Array);
ipc.send({ type: "set_title", title: "Updated Title" }); // dispatches to on_msg::<AppCmd>

// Host -> renderer event subscriptions
const off = ipc.on("app_info", (info) => console.log(info.version));
ipc.once("frame", (bytes) => ctxDraw(bytes.buffer));
ipc.off("app_info", handler); // or off() / ipc.removeListener(...) / ipc.removeAllListeners(...)
```

---

## Run & Test

```sh
# Default macOS build (WebKit)
cargo run --example web_bundle
cargo test --workspace

# Optional Chromium Embedded Framework (CEF) feature
cargo run --features cef --example web_bundle
cargo run --features cef --example url_app
cargo test -p ui --test conformance --features cef

# Runtime backend selection when compiled with --features cef
UI_BACKEND=webkit cargo test -p ui --test conformance --features cef
```

---

## API Overview

Root `ui::*` re-exports the core surface (`run`, `App`, `SingleInstance`, `Window`, `WebView`, `Ctx`, `TitleBar`, `Backdrop`, `Tabbing`, `Profile`, `Nav`, `NavRequest`, `Request`, `Response`, `Menu`, `Dialog`, `Bytes`, `Value`, `DecodeError`, `FromIpc`, `IntoIpc`, `RpcResult`, `Error`, `Result`, `json!`, `ipc_enum!`), organized into focused submodules:

```text
ui
├── app        // run, App, SingleInstance
├── window     // Window, WindowBuilder, WindowId, TitleBar, Backdrop, Tabbing, Bounds, Display, Theme, WindowState
├── webview    // WebView, WebViewBuilder, WebViewId, Ctx, Request, Response, Mime, Nav, NavRequest, Profile, LoadState
├── ipc        // Bytes, Value, FromIpc, IntoIpc, DecodeError, RpcResult (+ json!, ipc_enum!)
├── menu       // Menu, Submenu, MenuItem, Role, Shortcut, Mod
├── dialog     // Dialog, FileDialog, MessageDialog, Filter, MessageLevel, MessageButtons, MessageResult
├── event      // Event, WindowEvent, WebViewEvent
├── platform   // RawWindowHandle, RawWebViewHandle
└── error      // Error, Result
```

- **Thread-safe handles:** `App`, `Window`, and `WebView` are `Clone + Send + Sync` and dispatch safely onto the UI thread when invoked from worker threads.
- **Typed & binary codecs:** `FromIpc` / `IntoIpc` support primitives, `String`, `Bytes` / `&[u8]` / `&[u8; N]`, `Option<T>`, `Vec<T>`, `PathBuf`, `Value`, and tuples up to 4 elements, enabling `.rpc("slice", |_ctx, (path, a, b): (String, usize, usize)| ...)` and `.rpc("frame", |_ctx, buf: Bytes| ...)` directly in standard Rust.
- **Asset MIME detection:** `Response::asset(path, bytes)` maps common web extensions (`html`/`htm`, `js`/`mjs`, `css`, `wasm`, `svg`, `png`, `jpg`/`jpeg`, `gif`, `webp`, `ico`, `woff`, `woff2`, `ttf`, `otf`, `json`/`map`, `txt`, `mp3`, `mp4`, `webm`) via `Mime::from_path`.

---

## Packaging (`ui-pack`)

`ui-pack` reads `[package]` and `[package.metadata.ui]` (`name`, `bundle_id`, `icon`, `backend`, `category`, `min_macos`) from `Cargo.toml`:

```sh
cargo run -p ui-pack -- macos --manifest path/to/Cargo.toml --bin target/release/my-app --out target/pack
```

- **`macos`**: Assembles `<DisplayName>.app`; for `backend = "cef"`, bundles `Chromium Embedded Framework.framework` and helper `.app` processes inside `Contents/Frameworks/` with `CFBundleIdentifier` aligned to `bundle_id`.
- **`mas`**: Builds a sandboxed Mac App Store `.app` + `entitlements.mas.plist` (`com.apple.security.app-sandbox`) using the system webview (`backend = "cef"` is rejected).
- **`msix` / `linux`**: Generates Windows `AppxManifest.xml` or Linux `.desktop` manifests.

---

## Architecture & Platform Support

### IPC Transport Flow (`ui-ipc://ipc/`)

Renderer and host communication runs over the reserved `ui-ipc://ipc/` custom scheme using raw binary `ArrayBuffer` `POST` payloads:

```text
Renderer -> Host
  ipc.invoke(method, arg) ──▶ POST ui-ipc://ipc/rpc   ──▶ UI thread .rpc(..)      ──▶ ArrayBuffer reply
  ipc.send(ev, arg)       ──▶ POST ui-ipc://ipc/send  ──▶ UI thread .on/.on_msg   ──▶ 200 OK

Host -> Renderer (queued + coalesced)
  ctx.emit / wv.emit      ──▶ push frame to per-webview queue ──▶ empty->non-empty: window.__ipc_wake()
                                                                       │
                                                                       ▼
  ipc.on / ipc.once       ◀── dispatch decoded batch          ◀── POST ui-ipc://ipc/drain
```

- **Binary wire framing:** Each payload begins with a 1-byte tag (`0x00` Null, `0x01` UTF-8 JSON, `0x02` raw bytes, `0x03` Array, `0x04` Object). `ui::Bytes` / `&[u8]` values decode as zero-offset `Uint8Array` views without base64 encoding, and mixed tuples/objects pack structured JSON alongside binary buffers.
- **Coalesced event delivery:** Host `emit` only invokes `window.__ipc_wake()` when transitioning a webview's queue from empty to non-empty; burst events drain together in one `/drain` roundtrip. Registering the first JS listener triggers a microtask drain for boot-time events.
- **Reserved scheme & HTTPS fallback:** `"ui-ipc"` cannot be registered via `WebViewBuilder::assets`. On `https://` pages under `webkit` where custom-scheme `fetch` is blocked as mixed content, the runtime automatically sends and receives identical binary frames via `WKScriptMessageHandler`.

### Platform Status

| Crate / Area | macOS (`aarch64`, `x86_64`) | Windows & Linux |
| :--- | :--- | :--- |
| **`webkit` (default on macOS)** | Implemented & verified live (`AppKit` + `WKWebView` via C-ABI `libobjc`) | N/A |
| **`cef` (`--features cef`)** | Implemented & verified live (Spotify CEF `156.0.30`, Alloy `NSView`, multi-process `ui Helper*.app`) | `ui-cef-sys` resolves/verifies archives and generates `CEF_API_HASH`; host windowing & loader stubs return `Unsupported` |
| **`ui-pack`** | `.app` and Mac App Store (`mas`) packaging (`ditto` framework staging + entitlements) | `msix` (`AppxManifest.xml`) and `linux` (`.desktop`) generators |

### Backend Feature Matrix

| Capability | `webkit` (`WKWebView`) | `cef` (Chromium Alloy) | Notes |
| :--- | :--- | :--- | :--- |
| **Lifecycle, bounds, focus** | Child `WKWebView` | Child `CefBrowser` `NSView` | Logical points; host handles resize & DPI |
| **Navigation, HTML & zoom** | `loadRequest` / `loadHTMLString` / `pageZoom` | `load_url` / `data:` HTML / `set_zoom_level` | `back`, `forward`, `reload`, `load_state`, `current_url`, `title` |
| **Nav policy & popups** | Navigation & UI delegates | `on_before_browse`, `on_before_popup` | `Nav::{Allow, Cancel, External}` + `external_links(bool)` |
| **Custom schemes (`app://`)** | `WKURLSchemeHandler` | `cef_register_scheme_handler_factory` | `Request` / `Response` with CORS, headers, and POST bodies |
| **Init scripts & binary IPC** | `WKUserScript` + `WKURLSchemeHandler` (`https://` fallback supported) | `on_context_created` + async `CefResourceHandler` | Shared `window.ipc` shim (`invoke`, `send`, `on`, `once`, `off`, `/drain`) |
| **Storage profiles** | `WKWebsiteDataStore` (`nonPersistent` / UUID) | `cef_request_context_t` (`p_<name>` / incognito) | `Profile::{Default, persistent(name), ephemeral()}`, `clear_data()` |
| **DevTools** | `isInspectable` (macOS 13.3+) | `show_dev_tools` via `open_devtools()` | `open_devtools()` returns `Err(Error::Unsupported)` on `webkit` |
| **Host shell** | `TitleBar::{Native, Inset, Hidden}`, `Backdrop`, `Tabbing`, `Menu::standard()`, sheet `Dialog`, `SingleInstance` | Shared macOS `AppKit` host | Identical window, menu, and dialog API |

### Environment Variables

- `UI_BACKEND`: Set to `webkit` or `cef` when compiled with `--features cef` (defaults to `cef` when the feature is enabled).
- `UI_CEF_DIR`: Path to a pre-extracted Spotify CEF directory containing `include/cef_api_hash.h` (skips downloading to `~/.cache/ui/cef/`).
- `UI_CEF_CACHE_DIR`: Override Chromium `root_cache_path` (defaults under `~/Library/Application Support/ui-cef/<exe>` on macOS, falling back to an isolated temp directory if locked).
- `UI_CEF_SCHEMES`: Extra comma-separated custom scheme names for CEF subprocess registration.
- `UI_CEF_LOG_CONSOLE=1`: Forward CEF JS `console` output to `stderr`.
- `UI_HEADLESS=1`: Run pure-Rust conformance checks without launching a live window/webview.

### Workspace Layout

```text
ui/
├── Cargo.toml                  # Workspace root (edition 2024, resolver 3, zero external crates)
├── LICENSE                     # Apache-2.0 license
└── crates/
    ├── ui/
    │   ├── examples/
    │   │   ├── web_bundle.rs   # Single-page app://localhost/ bundle, inset titlebar, backdrop, RPC/events
    │   │   └── url_app.rs      # Hosted URL app, Profile::persistent, navigation filter, ipc_enum! commands
    │   ├── src/                # Public API + platform backends
    │   └── tests/
    │       └── conformance.rs  # Shared pure-Rust + live window/webview conformance suite (harness = false)
    ├── ui-cef-sys/             # cef.lock + Spotify CEF fetcher, C11 helper bundler, cef_api_hash check, raw C-API FFI
    └── ui-pack/                # .app / MAS entitlements assembler & MSIX / .desktop manifest generators
```

---

## License

Licensed under the [Apache License, Version 2.0](LICENSE).
