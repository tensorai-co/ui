//! A `ui` app whose web build is embedded at compile time.
//!
//! ```sh
//! cargo run -p embedded-app
//! UI_DEV_URL=http://localhost:5173 cargo run -p embedded-app   # load a dev server instead
//! ```

use ui::{Window, WebView};

fn main() -> ui::Result<()> {
    ui::run(|app| {
        let win = Window::new("Embedded App").size(900, 600).open(app)?;
        WebView::new()
            .bundle("app", ui::embedded!().spa("index.html").immutable("assets/"))
            .invoke("echo", |_ctx, s: String| format!("echo:{s}"))
            .on("report", |_ctx, report: ui::Value| println!("page report: {}", report.to_json()))
            .mount(&win)?;
        Ok(())
    })
}
