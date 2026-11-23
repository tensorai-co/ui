//! Shared cross-backend conformance suite (`harness = false` so GUI/webview tests run on the OS main thread).

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ui::ipc::{Bytes, FromIpc, IntoIpc, Value, parse};
use ui::menu::{Menu, Role};
use ui::webview::{LoadState, Mime, NavDecision, NavRequest, Profile, Request, Response};
use ui::{Backdrop, TitleBar, WebView, Window, WindowControls, json};

ui::ipc_enum! {
    enum TestCmd {
        #[tag = "rename"]
        Rename { title: String },
        #[tag = "ping"]
        Ping,
        #[tag = "scale"]
        Scale(u32),
    }
}

ui::ipc_struct! {
    #[derive(Debug, Clone, PartialEq)]
    struct V2Report {
        added: i64,
        timed_out: bool,
        chunks: String,
        closed_with: i64,
        misuse: bool,
        endless: String,
        menu_found: bool,
        menu_missing: bool,
        titlebar: String,
        controls_left: f64,
        platform_attr: String,
        dark_ok: bool,
        native: bool,
        client_platform: String,
        info_version: String,
        info_backend: String,
        info_named: bool,
        win_state: String,
        external_blocked: bool,
        clip_ok: bool,
        bad_menu_rejected: bool,
        found: String,
        not_found: String,
        pdf_head: String,
        mirror_ok: bool,
        missing: Option<String>,
    }
}

fn check_pure_rust_contracts() {
    // 1. json! and RFC 8259 roundtrip
    let payload = json!({
        "name": "ui",
        "version": 1,
        "flags": [true, false, null],
        "nested": { "pi": 3.5 }
    });
    let encoded = payload.to_json();
    let decoded = parse(&encoded).expect("valid JSON");
    assert_eq!(payload, decoded);

    // 2. ipc_enum! struct / unit / tuple variants
    let cmd = TestCmd::Rename {
        title: "Window X".into(),
    };
    assert_eq!(TestCmd::from_ipc(&cmd.clone().into_ipc()).unwrap(), cmd);
    let unit = TestCmd::Ping;
    assert_eq!(TestCmd::from_ipc(&unit.clone().into_ipc()).unwrap(), unit);
    let tup = TestCmd::Scale(2);
    assert_eq!(TestCmd::from_ipc(&tup.clone().into_ipc()).unwrap(), tup);

    // 3. Tuple + Bytes IPC codecs
    let tuple_val = ("foo.rs".to_owned(), 10usize, 200usize).into_ipc();
    assert_eq!(
        <(String, usize, usize)>::from_ipc(&tuple_val).unwrap(),
        ("foo.rs".to_owned(), 10, 200)
    );

    let slice_val = (&[1u8, 2, 255][..]).into_ipc();
    assert_eq!(slice_val.as_bytes(), Some(&[1u8, 2, 255][..]));
    assert_eq!(Bytes::from_ipc(&slice_val).unwrap().0, vec![1u8, 2, 255]);
    assert_eq!(slice_val.to_json(), "[1,2,255]");
    assert_eq!(
        Bytes::from_ipc(&Value::Array(vec![Value::Int(7), Value::Int(9)]))
            .unwrap()
            .0,
        vec![7, 9]
    );
    let bin_tuple = ("fb".to_owned(), Bytes(vec![9, 8, 7])).into_ipc();
    assert_eq!(
        <(String, Bytes)>::from_ipc(&bin_tuple).unwrap(),
        ("fb".to_owned(), Bytes(vec![9, 8, 7]))
    );

    // 4. MIME + Request + Response
    assert_eq!(Mime::from_path("index.html"), "text/html; charset=utf-8");
    assert_eq!(
        Mime::from_path("main.mjs"),
        "text/javascript; charset=utf-8"
    );
    assert_eq!(Mime::from_path("app.wasm"), "application/wasm");
    assert_eq!(Mime::from_path("font.woff2"), "font/woff2");
    assert_eq!(Mime::from_path("unknown.bin"), "application/octet-stream");

    let req = Request::new("app://localhost/assets/app.mjs?v=1", "GET");
    assert_eq!(req.scheme(), Some("app"));
    assert_eq!(req.host(), Some("localhost"));
    assert_eq!(req.path(), "/assets/app.mjs");
    assert_eq!(req.query(), Some("v=1"));

    let nav_ext = NavRequest::new("https://external.example.org/docs", true);
    assert_eq!(nav_ext.host(), Some("external.example.org"));
    assert!(nav_ext.is_main_frame());

    let asset = Response::asset("app.mjs", b"export const x = 1;".to_vec());
    assert_eq!(asset.status, 200);
    assert_eq!(asset.mime(), "text/javascript; charset=utf-8");

    // 5. Window/Menu value types
    let tb = TitleBar::overlay(38).window_controls(WindowControls::at(14, 12));
    assert_eq!(
        tb,
        TitleBar::Overlay {
            height: 38,
            window_controls: Some(WindowControls::at(14, 12))
        }
    );

    // The v1 names still work, as deprecated aliases.
    #[allow(deprecated)]
    {
        assert_eq!(TitleBar::inset(38).traffic_lights(14, 12), tb);
        assert_eq!(TitleBar::Hidden, TitleBar::Frameless);
        assert_eq!(ui::Backdrop::Transient, ui::Backdrop::Popover);
        assert_eq!(ui::Backdrop::Mica, ui::Backdrop::Window);
        assert_eq!(Role::Zoom, Role::Maximize);
        assert_eq!(ui::event::Event::Reopen, ui::event::Event::Activate);
        assert_eq!(ui::Nav::Cancel, NavDecision::Block);
        assert_eq!(ui::Nav::External, NavDecision::OpenInBrowser);
        assert_eq!(
            ui::menu::Shortcut::cmd_shift("z"),
            ui::menu::Shortcut::primary_shift("z")
        );
        let m: ui::menu::Mod = ui::menu::Modifiers::PRIMARY;
        assert_eq!(m, ui::menu::Modifiers::PRIMARY);
        fn takes_ctx(_: Option<ui::Ctx>) {}
        takes_ctx(None::<ui::Context>);
        let _: Option<ui::window::Display> = None::<ui::Monitor>;
    }

    let std_menu = Menu::standard();
    assert!(std_menu.submenus.iter().any(|s| s.title == "Edit"));
    assert!(std_menu.submenus[1].items.iter().any(|i| matches!(
        i,
        ui::menu::MenuItem::Action {
            role: Some(Role::Copy),
            ..
        }
    )));
}

