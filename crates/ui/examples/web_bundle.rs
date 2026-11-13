//! Example matching README Quickstart: single-page `app://localhost/` web bundle,
//! inset titlebar + traffic lights, sidebar backdrop, drag region, and typed IPC.

use ui::{Backdrop, Request, Response, TitleBar, WebView, Window, json};

mod assets {
    use super::*;

    const INDEX_HTML: &str = r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <title>My App — ui</title>
  <style>
    :root {
      color-scheme: light dark;
      font-family: -apple-system, BlinkMacSystemFont, "Inter", system-ui, sans-serif;
    }
    * { box-sizing: border-box; }
    html, body {
      margin: 0;
      height: 100%;
      background: rgba(24, 26, 32, 0.72);
      color: #f3f4f6;
      user-select: none;
    }
    header {
      height: 38px;
      padding: 0 16px 0 82px;
      display: flex;
      align-items: center;
      justify-content: space-between;
      border-bottom: 1px solid rgba(255, 255, 255, 0.08);
      -webkit-app-region: drag;
    }
    .actions {
      display: flex;
      gap: 8px;
      -webkit-app-region: no-drag;
    }
    button {
      background: #3b82f6;
      color: white;
      border: 0;
      border-radius: 6px;
      padding: 5px 12px;
      font-size: 12px;
      font-weight: 600;
      cursor: pointer;
    }
    button:hover { background: #2563eb; }
    main {
      padding: 28px;
      max-width: 720px;
    }
    .card {
      background: rgba(255, 255, 255, 0.06);
      border: 1px solid rgba(255, 255, 255, 0.1);
      border-radius: 12px;
      padding: 18px;
      margin-top: 16px;
      user-select: text;
    }
    input {
      width: 100%;
      margin-top: 10px;
      padding: 8px 10px;
      border-radius: 8px;
      border: 1px solid rgba(255, 255, 255, 0.18);
      background: rgba(0, 0, 0, 0.25);
      color: inherit;
      font-size: 13px;
    }
    pre {
      margin: 12px 0 0;
      font-size: 12px;
      color: #93c5fd;
      white-space: pre-wrap;
    }
  </style>
</head>
<body>
  <header data-ui-drag>
    <span style="font-size:13px;font-weight:600;opacity:0.9;">My App</span>
    <div class="actions">
      <button id="openBtn" type="button">Open Project</button>
    </div>
  </header>
  <main>
    <h1 style="margin:0 0 8px;font-size:22px;">Single-Page Web Bundle</h1>
    <p style="margin:0;opacity:0.75;font-size:14px;">Served over <code>app://localhost/</code> with ES modules &amp; native IPC.</p>
    <div class="card">
      <div>Paste / Copy test input (<code>Menu::standard()</code>):</div>
      <input type="text" value="Cmd+C / Cmd+V / Cmd+A / Cmd+Z work natively" />
      <pre id="status">Waiting for host...</pre>
    </div>
  </main>
  <script type="module" src="/app.mjs"></script>
</body>
</html>
"#;

    const APP_MJS: &str = r#"const status = document.getElementById("status");
const openBtn = document.getElementById("openBtn");

ipc.on("app_info", (info) => {
  status.textContent = "Connected to ui host (" + info.backend + ") v" + info.version;
});

openBtn.addEventListener("click", async () => {
  const folder = await ipc.invoke("open_project");
  status.textContent = folder ? ("Selected folder: " + folder) : "Folder picker cancelled.";
});

ipc.send("ready");
"#;

    pub fn serve(req: &Request) -> Response {
        match req.path() {
            "/" | "/index.html" => Response::asset("index.html", INDEX_HTML.as_bytes()),
            "/app.mjs" => Response::asset("app.mjs", APP_MJS.as_bytes()),
            _ => Response::not_found(),
        }
    }
}

fn main() -> ui::Result<()> {
    ui::run(|app| {
        let backend = app.backend();
        let win = Window::new("My App")
            .size(1280, 840)
            .min_size(760, 520)
            .title_bar(TitleBar::inset(38).traffic_lights(14, 12))
            .backdrop(Backdrop::Sidebar)
            .open(app)?;

        WebView::new()
            .assets("app", assets::serve)
            .inspectable(cfg!(debug_assertions))
            .rpc("open_project", |ctx, ()| {
                ctx.window().dialog().pick_folder()
            })
            .on("ready", move |ctx, ()| {
                ctx.emit(
                    "app_info",
                    json!({ "version": "0.1.0", "backend": backend }),
                );
            })
            .mount(&win)?;

        Ok(())
    })
}
