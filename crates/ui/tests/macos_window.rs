//! Live check of `ui::macos` window options and `restore_state` (harness = false).

#[cfg(target_os = "macos")]
fn main() {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use ui::macos::{WindowBuilderExt, WindowExt, WindowLevel};
    use ui::{TitleBar, WebView, Window};

    if std::env::var("UI_HEADLESS").as_deref() == Ok("1") {
        eprintln!("[macos-window] skipped (UI_HEADLESS=1)");
        return;
    }
    const NAME: &str = "ui-test-restore-state";
    ui::macos::forget_window_state(NAME);

    #[derive(Debug, Default)]
    struct Probe {
        first: Option<ui::window::Bounds>,
        restored: Option<ui::window::Bounds>,
        edited: (bool, bool),
        level: (Option<WindowLevel>, Option<WindowLevel>),
        chrome_titlebar: Option<f64>,
        page_edited: bool,
        second_instance: Option<Vec<String>>,
        second_acquire_was_none: bool,
    }
    let probe = Arc::new(Mutex::new(Probe::default()));
    let out = probe.clone();

    let guard = ui::SingleInstance::acquire("ui-test-single-instance")
        .expect("lock")
        .expect("first acquire is the primary");
    ui::run(move |app| {
        // 0. A second "launch" forwards its arguments to this one.
        let p_si = probe.clone();
        app.on_event(move |ev| {
            if let ui::event::Event::SecondInstance(args) = ev {
                p_si.lock().unwrap().second_instance = Some(args);
            }
        });
        guard.listen(app);
        let second = ui::SingleInstance::acquire("ui-test-single-instance").expect("lock");
        probe.lock().unwrap().second_acquire_was_none = second.is_none();

        // 1. Options at open time, then toggled at runtime.
        let win = Window::new("macos-options")
            .size(400, 300)
            .title_bar(TitleBar::inset(52))
            .restore_state(NAME)
            .macos(|m| {
                m.titlebar_separator(false)
                    .toolbar_style(ui::macos::ToolbarStyle::Unified)
                    .level(WindowLevel::Floating)
                    .window_controls_visible(false)
            })
            .open(app)?;
        let mac = win.macos();
        let level_open = mac.level();
        mac.set_level(WindowLevel::Normal);
        mac.set_document_edited(true);
        let edited_on = mac.document_edited();
        mac.set_document_edited(false);
        {
            let mut g = probe.lock().unwrap();
            g.level = (Some(level_open), Some(mac.level()));
            g.edited = (edited_on, mac.document_edited());
        }
        mac.set_window_controls_visible(true);

        // 2. A page reads the toolbar-style title bar height and uses setDocumentEdited.
        let p = probe.clone();
        let app2 = app.clone();
        WebView::html(
            r#"<script>
            (async () => {
              await ipc.invoke("ui:window.setDocumentEdited", true);
              const c = await ipc.invoke("ui:chrome");
              ipc.emit("chrome", c.titlebar_height);
            })();
            </script>"#,
        )
        .standard_rpcs(ui::StandardRpcs::none().window())
        .on("chrome", move |ctx, h: f64| {
            let edited = ctx.window().macos().document_edited();
            {
                let mut g = p.lock().unwrap();
                g.chrome_titlebar = Some(h);
                g.page_edited = edited;
            }
            ctx.window().macos().set_document_edited(false);
            // 3. Resize, close, and reopen under the same name.
            ctx.window().set_size(520, 340);
            let first = ctx.window().bounds();
            p.lock().unwrap().first = Some(first);
            ctx.window().close();
            let p2 = p.clone();
            let app3 = app2.clone();
            app2.dispatch(move |app| {
                let again = Window::new("macos-restored")
                    .size(400, 300)
                    .title_bar(TitleBar::inset(52))
                    .macos(|m| m.toolbar_style(ui::macos::ToolbarStyle::Unified))
                    .restore_state(NAME)
                    .open(app)
                    .expect("reopen");
                p2.lock().unwrap().restored = Some(again.bounds());
                again.close();
                app3.quit();
            });
        })
        .mount(&win)?;

        let app = app.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(15));
            app.quit();
        });
        Ok(())
    })
    .expect("ui::run failed");
    ui::macos::forget_window_state(NAME);

    let g = out.lock().unwrap();
    eprintln!("[macos-window] {g:?}");
    assert!(g.second_acquire_was_none, "a second acquire must not become primary");
    assert_eq!(
        g.second_instance,
        Some(std::env::args().skip(1).collect::<Vec<_>>()),
        "the second launch's arguments reach the primary"
    );
    assert_eq!(g.level, (Some(WindowLevel::Floating), Some(WindowLevel::Normal)));
    assert_eq!(g.edited, (true, false));
    assert!(g.page_edited, "ui:window.setDocumentEdited from the page");
    let h = g.chrome_titlebar.expect("page reported chrome");
    assert!((40.0..80.0).contains(&h), "Unified toolbar title bar height, got {h}");
    let (first, restored) = (g.first.expect("first"), g.restored.expect("restored"));
    assert_eq!((restored.width, restored.height), (first.width, first.height), "size restored");
    assert_eq!((restored.x, restored.y), (first.x, first.y), "position restored");
    eprintln!("[macos-window] ALL CHECKS PASSED");
}

#[cfg(not(target_os = "macos"))]
fn main() {}