#[derive(Debug, Default)]
struct LiveProbe {
    loaded_url: Option<String>,
    title: Option<String>,
    rpc_seen: Option<(String, usize, usize)>,
    bin_rpc_seen: Option<(String, Vec<u8>)>,
    bin_upload_seen: Option<Vec<u8>>,
    cmd_seen: Option<TestCmd>,
    emit_echoed: Option<String>,
    frame_echoed: Option<(u32, Vec<u8>)>,
    once_ok: bool,
    inline_html_bytes: Option<Vec<u8>>,
    dom_width: Option<i64>,
    dom_height: Option<i64>,
    nav_checked: bool,
    load_finished: bool,
    reserved_scheme_rejected: bool,
    /// Thread name `invoke_blocking` ran on.
    blocking_thread: Option<String>,
    /// `ctx.is_cancelled()` turned true after the page's `{ timeout }` passed.
    cancel_seen: bool,
    /// A page-closed stream saw `send` fail and returned.
    stream_stopped: bool,
    v2_report: Option<V2Report>,
    backend: String,
    dock_ok: bool,
}

/// A 2x2 PNG, for the Dock icon check.
const TINY_PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x02, 0x08, 0x06, 0x00, 0x00, 0x00, 0x72, 0xb6, 0x0d,
    0x24, 0x00, 0x00, 0x00, 0x11, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x30, 0xab, 0xfe, 0xf0,
    0x1f, 0x84, 0x19, 0x60, 0x0c, 0x00, 0x58, 0xf2, 0x0a, 0x81, 0x6c, 0xea, 0x49, 0xe2, 0x00, 0x00,
    0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

