//! Shared cross-backend conformance suite (`harness = false` so GUI/webview tests run on the OS main thread).

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ui::ipc::{Bytes, FromIpc, IntoIpc, Value, parse};
use ui::menu::{Menu, Role};
use ui::webview::{LoadState, Mime, Nav, NavRequest, Profile, Request, Response};
use ui::{Backdrop, TitleBar, WebView, Window, json};

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
    let tb = TitleBar::inset(38).traffic_lights(14, 12);
    assert_eq!(
        tb,
        TitleBar::Inset {
            height: 38,
            traffic_lights: Some((14, 12))
        }
    );

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
}

fn check_live_window_and_webview() {
    let probe = Arc::new(Mutex::new(LiveProbe::default()));
    let probe_res = probe.clone();

    let run_res = ui::run(move |app| {
        eprintln!(
            "[conformance] running live test on backend={}",
            app.backend()
        );

        let win = Window::new("ui-conformance")
            .size(640, 420)
            .min_size(320, 240)
            .title_bar(TitleBar::inset(38).traffic_lights(14, 12))
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

        const ENTRY_MJS: &str = r#"async function boot() {
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

  ipc.send("bin_upload", new Uint8Array([1, 2, 3, 4]));
  ipc.send("ready");
}
boot();"#;

        let wv = WebView::new()
            .profile(Profile::ephemeral())
            .inspectable(true)
            .assets("app", |req| match req.path() {
                "/" | "/index.html" => Response::asset("index.html", HTML.as_bytes()),
                "/entry.mjs" => Response::asset("entry.mjs", ENTRY_MJS.as_bytes()),
                _ => Response::not_found(),
            })
            .on_navigate(move |req| {
                if let Ok(mut g) = p_nav.lock() {
                    g.nav_checked = true;
                }
                if req.host() == Some("blocked.invalid") {
                    Nav::Cancel
                } else {
                    Nav::Allow
                }
            })
            .rpc(
                "slice_echo",
                move |_ctx, (path, a, b): (String, usize, usize)| {
                    if let Ok(mut g) = p_rpc.lock() {
                        g.rpc_seen = Some((path.clone(), a, b));
                    }
                    format!("{path}:{a}:{b}")
                },
            )
            .rpc("bin_echo", move |_ctx, (tag, data): (String, Bytes)| {
                if let Ok(mut g) = p_bin_rpc.lock() {
                    g.bin_rpc_seen = Some((tag.clone(), data.0.clone()));
                }
                let flipped: Vec<u8> = data.0.into_iter().map(|b| b.wrapping_add(1)).collect();
                (tag, Bytes(flipped))
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
            .on_msg::<TestCmd>(move |ctx, cmd| {
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
                        .rpc("ping_bin", |_ctx, data: Bytes| data)
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
            let deadline = Instant::now() + Duration::from_secs(15);
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
    assert!(g.load_finished, "LoadState::Finished should have fired");
}

fn main() {
    check_pure_rust_contracts();
    if std::env::var("UI_HEADLESS").as_deref() != Ok("1") {
        check_live_window_and_webview();
    }
    eprintln!("[conformance] ALL CONFORMANCE CHECKS PASSED");
}
