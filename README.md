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
            .title_bar(TitleBar::overlay(38).window_controls(WindowControls::at(14, 12)))
            .backdrop(Backdrop::Sidebar)
            .open(app)?;

        WebView::new()
            .assets("app", serve) // serves app://localhost/index.html with MIME inference
            .devtools(cfg!(debug_assertions))
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
    .title_bar(TitleBar::overlay(52))
    .restore_state("main") // frame, monitor and full screen remembered across launches
    .macos(|m| m.toolbar_style(ToolbarStyle::Unified).titlebar_separator(false))
    .open(app)?;
win.macos().set_document_edited(true); // the dot in the close button (also ui.window.setDocumentEdited)
win.macos().set_level(WindowLevel::Floating);
```

`MacWindowOptions` also has `window_controls_visible`, `all_spaces` and `fullscreen_auxiliary`; `ui::macos::forget_window_state(name)` resets a saved layout.

Native context menus are also available from Rust: `window.popup_menu(vec![ContextMenuItem::new("rename", "Rename")], x, y)` returns the chosen id.

### Notifications

```rust
let id = app.show_notification(
    ui::Notification::new("Build finished").body("All 112 tests passed").id("build"),
);
app.on_event(|ev| if let Event::NotificationClicked(id) = ev { /* … */ });
app.remove_notification("build");
```

From an `.app` bundle these use the system notification center, with the app's own icon, clicks (`Event::NotificationClicked`, and `onClick` in `ui.notify(title, body, { onClick })`) and removal; permission is asked the first time (`app.request_notification_permission(..)` / `app.notification_permission(..)` to check without asking). Unbundled `cargo run` binaries fall back to `osascript`, which can't be clicked or removed.

### Webview Extras

`webview.find(text, FindOptions::default(), |found| …)` / `clear_find()` (find in page; CEF can't report matches), `print()`, `pdf(|bytes| …)` (WebKit), and `WebViewEvent::FileDrop { paths, x, y }` with real filesystem paths for dropped files (WebKit; pages also get `ui:file-drop`). Page zoom set with `set_zoom` or the View menu is remembered per profile. With `.standard_rpcs(..)`, pages reach these as `ui.webview.find / clearFind / print / pdf / onFileDrop`. `.mirror_theme()` makes the window's appearance follow the page's `<html data-theme>`.

### Typed Calls in TypeScript

```rust
ui::ipc_types! {
    fn ipc_typescript;
    invoke export_report(req: ExportRequest) -> Report;
    stream term_open(rows: u16, cols: u16) -> i32, data: Bytes;
}

fn main() -> ui::Result<()> {
    ui::write_typescript_if_requested(ipc_typescript); // for `ui-pack gen-ts`
    ui::run(|app| { /* … */ })
}
```

`ui-pack gen-ts --out web/src/ipc.d.ts` writes the `interface`s for every `ipc_struct!` involved plus `IpcInvoke` / `IpcStreams`; then `typed<IpcInvoke, IpcStreams>()` from `ui-client` checks call names, arguments and results at compile time. Types without a TypeScript mapping come out as `unknown`.

### Signed Update Checks

```sh
ui-pack update-key --out keys/          # keys/update-private.pem (keep secret) + the public key to embed
ui-pack update-feed keys/update-private.pem 1.3.0 https://example.com/App-1.3.0.zip "Release notes" > feed.json
```

```rust
if let Some(update) = ui::update::check("https://example.com/feed.json", PUBLIC_KEY, env!("CARGO_PKG_VERSION"))? {
    // update.version, update.url, update.notes — verified against PUBLIC_KEY
}
```

Feeds are ECDSA P-256 signed and verified with the system Security framework (macOS), so there's no crypto dependency; only `https://` feeds are fetched.

### Speech to Text (macOS)

WKWebView has no Web Speech API, so `ui::macos::speech` provides one: `speech::request_permission(..)`, then `speech::listen(Some("en-US"), |t| …)` delivers partial and final transcripts until the session is stopped. Pages use `ui.speech.listen()` (a stream of `{ text, final }`) with `StandardRpcs::none().speech()`. It needs an app bundle with `microphone_usage` and `speech_usage` set in `[package.metadata.ui]` (and the `com.apple.security.device.audio-input` entitlement under the hardened runtime); without them it returns an error instead of letting macOS kill the app.

### Optional `serde` Support

```toml
ui = { git = "https://github.com/tensorai-co/ui", features = ["serde"] }
```