#[cfg(target_os = "macos")]
fn check_dock(app: &ui::App) -> bool {
    use ui::macos::{AppExt, Attention};
    let dock = app.macos().dock();
    dock.set_badge("3");
    let shown = dock.badge();
    dock.clear_badge();
    let cleared = dock.badge();
    let icon_ok = dock.set_icon(TINY_PNG).is_ok();
    let junk_rejected = dock.set_icon(b"not an image").is_err();
    dock.reset_icon();
    dock.set_menu(vec![
        ui::menu::MenuItem::new("new_window", "New Window").emit(),
        ui::menu::MenuItem::separator(),
    ]);
    dock.set_menu(Vec::new());
    if let Some(req) = dock.bounce(Attention::Informational) {
        dock.cancel_bounce(req);
    }
    let ok = shown.as_deref() == Some("3") && cleared.is_none() && icon_ok && junk_rejected;
    if !ok {
        eprintln!(
            "[conformance] dock: shown={shown:?} cleared={cleared:?} icon_ok={icon_ok} junk_rejected={junk_rejected}"
        );
    }
    ok
}

fn check_live_window_and_webview() {
    let probe = Arc::new(Mutex::new(LiveProbe::default()));
    let probe_res = probe.clone();

    let run_res = ui::run(move |app| {
        eprintln!(
            "[conformance] running live test on backend={}",
            app.backend()
        );
        probe.lock().unwrap().backend = app.backend().to_owned();
        #[cfg(target_os = "macos")]
        {
            probe.lock().unwrap().dock_ok = check_dock(app);
        }

        let win = Window::new("ui-conformance")
            .size(640, 420)
            .min_size(320, 240)
            .title_bar(TitleBar::overlay(38).window_controls(WindowControls::at(14, 12)))
            .backdrop(Backdrop::Sidebar)
            .open(app)?;

        // Reserved scheme `ui-ipc` must be rejected by `.mount`.
        let reserved = WebView::new()
            .assets("ui-ipc", |_| Response::not_found())
            .mount(&win);
        if let Ok(mut g) = probe.lock() {
            g.reserved_scheme_rejected = reserved.is_err();
        }

        let p_nav = probe.clone();
        let p_rpc = probe.clone();
        let p_bin_rpc = probe.clone();
        let p_bin_up = probe.clone();
        let p_ready = probe.clone();
        let p_frame_report = probe.clone();
        let p_report = probe.clone();
        let p_cmd = probe.clone();
        let p_ev = probe.clone();
        let p_blocking = probe.clone();
        let p_cancel = probe.clone();
        let p_stopped = probe.clone();
        let p_v2 = probe.clone();
        let app_exit = app.clone();

        const HTML: &str = r#"<!doctype html>
<html>
<head><meta charset="utf-8"><title>Conformance Title</title></head>
<body style="margin:0;width:100vw;height:100vh;background:#1e293b;color:#fff;">
  <div id="hdr" data-ui-drag style="height:38px;">Header</div>
  <main id="root" style="width:100%;height:200px;">Hello ui</main>
  <script type="module" src="/entry.mjs"></script>
</body>
</html>"#;

        const ENTRY_MJS: &str = r#"import { ui, isNative, platform } from "/ui-client.js";
async function boot() {
  let echoed = null;
  let onceCount = 0;
  let offCount = 0;

  const offFn = () => { offCount++; };
  ipc.on("frame_emit", offFn);
  ipc.off("frame_emit", offFn);

  ipc.once("host_ping", () => {
    onceCount++;
  });

  let pendingReport = null;
  ipc.on("host_ping", (payload) => {
    echoed = payload.msg;
    const rect = document.getElementById("root").getBoundingClientRect();
    ipc.send({ type: "rename", title: "Renamed By WebView" });
    pendingReport = [echoed, Math.round(rect.width), Math.round(rect.height)];
  });

  ipc.on("frame_emit", ([w, frame]) => {
    if (!(frame instanceof Uint8Array) || frame.byteOffset !== 0 || frame.buffer.byteLength !== frame.byteLength) {
      throw new Error("expected clean zero-offset Uint8Array framebuffer");
    }
    const [eEcho, rw, rh] = pendingReport;
    ipc.send("frame_report", w, frame);
    ipc.send("report", [eEcho, rw, rh, onceCount === 1 && offCount === 0]);
  });

  const sum = await ipc.invoke("slice_echo", "hello.rs", 2, 5);
  if (sum !== "hello.rs:2:5") {
    throw new Error("unexpected rpc result: " + sum);
  }

  const rawIn = new Uint8Array([10, 20, 30, 255]);
  const [tag, rawOut] = await ipc.invoke("bin_echo", "bin", rawIn);
  if (tag !== "bin" || !(rawOut instanceof Uint8Array) || rawOut.byteOffset !== 0 || rawOut.buffer.byteLength !== rawOut.byteLength) {
    throw new Error("unexpected binary rpc return type");
  }
  if (rawOut.length !== 4 || rawOut[0] !== 11 || rawOut[1] !== 21 || rawOut[2] !== 31 || rawOut[3] !== 0) {
    throw new Error("unexpected binary rpc return bytes: " + Array.from(rawOut));
  }

  // v2 IPC: worker-thread calls, cancellation, streams.
  const added = await ipc.invoke("slow_add", 2, 3);
  let timedOut = false;
  try {
    await ipc.invoke("wait_cancel", "x", { timeout: 150 });
  } catch (e) {
    timedOut = e.name === "TimeoutError";
  }
  const chunks = [];
  const counter = ipc.stream("count", 4);
  counter.on("data", (b) => chunks.push(b[0]));
  counter.send(new Uint8Array([40]));
  const closedWith = await counter.closed;
  let misuse = false;
  try {
    await ipc.invoke("count", 1);
  } catch (e) {
    misuse = /is a stream/.test(e.message);
  }
  const endless = ipc.stream("until_closed");
  await new Promise((r) => endless.on("data", r));
  endless.close();
  const endlessResult = await endless.closed;
  const menuFound = await ipc.menu.set({ id: "role:ActualSize", checked: true, enabled: false });
  const menuMissing = await ipc.menu.set({ id: "no-such-item", enabled: false });
  const waitFor = async (pred) => {
    for (let i = 0; i < 150; i++) {
      if (pred()) return true;
      await new Promise((r) => setTimeout(r, 20));
    }
    return false;
  };
  const root = document.documentElement;
  await waitFor(() => root.hasAttribute("data-ui-platform"));
  const css = getComputedStyle(root);
  const titlebar = css.getPropertyValue("--ui-titlebar-height").trim();
  const controlsLeft = parseFloat(css.getPropertyValue("--ui-window-controls-left")) || 0;
  const platformAttr = root.getAttribute("data-ui-platform") || "";
  await ipc.window.setTheme("dark");
  const darkOk = await waitFor(() => root.getAttribute("data-ui-theme") === "dark");
  await ipc.window.setTheme("system");
  // .mirror_theme(): the window follows <html data-theme>.
  root.setAttribute("data-theme", "dark");
  const mirrorDark = await waitFor(() => root.getAttribute("data-ui-theme") === "dark");
  root.setAttribute("data-theme", "light");
  const mirrorLight = await waitFor(() => root.getAttribute("data-ui-theme") === "light");
  root.removeAttribute("data-theme");

  let badMenu = await ipc.contextMenu([{ label: "No id" }], { x: 1, y: 1 }).then(
    () => false,
    (e) => /needs an "id"/.test(e.message)
  );

  // ui-client + standard_rpcs
  const info = await ui.app.info();
  const winState = await ui.window.state();
  let externalBlocked = false;
  try {
    await ui.app.openExternal("file:///etc/hosts");
  } catch (e) {
    externalBlocked = /only opens http/.test(e.message);
  }
  const found = await ui.webview.find("Hello ui");
  const notFound = await ui.webview.find("zz-not-on-this-page");
  await ui.webview.clearFind();
  let pdfHead = "";
  try {
    const pdf = await ui.webview.pdf();
    pdfHead = new TextDecoder().decode(pdf.subarray(0, 5));
  } catch (e) {
    pdfHead = /isn't available/.test(e.message) ? "unsupported" : "error: " + e.message;
  }

  // Clipboard round trip, only when it holds plain text we can put back.
  let clipOk = true;
  const original = await ui.clipboard.readText();
  if (typeof original === "string") {
    await ui.clipboard.writeText("ui-client-test");
    clipOk = (await ui.clipboard.readText()) === "ui-client-test";
    await ui.clipboard.writeText(original);
  }
  ipc.emit("v2_report", {
    added, timed_out: timedOut, chunks: chunks.join(","), closed_with: closedWith,
    misuse, endless: endlessResult, menu_found: menuFound, menu_missing: menuMissing,
    titlebar, controls_left: controlsLeft, platform_attr: platformAttr, dark_ok: darkOk,
    native: isNative(), client_platform: platform, info_version: info.version,
    info_backend: info.backend, info_named: info.name.length > 0, win_state: winState,
    external_blocked: externalBlocked, clip_ok: clipOk, bad_menu_rejected: badMenu,
    found: found === null ? "null" : String(found), not_found: notFound === null ? "null" : String(notFound),
    pdf_head: pdfHead, mirror_ok: mirrorDark && mirrorLight,
  });

  ipc.send("bin_upload", new Uint8Array([1, 2, 3, 4]));
  ipc.send("ready");
}
boot();"#;

        let wv = WebView::new()
            .profile(Profile::ephemeral())
            .devtools(true)
            .assets("app", |req| match req.path() {
                "/" | "/index.html" => Response::asset("index.html", HTML.as_bytes()),
                "/entry.mjs" => Response::asset("entry.mjs", ENTRY_MJS.as_bytes()),
                "/ui-client.js" => Response::asset(
                    "ui-client.js",
                    include_str!("../../../packages/ui-client/index.js").as_bytes(),
                ),
                _ => Response::not_found(),
            })
            .on_navigate(move |req| {
                if let Ok(mut g) = p_nav.lock() {
                    g.nav_checked = true;
                }
                if req.host() == Some("blocked.invalid") {
                    NavDecision::Block
                } else {
                    NavDecision::Allow
                }
            })
            .invoke(
                "slice_echo",
                move |_ctx, (path, a, b): (String, usize, usize)| {
                    if let Ok(mut g) = p_rpc.lock() {
                        g.rpc_seen = Some((path.clone(), a, b));
                    }
                    format!("{path}:{a}:{b}")
                },
            )
            .invoke("bin_echo", move |_ctx, (tag, data): (String, Bytes)| {
                if let Ok(mut g) = p_bin_rpc.lock() {
                    g.bin_rpc_seen = Some((tag.clone(), data.0.clone()));
                }
                let flipped: Vec<u8> = data.0.into_iter().map(|b| b.wrapping_add(1)).collect();
                (tag, Bytes(flipped))
            })
            .standard_rpcs(ui::StandardRpcs::all().version("9.9.9"))
            .mirror_theme()
            .invoke_blocking("slow_add", move |_ctx, (a, b): (i64, i64)| {
                std::thread::sleep(Duration::from_millis(30));
                if let Ok(mut g) = p_blocking.lock() {
                    g.blocking_thread = std::thread::current().name().map(str::to_owned);
                }
                a + b
            })
            .invoke_blocking("wait_cancel", move |ctx, _tag: String| {
                let start = Instant::now();
                while !ctx.is_cancelled() && start.elapsed() < Duration::from_secs(5) {
                    std::thread::sleep(Duration::from_millis(5));
                }
                if let Ok(mut g) = p_cancel.lock() {
                    g.cancel_seen = ctx.is_cancelled();
                }
                "finished"
            })
            .invoke_stream("count", |_ctx, n: u8, stream| -> ui::Result<i64> {
                let mut total = 0i64;
                for i in 0..n {
                    stream.send(Bytes(vec![i]))?;
                    total += i64::from(i);
                }
                if let Some(back) = stream.recv() {
                    total += Bytes::from_ipc(&back)
                        .map(|b| i64::from(b.0[0]))
                        .unwrap_or(0);
                }
                Ok(stream.close_with(total))
            })
            .invoke_stream("until_closed", move |_ctx, (), stream| {
                let start = Instant::now();
                let mut n = 0u32;
                while stream.send(n).is_ok() && start.elapsed() < Duration::from_secs(5) {
                    n += 1;
                    std::thread::sleep(Duration::from_millis(5));
                }
                if let Ok(mut g) = p_stopped.lock() {
                    g.stream_stopped = stream.is_closed();
                }
                "stopped"
            })
            .on("v2_report", move |_ctx, report: V2Report| {
                if let Ok(mut g) = p_v2.lock() {
                    g.v2_report = Some(report);
                }
            })
            .on("bin_upload", move |_ctx, data: Bytes| {
                if let Ok(mut g) = p_bin_up.lock() {
                    g.bin_upload_seen = Some(data.0);
                }
            })
            .on("ready", move |ctx, ()| {
                if let Ok(mut g) = p_ready.lock() {
                    g.loaded_url = Some(ctx.webview().current_url());
                    g.title = Some(ctx.webview().title());
                }
                ctx.emit("host_ping", json!({ "msg": "pong-from-host" }));
                ctx.emit("frame_emit", (4u32, Bytes(vec![0xaa, 0xbb, 0xcc, 0xdd])));
            })
            .on_command::<TestCmd>(move |ctx, cmd| {
                if let TestCmd::Rename { ref title } = cmd {
                    ctx.window().set_title(title);
                }
                if let Ok(mut g) = p_cmd.lock() {
                    g.cmd_seen = Some(cmd);
                }
            })
            .on("frame_report", move |_ctx, (fw, fbytes): (u32, Bytes)| {
                if let Ok(mut g) = p_frame_report.lock() {
                    g.frame_echoed = Some((fw, fbytes.0));
                }
            })
            .on(
                "report",
                move |ctx, (echoed, w, h, once_ok): (String, i64, i64, bool)| {
                    let inline_probe = p_report.clone();
                    if let Ok(mut g) = p_report.lock() {
                        g.emit_echoed = Some(echoed);
                        g.dom_width = Some(w);
                        g.dom_height = Some(h);
                        g.once_ok = once_ok;
                    }
                    ctx.window().close();

                    // Verify `WebView::html(...)` inline page also speaks `ui-ipc://` binary IPC in the same app run.
                    let win2 = Window::new("ui-inline-html")
                        .size(320, 240)
                        .open(&ctx.app())
                        .expect("open inline window");
                    const INLINE: &str = r#"<!doctype html><html><body><script>
                  (async () => {
                    const ret = await ipc.invoke("ping_bin", new Uint8Array([4, 5, 6]));
                    ipc.send("done", ret);
                  })();
                </script></body></html>"#;
                    WebView::html(INLINE)
                        .profile(Profile::ephemeral())
                        .invoke("ping_bin", |_ctx, data: Bytes| data)
                        .on("done", move |ctx, data: Bytes| {
                            if let Ok(mut g) = inline_probe.lock() {
                                g.inline_html_bytes = Some(data.0);
                            }
                            ctx.window().close();
                            ctx.app().quit();
                        })
                        .mount(&win2)
                        .expect("mount inline webview");
                },
            )
            .on_event(move |ev| {
                if let ui::event::WebViewEvent::LoadState(LoadState::Finished) = ev
                    && let Ok(mut g) = p_ev.lock()
                {
                    g.load_finished = true;
                }
            })
            .mount(&win)?;

        if app.backend() == "webkit" {
            assert!(
                wv.open_devtools().is_err(),
                "webkit open_devtools must return Unsupported"
            );
        }

        // Watchdog thread so CI/local runs never hang if a backend fails to render/report.
        std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(30);
            while Instant::now() < deadline {
                if let Ok(g) = probe.lock()
                    && g.inline_html_bytes.is_some()
                {
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            app_exit.quit();
        });

        Ok(())
    });

    run_res.expect("ui::run failed");

    let g = probe_res.lock().unwrap();
    eprintln!("[conformance] probe result: {g:?}");
    assert!(g.reserved_scheme_rejected, "ui-ipc scheme must be reserved");
    assert_eq!(g.rpc_seen, Some(("hello.rs".into(), 2, 5)));
    assert_eq!(g.bin_rpc_seen, Some(("bin".into(), vec![10, 20, 30, 255])));
    assert_eq!(g.bin_upload_seen, Some(vec![1, 2, 3, 4]));
    assert_eq!(
        g.cmd_seen,
        Some(TestCmd::Rename {
            title: "Renamed By WebView".into()
        })
    );
    assert_eq!(g.emit_echoed.as_deref(), Some("pong-from-host"));
    assert_eq!(g.frame_echoed, Some((4, vec![0xaa, 0xbb, 0xcc, 0xdd])));
    assert_eq!(
        g.inline_html_bytes,
        Some(vec![4, 5, 6]),
        "WebView::html inline page must support ui-ipc:// binary IPC"
    );
    assert!(g.once_ok, "ipc.once / ipc.off semantics failed");
    assert!(
        g.dom_width.unwrap_or(0) > 100,
        "webview DOM width should be rendered > 100, got {:?}",
        g.dom_width
    );
    assert!(
        g.dom_height.unwrap_or(0) >= 200,
        "webview DOM height should be >= 200, got {:?}",
        g.dom_height
    );
    assert!(g.nav_checked, "on_navigate should have fired");
    assert!(
        g.blocking_thread
            .as_deref()
            .is_some_and(|n| n.starts_with("ui-invoke:")),
        "invoke_blocking must run on a worker thread, got {:?}",
        g.blocking_thread
    );
    assert!(g.cancel_seen, "a page timeout must set ctx.is_cancelled()");
    assert_eq!(
        g.dock_ok,
        cfg!(target_os = "macos"),
        "Dock badge / icon checks"
    );
    assert!(g.stream_stopped, "stream.close() must close the Rust side");
    let mut report = g.v2_report.clone().expect("v2 report");
    assert!(
        (40.0..120.0).contains(&report.controls_left),
        "--ui-window-controls-left should cover the three controls, got {}",
        report.controls_left
    );
    report.controls_left = 0.0;
    assert_eq!(
        Some(report),
        Some(V2Report {
            added: 5,
            timed_out: true,
            chunks: "0,1,2,3".into(),
            closed_with: 46,
            misuse: true,
            endless: "stopped".into(),
            menu_found: cfg!(target_os = "macos"),
            menu_missing: false,
            titlebar: "38px".into(),
            controls_left: 0.0,
            platform_attr: "macos".into(),
            dark_ok: true,
            native: true,
            client_platform: "macos".into(),
            info_version: "9.9.9".into(),
            info_backend: g.backend.clone(),
            info_named: true,
            win_state: "normal".into(),
            external_blocked: true,
            clip_ok: true,
            bad_menu_rejected: true,
            // WebKit reports matches and renders PDFs; CEF does neither through `ui`.
            found: if g.backend == "webkit" {
                "true"
            } else {
                "null"
            }
            .into(),
            not_found: if g.backend == "webkit" {
                "false"
            } else {
                "null"
            }
            .into(),
            pdf_head: if g.backend == "webkit" {
                "%PDF-"
            } else {
                "unsupported"
            }
            .into(),
            mirror_ok: true,
            missing: None,
        }),
        "invoke_blocking / timeout / stream results"
    );
    assert!(g.load_finished, "LoadState::Finished should have fired");
}

fn main() {
    check_pure_rust_contracts();
    if std::env::var("UI_HEADLESS").as_deref() != Ok("1") {
        check_live_window_and_webview();
    }
    eprintln!("[conformance] ALL CONFORMANCE CHECKS PASSED");
}
