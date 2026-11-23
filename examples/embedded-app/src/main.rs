//! A `ui` app whose web build is embedded at compile time.
//!
//! ```sh
//! cargo run -p embedded-app
//! UI_DEV_URL=http://localhost:5173 cargo run -p embedded-app   # load a dev server instead
//! ```
//!
//! Packaged with `ui-pack macos`, it also handles `uiembedded://…` links and
//! `.uidoc` files (see `[package.metadata.ui]`), printing them as they arrive.
//! Set `UI_EVENT_LOG=<file>` to append them to a file as well.

use std::io::Write;

use ui::event::Event;
use ui::{WebView, Window};

fn log_event(line: &str) {
    println!("{line}");
    if let Some(path) = std::env::var_os("UI_EVENT_LOG")
        && let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
    {
        let _ = writeln!(f, "{line}");
    }
}

fn main() -> ui::Result<()> {
    ui::run(|app| {
        app.on_event(|ev| match ev {
            Event::OpenUrl(url) => log_event(&format!("open-url {url}")),
            Event::OpenFiles(files) => {
                for f in files {
                    log_event(&format!("open-file {}", f.display()));
                }
            }
            _ => {}
        });
        let win = Window::new("Embedded App").size(900, 600).open(app)?;
        WebView::new()
            .bundle(
                "app",
                ui::embedded!().spa("index.html").immutable("assets/"),
            )
            .invoke("echo", |_ctx, s: String| format!("echo:{s}"))
            .on("report", |_ctx, report: ui::Value| {
                println!("page report: {}", report.to_json())
            })
            .mount(&win)?;
        Ok(())
    })
}