`ui::Serde<T>` makes any `Serialize`/`Deserialize` type an IPC argument or result, converted straight to and from the IPC value tree (no JSON text in between). The default build has no third-party dependencies.

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

### Renamed in 0.2

v1 names still compile as `#[deprecated]` aliases for one release, each pointing at its replacement: `TitleBar::inset(..).traffic_lights(x, y)` → `TitleBar::overlay(..).window_controls(WindowControls::at(x, y))`, `TitleBar::Hidden` → `Frameless`, `Role::{Hide, HideOthers, ShowAll}` → `ui::macos::Role::…`, `Backdrop::{Transient, Mica, Acrylic}` → `Popover` / `Window`, `Role::Zoom` → `Maximize`, `Event::Reopen` → `Activate`, `Shortcut::cmd(_shift)` → `primary(_shift)`, `.inspectable` → `.devtools`, `.rpc` → `.invoke`, `.on_msg` → `.on_command`, `Nav::{Cancel, External}` → `NavDecision::{Block, OpenInBrowser}`, `.on_popup` → `.on_new_window`, `.external_links` → `.open_links_in_browser`, `Display` / `displays()` → `Monitor` / `monitors()`, `Mod` → `Modifiers`, `Ctx` → `Context`, `WindowBuilder::tabbing` → `.macos(|m| m.tabbing(..))`, and in JS `ipc.send` → `ipc.emit`.

### Hosted URL App & Typed Commands

```rust
use ui::macos::{Tabbing, WindowBuilderExt};
use ui::{NavDecision, Profile, TitleBar, WebView, Window};

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
            .macos(|m| m.tabbing(Tabbing::Disallowed))
            .open(app)?;

        WebView::url("https://example.com")
            .profile(Profile::persistent("default"))
            .on_navigate(|req| match req.host() {
                Some("example.com") => NavDecision::Allow,
                Some(_) => NavDecision::OpenInBrowser,
                None => NavDecision::Allow,
            })
            .on_new_window(|_req| NavDecision::OpenInBrowser)
            .on_command::<AppCmd>(|ctx, cmd| match cmd {
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
ipc.emit({ type: "set_title", title: "Updated Title" }); // dispatches to on_command::<AppCmd>

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

Root `ui::*` re-exports the core surface (`TsType`, `write_typescript_if_requested`, `run`, `App`, `SingleInstance`, `Window`, `WebView`, `Context`, `Stream`, `TitleBar`, `WindowControls`, `Backdrop`, `Monitor`, `Profile`, `NavDecision`, `NavRequest`, `FindOptions`, `Notification`, `Request`, `Response`, `Menu`, `Dialog`, `Bytes`, `Value`, `DecodeError`, `FromIpc`, `IntoIpc`, `RpcResult`, `Error`, `Result`, `json!`, `ipc_enum!`, `ipc_struct!`), organized into focused submodules:

```text
ui
├── app        // run, App, SingleInstance
├── window     // Window, WindowBuilder, WindowId, TitleBar, WindowControls, Backdrop, Bounds, Monitor, Theme, WindowState
├── webview    // WebView, WebViewBuilder, WebViewId, Context, Request, Response, Mime, NavDecision, NavRequest, Profile, LoadState, FindOptions
├── embed      // Embedded (+ embedded!), served with WebViewBuilder::bundle
├── ipc        // Bytes, Value, FromIpc, IntoIpc, DecodeError, RpcResult (+ json!, ipc_enum!, ipc_struct!)
├── menu       // Menu, Submenu, MenuItem, MenuItemHandle, MenuItemUpdate, ContextMenuItem, Role, Shortcut, Modifiers
├── standard   // StandardRpcs (ui.app / dialog / clipboard / window / notify page calls)
├── macos      // AppExt, WindowBuilderExt, WindowExt, Dock, MacWindowOptions, ToolbarStyle, Tabbing, Role, speech (macOS only)
├── ts         // TsType, ipc_types!, write_typescript_if_requested
├── update     // check, verify_feed, UpdateInfo
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
cargo ui run --dev                                                             # same commands via the cargo-ui binary
cargo ui gen-ts --out web/src/ipc.d.ts                                         # TypeScript for ipc_types!
```

Notarize a Developer ID build with `ui-pack macos --sign "Developer ID Application: …" --notarize <keychain-profile>` (zips, submits with `notarytool --wait`, staples). Entitlements can also be inline:

```toml
[package.metadata.ui.entitlements]
"com.apple.security.hypervisor" = true
```

`[package.metadata.ui]` also takes `url_schemes`, `document_types`, `entitlements = "app.entitlements"` (used to sign `macos` bundles and `run` binaries, and as the Mac App Store entitlements), and `dev_command` / `dev_url` for `ui-pack run --dev`, which starts the web dev server and sets `UI_DEV_URL`. An `icon` ending in `.icns` is copied into the bundle, and `ui-pack run` shows it in the Dock for the unbundled binary (`UI_DOCK_ICON`).

- **`macos`**: Assembles `<DisplayName>.app`; for `backend = "cef"`, bundles `Chromium Embedded Framework.framework` and helper `.app` processes inside `Contents/Frameworks/` with `CFBundleIdentifier` aligned to `bundle_id`.
- **`mac-app-store`** (alias `mas`): Builds a sandboxed Mac App Store `.app` + `entitlements.mas.plist` (`com.apple.security.app-sandbox`) using the system webview (`backend = "cef"` is rejected).
- **`msix` / `linux`**: Generates Windows `AppxManifest.xml` or Linux `.desktop` manifests.

---

## Architecture & Platform Support

### IPC Transport Flow (`ui-ipc://ipc/`)

