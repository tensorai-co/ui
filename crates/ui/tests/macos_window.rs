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

    /// A window's content width, height and center.
    type Measure = (u32, u32, i32, i32);

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
        zoom_restored: Option<f64>,
        /// Before and after `set_size_centered`.
        centered: Option<(Measure, Measure)>,
        pixel_scale: Option<f64>,
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

        // Page zoom is remembered per profile.
        {
            let zw = Window::new("zoom").size(300, 200).open(app)?;
            let profile = ui::Profile::persistent("ui-test-zoom");
            let a = WebView::html("<p>a</p>")
                .profile(profile.clone())
                .mount(&zw)?;
            a.set_zoom(1.25);
            let b = WebView::html("<p>b</p>").profile(profile).mount(&zw)?;
            probe.lock().unwrap().zoom_restored = Some(b.zoom());
            b.set_zoom(1.0); // leave the test profile at 100%
            zw.close();
        }

        // A fixed-size window grows about its center into a resizable one.
        {
            let sw = Window::new("size-centered")
                .size(400, 300)
                .min_size(400, 300)
                .max_size(400, 300)
                .resizable(false)
                .title_bar(TitleBar::overlay(48))
                .open(app)?;
            let measure = |b: ui::window::Bounds| -> Measure {
                (
                    b.width,
                    b.height,
                    b.x + b.width as i32 / 2,
                    b.y + b.height as i32 / 2,
                )
            };
            let before = measure(sw.bounds());
            sw.set_max_size(None);
            sw.set_min_size(Some((480, 360)));
            sw.set_resizable(true);
            sw.set_size_centered(600, 420, false);
            probe.lock().unwrap().centered = Some((before, measure(sw.bounds())));

            // A pixel view over part of it: placed, sized at the window's scale, drawn into, focused, closed.
            let view = ui::PixelView::new(&sw, |_| {}).expect("a pixel view");
            view.set_frame(300.0, 0.0, 300.0, 420.0);
            let scale = view.scale();
            view.resize((300.0 * scale) as i32, (420.0 * scale) as i32, 0x202020);
            view.draw(0, 0, 2, 2, vec![0xff0000; 4]);
            view.set_cursor("text");
            view.focus();
            probe.lock().unwrap().pixel_scale = Some(scale);
            view.close();
            sw.close();
        }

        // 1. Options at open time, then toggled at runtime.
        let win = Window::new("macos-options")
            .size(400, 300)
            .title_bar(TitleBar::overlay(52))
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
                    .title_bar(TitleBar::overlay(52))
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
    assert_eq!(
        g.zoom_restored,
        Some(1.25),
        "zoom restored for the same profile"
    );
    assert!(g.pixel_scale.is_some_and(|s| s >= 1.0), "a pixel view's scale: {:?}", g.pixel_scale);
    let (before, after) = g.centered.expect("set_size_centered ran");
    assert_eq!((before.0, before.1), (400, 300));
    assert_eq!(
        (after.0, after.1),
        (600, 420),
        "resized past the old max size"
    );
    assert!(
        (after.2 - before.2).abs() <= 1 && (after.3 - before.3).abs() <= 1,
        "kept its center: {before:?} -> {after:?}"
    );
    assert!(
        g.second_acquire_was_none,
        "a second acquire must not become primary"
    );
    assert_eq!(
        g.second_instance,
        Some(std::env::args().skip(1).collect::<Vec<_>>()),
        "the second launch's arguments reach the primary"
    );
    assert_eq!(
        g.level,
        (Some(WindowLevel::Floating), Some(WindowLevel::Normal))
    );
    assert_eq!(g.edited, (true, false));
    assert!(g.page_edited, "ui:window.setDocumentEdited from the page");
    let h = g.chrome_titlebar.expect("page reported chrome");
    assert!(
        (40.0..80.0).contains(&h),
        "Unified toolbar title bar height, got {h}"
    );
    let (first, restored) = (g.first.expect("first"), g.restored.expect("restored"));
    assert_eq!(
        (restored.width, restored.height),
        (first.width, first.height),
        "size restored"
    );
    assert_eq!(
        (restored.x, restored.y),
        (first.x, first.y),
        "position restored"
    );
    eprintln!("[macos-window] ALL CHECKS PASSED");
}

#[cfg(not(target_os = "macos"))]
fn main() {}
