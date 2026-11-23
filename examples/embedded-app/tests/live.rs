//! Live check: the embedded bundle and `UI_DEV_URL` (harness = false, runs on the main thread).

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ui::{WebView, Window};

ui::ipc_struct! {
    #[derive(Debug, Clone, PartialEq)]
    struct Report {
        href: String,
        echo: String,
        route_status: Option<i64>,
        route_is_index: Option<bool>,
        missing_status: Option<i64>,
        asset_cache: Option<String>,
    }
}

const DEV_PAGE: &str = r#"<!doctype html><html><body><script type="module">
  const echo = await ipc.invoke("echo", "dev");
  ipc.emit("report", { href: location.href, echo });
</script></body></html>"#;

/// Serves `DEV_PAGE` for every request; returns its port.
fn dev_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind dev server");
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            while reader.read_line(&mut line).is_ok_and(|n| n > 2) {
                line.clear();
            }
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{DEV_PAGE}",
                DEV_PAGE.len()
            );
        }
    });
    port
}

fn main() {
    if std::env::var("UI_HEADLESS").as_deref() == Ok("1") {
        eprintln!("[embedded-app] skipped (UI_HEADLESS=1)");
        return;
    }
    let reports: Arc<Mutex<Vec<Report>>> = Arc::default();
    let seen = reports.clone();
    let port = dev_server();

    ui::run(move |app| {
        let mount = |title: &str, reports: Arc<Mutex<Vec<Report>>>| -> ui::Result<()> {
            let win = Window::new(title).size(480, 320).open(app)?;
            WebView::new()
                .bundle(
                    "app",
                    ui::embedded!().spa("index.html").immutable("assets/"),
                )
                .invoke("echo", |_ctx, s: String| format!("echo:{s}"))
                .on("report", move |ctx, r: Report| {
                    reports.lock().unwrap().push(r);
                    ctx.window().close();
                })
                .mount(&win)?;
            Ok(())
        };

        mount("embedded", reports.clone())?;
        // SAFETY: single-threaded at this point; read synchronously by the next mount.
        unsafe { std::env::set_var("UI_DEV_URL", format!("http://127.0.0.1:{port}/")) };
        mount("dev-url", reports.clone())?;
        // SAFETY: as above; no other thread reads the environment.
        unsafe { std::env::remove_var("UI_DEV_URL") };

        let app = app.clone();
        let watch = reports.clone();
        std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(15);
            while Instant::now() < deadline && watch.lock().unwrap().len() < 2 {
                std::thread::sleep(Duration::from_millis(50));
            }
            app.quit();
        });
        Ok(())
    })
    .expect("ui::run failed");

    let mut got = seen.lock().unwrap().clone();
    got.sort_by(|a, b| a.href.cmp(&b.href));
    eprintln!("[embedded-app] reports: {got:#?}");
    assert_eq!(got.len(), 2, "both pages must report");
    let embedded = got
        .iter()
        .find(|r| r.href.starts_with("app://"))
        .expect("embedded page");
    assert_eq!(
        embedded.href, "app://localhost/",
        "the URL stays / for routers"
    );
    assert_eq!(embedded.echo, "echo:embedded");
    assert_eq!(embedded.route_status, Some(200));
    assert_eq!(
        embedded.route_is_index,
        Some(true),
        "SPA routes serve index.html"
    );
    assert_eq!(embedded.missing_status, Some(404));
    assert_eq!(
        embedded.asset_cache.as_deref(),
        Some("public, max-age=31536000, immutable")
    );
    let dev = got
        .iter()
        .find(|r| r.href.starts_with("http://"))
        .expect("dev page");
    assert_eq!(dev.href, format!("http://127.0.0.1:{port}/"));
    assert_eq!(dev.echo, "echo:dev", "IPC must work on the dev server page");
    eprintln!("[embedded-app] ALL CHECKS PASSED");
}