Renderer and host communication runs over the reserved `ui-ipc://ipc/` custom scheme using raw binary `ArrayBuffer` `POST` payloads:

```text
Renderer -> Host
  ipc.invoke(method, arg) ──▶ POST ui-ipc://ipc/rpc   ──▶ UI thread .invoke(..)      ──▶ ArrayBuffer reply
  ipc.invoke(.., {signal}) ─▶ POST ui-ipc://ipc/rpc (+call id) ─▶ worker .invoke_blocking ─▶ reply via UI thread
  ipc.stream(method, ..)  ──▶ POST ui-ipc://ipc/rpc (+call id, stream) ─▶ worker .invoke_stream ─▶ frames on __ui_stream:<id>
  ipc.emit(ev, arg)       ──▶ POST ui-ipc://ipc/send  ──▶ UI thread .on/.on_command ──▶ 200 OK

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
| **Nav policy & popups** | Navigation & UI delegates | `on_before_browse`, `on_before_popup` | `NavDecision::{Allow, Block, OpenInBrowser}` + `open_links_in_browser(bool)` |
| **Custom schemes (`app://`)** | `WKURLSchemeHandler` | `cef_register_scheme_handler_factory` | `Request` / `Response` with CORS, headers, and POST bodies |
| **Init scripts & binary IPC** | `WKUserScript` + `WKURLSchemeHandler` (`https://` fallback supported) | `on_context_created` + async `CefResourceHandler` | Shared `window.ipc` shim (`invoke`, `send`, `on`, `once`, `off`, `/drain`) |
| **Storage profiles** | `WKWebsiteDataStore` (`nonPersistent` / UUID) | `cef_request_context_t` (`p_<name>` / incognito) | `Profile::{Default, persistent(name), ephemeral()}`, `clear_data()` |
| **DevTools** | `devtools(bool)` → `isInspectable` (macOS 13.3+) | `show_dev_tools` via `open_devtools()` | `open_devtools()` returns `Err(Error::Unsupported)` on `webkit` |
| **Host shell** | `TitleBar::{Native, Overlay, Frameless}`, `Backdrop`, `ui::macos` options, `Menu::standard()`, sheet `Dialog`, `SingleInstance` | Shared macOS `AppKit` host | Identical window, menu, and dialog API |

### Environment Variables

- `UI_BACKEND`: Set to `webkit` or `cef` when compiled with `--features cef` (defaults to `cef` when the feature is enabled).
- `UI_CEF_DIR`: Path to a pre-extracted Spotify CEF directory containing `include/cef_api_hash.h` (skips downloading to `~/.cache/ui/cef/`).
- `UI_CEF_CACHE_DIR`: Override Chromium `root_cache_path` (defaults under `~/Library/Application Support/ui-cef/<exe>` on macOS, falling back to an isolated temp directory if locked).
- `UI_CEF_SCHEMES`: Extra comma-separated custom scheme names for CEF subprocess registration.
- `UI_CEF_LOG_CONSOLE=1`: Forward CEF JS `console` output to `stderr`.
- `UI_HEADLESS=1`: Run pure-Rust conformance checks without launching a live window/webview.
- `UI_DEV_URL`: In debug builds, load this `localhost` dev server in place of a webview's `assets`/`bundle` start page.
- `UI_BUILD_WEB`: For `ui-build`, `1` always runs the `before` command and `0` never does.
- `UI_DOCK_ICON`: Path to an image the unbundled binary shows in the Dock (set by `ui-pack run`).

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
