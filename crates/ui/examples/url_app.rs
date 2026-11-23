//! Example matching README Quickstart: URL-based desktop app + typed `ipc_enum!` commands.

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
        let target_url =
            std::env::var("UI_URL").unwrap_or_else(|_| "https://example.com".to_owned());
        let win = Window::new("Example App")
            .size(1200, 800)
            .title_bar(TitleBar::Native)
            .macos(|m| m.tabbing(Tabbing::Disallowed))
            .open(app)?;

        let allowed_host = target_url
            .split_once("://")
            .and_then(|(_, rest)| rest.split(['/', '?', '#']).next())
            .unwrap_or("example.com")
            .to_owned();

        WebView::url(target_url)
            .profile(Profile::persistent("default"))
            .on_navigate(move |req| match req.host() {
                Some(h) if h == allowed_host => NavDecision::Allow,
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
