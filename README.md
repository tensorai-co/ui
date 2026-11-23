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
            .invoke("open_project", |ctx, ()| ctx.window().dialog().pick_folder())
            .invoke("transform", |_ctx, (name, raw): (String, Bytes)| {
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

### Embedded Web Build & Dev Server

`ui-build` embeds a bundler's output folder at compile time, so there's no hand-written asset table. A complete example lives in [examples/embedded-app](examples/embedded-app).

```toml
[build-dependencies]
ui-build = { git = "https://github.com/tensorai-co/ui" }
```

```rust
// build.rs: runs `npm run build` when dist/ is missing, in release builds, or with UI_BUILD_WEB=1
fn main() { ui_build::web("dist").before("npm run build").emit(); }

// main.rs
WebView::new().bundle("app", ui::embedded!().spa("index.html").immutable("assets/"))
```

- The app opens at `app://localhost/` (the URL stays `/` for client-side routers). With `.spa(..)`, unknown paths that don't look like files serve the SPA page; missing files and paths under `.immutable(..)` folders are 404.
- Files under `.immutable(..)` prefixes get `Cache-Control: public, max-age=31536000, immutable`; the rest get `no-cache`.
- **Dev mode:** in debug builds, `UI_DEV_URL=http://localhost:5173 cargo run` loads the dev server (e.g. Vite with hot reload) in place of the bundle, and IPC keeps working. Only `localhost`, `127.0.0.1` and `[::1]` URLs are accepted.

### Title Bar Layout in CSS & Window Theme

The shim keeps CSS variables and attributes on `<html>` current, so pages don't hard-code window-control offsets:

```css
.toolbar { height: var(--ui-titlebar-height); padding-left: calc(var(--ui-window-controls-left) + 12px); }
html[data-ui-fullscreen='true'] .toolbar { /* controls are hidden; --ui-window-controls-left is 0 */ }
html[data-ui-focused='false'] .sidebar { opacity: 0.7; }
```

Also set: `--ui-window-controls-right`, `data-ui-platform` (`macos` / `windows` / `linux`) and `data-ui-theme` (`light` / `dark`, the window's effective appearance). `win.set_theme(Some(Theme::Light))`, or `ipc.window.setTheme("light" | "dark" | "system")` from the page, makes the window's vibrancy, controls and sheets follow the app's own theme setting instead of the system's.

### Standard Page Calls & `ui-client`

`.standard_rpcs(..)` exposes the calls every app otherwise writes itself, per group: `app` (`info`, `openExternal` for http/https/mailto only), `dialog` (file pickers, message and confirm sheets), `clipboard`, `window` and `notify`.

```rust
WebView::new().standard_rpcs(ui::StandardRpcs::all().version(env!("CARGO_PKG_VERSION")))
// or: StandardRpcs::none().app().dialog()
```

The [`ui-client`](packages/ui-client) npm package (versioned with the crate) gives pages typed access to them and to `window.ipc`. In a plain browser `isNative()` is false and the helpers fall back to web APIs where one exists:

```ts
import { ipc, isNative, platform, ui } from "ui-client";

const folder = await ui.dialog.pickFolder({ title: "Open Project" });
if (await ui.dialog.confirm("Delete file?", "This can't be undone.")) { /* … */ }
await ui.clipboard.writeText("copied");
const choice = await ui.contextMenu([
  { id: "rename", label: "Rename", shortcut: "F2" },
  "-",
  { id: "delete", label: "Delete", danger: true },
]); // native NSMenu at the last pointer press; resolves to the id or null
```

### macOS Extensions (`ui::macos`)

macOS-only features live in `ui::macos`, so using them on another OS is a compile error rather than a silent no-op:

```rust
use ui::macos::{AppExt, Attention};

let dock = app.macos().dock();
dock.set_icon(include_bytes!("../assets/AppIcon.icns"))?; // e.g. for an unbundled `cargo run`
dock.set_badge("2");
dock.bounce(Attention::Informational);
dock.set_menu(vec![MenuItem::new("new_window", "New Window").emit()]);
```

Window options live there too, along with the cross-platform `restore_state`:

```rust
use ui::macos::{ToolbarStyle, WindowBuilderExt, WindowExt, WindowLevel};

let win = Window::new("Editor")
    .title_bar(TitleBar::inset(52))
    .restore_state("main") // frame, monitor and full screen remembered across launches
    .macos(|m| m.toolbar_style(ToolbarStyle::Unified).titlebar_separator(false))
    .open(app)?;
win.macos().set_document_edited(true); // the dot in the close button (also ui.window.setDocumentEdited)
win.macos().set_level(WindowLevel::Floating);
```

`MacWindowOptions` also has `window_controls_visible`, `all_spaces` and `fullscreen_auxiliary`; `ui::macos::forget_window_state(name)` resets a saved layout.

Native context menus are also available from Rust: `window.popup_menu(vec![ContextMenuItem::new("rename", "Rename")], x, y)` returns the chosen id.

### Deep Links, Opened Files & Single Instance

```toml
[package.metadata.ui]
url_schemes = ["myapp"]          # myapp://… links (Info.plist, MSIX protocol, Linux MimeType)
document_types = ["md", "txt"]   # files opened from Finder or dropped on the Dock icon
```

```rust
app.on_event(|ev| match ev {
    Event::OpenUrl(url) => { /* e.g. myapp://signin/callback?code=… */ }
    Event::OpenFiles(paths) => { /* … */ }
    Event::SecondInstance(args) => { /* focus the main window */ }
    _ => {}
});
```

Pages get the same with `ui.app.on("open-url", fn)` / `ui.app.on("open-files", fn)` from `ui-client`, including a link that launched the app before the page loaded. `SingleInstance::acquire` in a second launch returns `Ok(None)` and forwards its arguments; the running app calls `instance.listen(&app)` to receive them as `Event::SecondInstance` (URL arguments also arrive as `Event::OpenUrl`).

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

// Slow calls: a trailing { signal, timeout } is call options, not an argument
const report = await ipc.invoke("export_report", req, { timeout: 600_000, signal });

// Streams: ordered frames from Rust, data back the other way
const term = ipc.stream("term_open", 24, 80);
term.on("data", (bytes) => xterm.write(bytes));
term.send(stdinBytes);
term.close();                       // Rust's Stream::is_closed() turns true
const exitCode = await term.closed; // the handler's return value

// Fire-and-forget renderer -> host messages (`send` is the v1 name for `emit`)
ipc.emit("ready");
ipc.emit("upload", arrayBufferOrUint8Array);
ipc.emit({ type: "set_title", title: "Updated Title" }); // dispatches to on_msg::<AppCmd>

// Window appearance, independent of the system's
await ipc.window.setTheme("dark"); // or "light" / "system"

// Menu items made with .emit(), and live menu updates
ipc.menu.on("toggle_panel", () => togglePanel());
await ipc.menu.set({ id: "toggle_panel", checked: true, enabled: true, title: "Hide Panel" });

// Host -> renderer event subscriptions
const off = ipc.on("app_info", (info) => console.log(info.version));
ipc.once("frame", (bytes) => ctxDraw(bytes.buffer));
ipc.off("app_info", handler); // or off() / ipc.removeListener(...) / ipc.removeAllListeners(...)
```

### Slow Calls, Streams & Typed Structs

`.invoke` handlers run on the UI thread and must return quickly. `.invoke_blocking` runs each call on its own worker thread and resolves the page's promise when it returns; `.invoke_stream` adds a `ui::Stream` for ordered frames in both directions. When the page aborts, times out, or closes the stream, `ctx.is_cancelled()` / `stream.is_closed()` turn true.

```rust
ui::ipc_struct! {
    struct ExportRequest { path: String, format: String, pages: Option<u32> }
}

WebView::new()
    .invoke_blocking("export_report", |ctx, req: ExportRequest| -> ui::Result<String> {
        for page in 0..req.pages.unwrap_or(1) {
            if ctx.is_cancelled() { return Err(ui::Error::InvalidState("cancelled".into())); }
            render(&req.path, page)?;
        }
        Ok(req.path)
    })
    .invoke_stream("term_open", |_ctx, (rows, cols): (u16, u16), stream| -> ui::Result<i32> {
        let mut pty = Pty::spawn(rows, cols)?;
        while !stream.is_closed() {
            if let Some(input) = stream.try_recv() { pty.write(&Bytes::from_ipc(&input)?.0)?; }
            stream.send(Bytes(pty.read_chunk()?))?;  // raw binary frames
        }
        Ok(stream.close_with(pty.exit_code()))        // resolves `term.closed`
    })
```

`ipc_struct!` generates `FromIpc`/`IntoIpc` for a struct sent as a JS object: `Option` fields may be left out, and decode errors name the field (`missing IPC field "path"`).

### Menus

`Menu::standard()` provides App (About, Services, Hide, Quit, with the app's name), Edit, View (Actual Size, Zoom In/Out, Full Screen), Window and Help menus; on macOS the Help menu gets the system search field. Further roles: `Settings`, `Reload`, `Back`, `Forward`, `ToggleDevTools`, `BringAllToFront`, `Help`.

```rust
use ui::menu::{Menu, MenuItem, Shortcut, Submenu};

app.menu(
    Menu::standard()
        .with_settings() // "Settings…" (Cmd+,) emits `menu:settings`
        .insert_submenu(Submenu::new("Go").item(
            MenuItem::new("toggle_panel", "Toggle Panel").shortcut(Shortcut::ctrl("`")).emit(),
        )),
);
app.menu_item("toggle_panel").set_checked(true);
```

An `.emit()` item sends `menu:<id>` to the focused window's webviews (`ipc.menu.on(id, ..)`) and `Event::Menu(id)` to `app.on_event`. Double-clicking a `data-ui-drag` region zooms or minimizes the window, following the macOS "Double-click a window's title bar to…" setting.

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

# ui-client package tests
(cd packages/ui-client && npm test)

# Runtime backend selection when compiled with --features cef
UI_BACKEND=webkit cargo test -p ui --test conformance --features cef
```

---

## API Overview

Root `ui::*` re-exports the core surface (`run`, `App`, `SingleInstance`, `Window`, `WebView`, `Ctx`, `Stream`, `TitleBar`, `Backdrop`, `Tabbing`, `Profile`, `Nav`, `NavRequest`, `Request`, `Response`, `Menu`, `Dialog`, `Bytes`, `Value`, `DecodeError`, `FromIpc`, `IntoIpc`, `RpcResult`, `Error`, `Result`, `json!`, `ipc_enum!`, `ipc_struct!`), organized into focused submodules:

```text
ui
├── app        // run, App, SingleInstance
├── window     // Window, WindowBuilder, WindowId, TitleBar, Backdrop, Tabbing, Bounds, Display, Theme, WindowState
├── webview    // WebView, WebViewBuilder, WebViewId, Ctx, Request, Response, Mime, Nav, NavRequest, Profile, LoadState
├── embed      // Embedded (+ embedded!), served with WebViewBuilder::bundle
├── ipc        // Bytes, Value, FromIpc, IntoIpc, DecodeError, RpcResult (+ json!, ipc_enum!, ipc_struct!)
├── menu       // Menu, Submenu, MenuItem, MenuItemHandle, MenuItemUpdate, ContextMenuItem, Role, Shortcut, Mod
├── standard   // StandardRpcs (ui.app / dialog / clipboard / window / notify page calls)
├── macos      // AppExt, MacApp, Dock, Attention (macOS only)
├── dialog     // Dialog, FileDialog, MessageDialog, Filter, MessageLevel, MessageButtons, MessageResult
├── event      // Event, WindowEvent, WebViewEvent
├── platform   // RawWindowHandle, RawWebViewHandle
└── error      // Error, Result
```

- **Thread-safe handles:** `App`, `Window`, and `WebView` are `Clone + Send + Sync` and dispatch safely onto the UI thread when invoked from worker threads.
- **Typed & binary codecs:** `FromIpc` / `IntoIpc` support primitives, `String`, `Bytes` / `&[u8]` / `&[u8; N]`, `Option<T>`, `Vec<T>`, `PathBuf`, `Value`, tuples up to 4 elements, and structs declared with `ipc_struct!`, enabling `.invoke("slice", |_ctx, (path, a, b): (String, usize, usize)| ...)` and `.invoke("frame", |_ctx, buf: Bytes| ...)` directly in standard Rust. `.rpc` remains as the v1 name for `.invoke`.
- **Asset MIME detection:** `Response::asset(path, bytes)` maps common web extensions (`html`/`htm`, `js`/`mjs`, `css`, `wasm`, `svg`, `png`, `jpg`/`jpeg`, `gif`, `webp`, `ico`, `woff`, `woff2`, `ttf`, `otf`, `json`/`map`, `txt`, `mp3`, `mp4`, `webm`) via `Mime::from_path`.

---

## Packaging (`ui-pack`)

`ui-pack` reads `[package]` and `[package.metadata.ui]` (`name`, `bundle_id`, `icon`, `backend`, `category`, `min_macos`) from `Cargo.toml`:

```sh
cargo run -p ui-pack -- macos --manifest path/to/Cargo.toml --bin target/release/my-app --out target/pack [--sign "Developer ID Application: …"]
cargo run -p ui-pack -- run --manifest path/to/Cargo.toml [--release] [--dev]   # build, sign with entitlements, launch
cargo run -p ui-pack -- icons assets/app-icon.png                              # AppIcon.iconset + AppIcon.icns
```

`[package.metadata.ui]` also takes `url_schemes`, `document_types`, `entitlements = "app.entitlements"` (used to sign `macos` bundles and `run` binaries, and as the Mac App Store entitlements), and `dev_command` / `dev_url` for `ui-pack run --dev`, which starts the web dev server and sets `UI_DEV_URL`. An `icon` ending in `.icns` is copied into the bundle.

- **`macos`**: Assembles `<DisplayName>.app`; for `backend = "cef"`, bundles `Chromium Embedded Framework.framework` and helper `.app` processes inside `Contents/Frameworks/` with `CFBundleIdentifier` aligned to `bundle_id`.
- **`mac-app-store`** (alias `mas`): Builds a sandboxed Mac App Store `.app` + `entitlements.mas.plist` (`com.apple.security.app-sandbox`) using the system webview (`backend = "cef"` is rejected).
- **`msix` / `linux`**: Generates Windows `AppxManifest.xml` or Linux `.desktop` manifests.

---

## Architecture & Platform Support

### IPC Transport Flow (`ui-ipc://ipc/`)

Renderer and host communication runs over the reserved `ui-ipc://ipc/` custom scheme using raw binary `ArrayBuffer` `POST` payloads:

```text
Renderer -> Host
  ipc.invoke(method, arg) ──▶ POST ui-ipc://ipc/rpc   ──▶ UI thread .rpc(..)      ──▶ ArrayBuffer reply
  ipc.invoke(.., {signal}) ─▶ POST ui-ipc://ipc/rpc (+call id) ─▶ worker .invoke_blocking ─▶ reply via UI thread
  ipc.stream(method, ..)  ──▶ POST ui-ipc://ipc/rpc (+call id, stream) ─▶ worker .invoke_stream ─▶ frames on __ui_stream:<id>
  ipc.emit(ev, arg)       ──▶ POST ui-ipc://ipc/send  ──▶ UI thread .on/.on_msg   ──▶ 200 OK

Host -> Renderer (queued + coalesced)
  ctx.emit / wv.emit      ──▶ push frame to per-webview queue ──▶ empty->non-empty: window.__ipc_wake()
                                                                       │
                                                                       ▼
  ipc.on / ipc.once       ◀── dispatch decoded batch          ◀── POST ui-ipc://ipc/drain
```

- **Calls with an id:** an RPC packet whose flags byte sets bit 0 carries a `u32` call id (bit 1 marks a stream). `/send` opcodes `0x05` (cancel) and `0x06` (stream data) address that id. Stream frames and the final `{ ok }` / `{ error }` frame travel through the same event queue, so they arrive in order.
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
- `UI_DEV_URL`: In debug builds, load this `localhost` dev server in place of a webview's `assets`/`bundle` start page.
- `UI_BUILD_WEB`: For `ui-build`, `1` always runs the `before` command and `0` never does.

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
    ├── ui-build/               # build.rs helper: embeds a web build for ui::embedded!(), runs the web build
    ├── ui-cef-sys/             # cef.lock + Spotify CEF fetcher, C11 helper bundler, cef_api_hash check, raw C-API FFI
    └── ui-pack/                # .app / MAS entitlements assembler & MSIX / .desktop manifest generators
packages/
└── ui-client/                  # npm package: typed window.ipc, isNative/platform, ui.* standard calls
examples/
└── embedded-app/               # ui-build + ui::embedded!() app, with a live test of the bundle and UI_DEV_URL
```

---

## License

Licensed under the [Apache License, Version 2.0](LICENSE).
