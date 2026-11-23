//! Standard page calls (`ui.app.*`, `ui.dialog.*`, `ui.clipboard.*`, `ui.window.*`,
//! `ui.notify`) that apps opt into with [`WebViewBuilder::standard_rpcs`].
//!
//! Each call is an ordinary RPC named `ui:<group>.<name>`, so `ipc.invoke` and the
//! `ui-client` package reach them the same way as app RPCs.
//!
//! [`WebViewBuilder::standard_rpcs`]: crate::webview::WebViewBuilder::standard_rpcs

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::dialog::{Filter, MessageButtons, MessageLevel, MessageResult};
use crate::ipc::Value;
use crate::webview::{Context, WebViewBuilder};
use crate::window::WindowState;
use crate::{Error, Result};

/// Which groups of standard page calls a webview exposes.
///
/// ```ignore
/// WebView::new()
///     .standard_rpcs(StandardRpcs::all().version(env!("CARGO_PKG_VERSION")))
/// // or only some groups:
///     .standard_rpcs(StandardRpcs::none().app().dialog())
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StandardRpcs {
    app: bool,
    dialog: bool,
    clipboard: bool,
    window: bool,
    notify: bool,
    webview: bool,
    version: Option<String>,
}

impl StandardRpcs {
    /// Every group.
    pub fn all() -> Self {
        Self {
            app: true,
            dialog: true,
            clipboard: true,
            window: true,
            notify: true,
            webview: true,
            version: None,
        }
    }

    /// No groups; add them one by one.
    pub fn none() -> Self {
        Self::default()
    }

    /// `ui.app.info()` and `ui.app.openExternal(url)` (http, https and mailto only).
    pub fn app(mut self) -> Self {
        self.app = true;
        self
    }

    /// `ui.dialog.pickFile / pickFiles / pickFolder / saveFile / message / confirm`, as window sheets.
    pub fn dialog(mut self) -> Self {
        self.dialog = true;
        self
    }

    /// `ui.clipboard.readText / writeText`, which also work on `https://` pages.
    pub fn clipboard(mut self) -> Self {
        self.clipboard = true;
        self
    }

    /// `ui.window.minimize / maximize / setFullscreen / setTitle / state / close / setDocumentEdited`.
    pub fn window(mut self) -> Self {
        self.window = true;
        self
    }

    /// `ui.notify(title, body, { id, subtitle, sound, onClick })` and `ui.notification.remove(id)`.
    pub fn notify(mut self) -> Self {
        self.notify = true;
        self
    }

    /// `ui.webview.find / clearFind / print / pdf`.
    pub fn webview(mut self) -> Self {
        self.webview = true;
        self
    }

    /// The app version `ui.app.info()` reports, e.g. `env!("CARGO_PKG_VERSION")`.
    /// Defaults to the bundle's `CFBundleShortVersionString` when bundled.
    pub fn version(mut self, version: impl Into<String>) -> Self {
        self.version = Some(version.into());
        self
    }
}

crate::ipc_struct! {
    #[derive(Debug, Clone, Default)]
    struct FilterSpec {
        name: String,
        extensions: Vec<String>,
    }
}

crate::ipc_struct! {
    #[derive(Debug, Clone, Default)]
    struct FileOptions {
        title: Option<String>,
        directory: Option<String>,
        default_name: Option<String>,
        filters: Option<Vec<FilterSpec>>,
    }
}

crate::ipc_struct! {
    #[derive(Debug, Clone, Default)]
    struct FindRequest {
        text: String,
        case_sensitive: Option<bool>,
        backwards: Option<bool>,
        wraps: Option<bool>,
    }
}

crate::ipc_struct! {
    #[derive(Debug, Clone, Default)]
    struct NotifyOptions {
        id: Option<String>,
        subtitle: Option<String>,
        sound: Option<bool>,
    }
}

crate::ipc_struct! {
    #[derive(Debug, Clone, Default)]
    struct MessageOptions {
        title: String,
        body: Option<String>,
        level: Option<String>,
        buttons: Option<String>,
    }
}

fn file_dialog(ctx: &Context, opts: Option<FileOptions>) -> crate::dialog::FileDialog {
    let opts = opts.unwrap_or_default();
    let mut d = ctx.window().dialog().file();
    if let Some(t) = opts.title {
        d = d.title(t);
    }
    if let Some(dir) = opts.directory {
        d = d.directory(PathBuf::from(dir));
    }
    if let Some(n) = opts.default_name {
        d = d.default_name(n);
    }
    d.filters(
        opts.filters
            .unwrap_or_default()
            .into_iter()
            .map(|f| Filter::new(f.name, f.extensions)),
    )
}

fn path_value(p: Option<PathBuf>) -> Value {
    p.map_or(Value::Null, |p| {
        Value::String(p.to_string_lossy().into_owned())
    })
}

fn parse_level(s: Option<&str>) -> Result<MessageLevel> {
    Ok(match s.unwrap_or("info") {
        "info" => MessageLevel::Info,
        "warning" => MessageLevel::Warning,
        "error" => MessageLevel::Error,
        other => {
            return Err(Error::InvalidState(format!(
                "unknown dialog level \"{other}\""
            )));
        }
    })
}

fn parse_buttons(s: Option<&str>) -> Result<MessageButtons> {
    Ok(match s.unwrap_or("ok") {
        "ok" => MessageButtons::Ok,
        "ok_cancel" => MessageButtons::OkCancel,
        "yes_no" => MessageButtons::YesNo,
        "yes_no_cancel" => MessageButtons::YesNoCancel,
        other => {
            return Err(Error::InvalidState(format!(
                "unknown dialog buttons \"{other}\""
            )));
        }
    })
}

fn result_name(r: MessageResult) -> &'static str {
    match r {
        MessageResult::Ok => "ok",
        MessageResult::Cancel => "cancel",
        MessageResult::Yes => "yes",
        MessageResult::No => "no",
    }
}

/// Only web and mail links leave the app; `file:`, custom schemes and the like don't.
pub(crate) fn external_url_allowed(url: &str) -> bool {
    let Some((scheme, rest)) = url.split_once(':') else {
        return false;
    };
    match scheme.to_ascii_lowercase().as_str() {
        "http" | "https" => rest.starts_with("//") && rest.len() > 2,
        "mailto" => !rest.is_empty(),
        _ => false,
    }
}

fn state_name(s: WindowState) -> &'static str {
    match s {
        WindowState::Normal => "normal",
        WindowState::Minimized => "minimized",
        WindowState::Maximized => "maximized",
        WindowState::Fullscreen => "fullscreen",
    }
}

pub(crate) fn install(mut b: WebViewBuilder, cfg: StandardRpcs) -> WebViewBuilder {
    if cfg.app {
        let version = cfg.version.clone();
        b = b
            .invoke("ui:app.info", move |ctx: Context, ()| {
                let app = ctx.app();
                let info = app.info();
                let platform = if cfg!(target_os = "macos") {
                    "macos"
                } else if cfg!(windows) {
                    "windows"
                } else {
                    "linux"
                };
                let s = |v: &str| Value::String(v.to_owned());
                Value::Object(BTreeMap::from([
                    ("name".into(), s(&info.name)),
                    (
                        "version".into(),
                        version
                            .clone()
                            .or(info.version)
                            .map_or(Value::Null, Value::String),
                    ),
                    ("backend".into(), s(app.backend())),
                    ("platform".into(), s(platform)),
                ]))
            })
            .invoke(
                "ui:app.openExternal",
                |ctx: Context, url: String| -> Result<()> {
                    if !external_url_allowed(&url) {
                        return Err(Error::InvalidState(format!(
                            "openExternal only opens http, https and mailto URLs, not \"{url}\""
                        )));
                    }
                    ctx.app().open_external(&url)
                },
            );
    }
    if cfg.dialog {
        b = b
            .invoke(
                "ui:dialog.pickFile",
                |ctx: Context, o: Option<FileOptions>| path_value(file_dialog(&ctx, o).pick_file()),
            )
            .invoke(
                "ui:dialog.pickFiles",
                |ctx: Context, o: Option<FileOptions>| {
                    file_dialog(&ctx, o)
                        .pick_files()
                        .into_iter()
                        .map(|p| p.to_string_lossy().into_owned())
                        .collect::<Vec<_>>()
                },
            )
            .invoke(
                "ui:dialog.pickFolder",
                |ctx: Context, o: Option<FileOptions>| {
                    path_value(file_dialog(&ctx, o).pick_folder())
                },
            )
            .invoke(
                "ui:dialog.saveFile",
                |ctx: Context, o: Option<FileOptions>| path_value(file_dialog(&ctx, o).save_file()),
            )
            .invoke(
                "ui:dialog.message",
                |ctx: Context, o: MessageOptions| -> Result<String> {
                    let r = ctx
                        .window()
                        .dialog()
                        .message(o.title, o.body.unwrap_or_default())
                        .level(parse_level(o.level.as_deref())?)
                        .buttons(parse_buttons(o.buttons.as_deref())?)
                        .show();
                    Ok(result_name(r).to_owned())
                },
            )
            .invoke(
                "ui:dialog.confirm",
                |ctx: Context, o: MessageOptions| -> Result<bool> {
                    let r = ctx
                        .window()
                        .dialog()
                        .message(o.title, o.body.unwrap_or_default())
                        .level(parse_level(o.level.as_deref())?)
                        .buttons(MessageButtons::OkCancel)
                        .show();
                    Ok(r == MessageResult::Ok)
                },
            );
    }
    if cfg.clipboard {
        b = b
            .invoke("ui:clipboard.readText", |ctx: Context, ()| {
                ctx.app().clipboard_text()
            })
            .invoke("ui:clipboard.writeText", |ctx: Context, text: String| {
                ctx.app().set_clipboard_text(&text);
            });
    }
    if cfg.window {
        b = b
            .invoke("ui:window.minimize", |ctx: Context, ()| {
                ctx.window().minimize()
            })
            .invoke("ui:window.maximize", |ctx: Context, ()| {
                ctx.window().maximize()
            })
            .invoke("ui:window.setFullscreen", |ctx: Context, on: bool| {
                ctx.window().set_fullscreen(on);
            })
            .invoke("ui:window.setTitle", |ctx: Context, title: String| {
                ctx.window().set_title(&title);
            })
            .invoke("ui:window.state", |ctx: Context, ()| {
                state_name(ctx.window().state()).to_owned()
            })
            .invoke("ui:window.close", |ctx: Context, ()| ctx.window().close())
            .invoke(
                "ui:window.setDocumentEdited",
                |ctx: Context, edited: bool| {
                    #[cfg(target_os = "macos")]
                    {
                        use crate::macos::WindowExt;
                        ctx.window().macos().set_document_edited(edited);
                    }
                    #[cfg(not(target_os = "macos"))]
                    let _ = (ctx, edited);
                },
            );
    }
    if cfg.webview {
        b = b
            // Worker-thread calls: they wait for the engine's completion handler.
            .invoke_blocking("ui:webview.find", |ctx: Context, req: FindRequest| {
                let (tx, rx) = std::sync::mpsc::channel();
                let defaults = crate::webview::FindOptions::default();
                let opts = crate::webview::FindOptions {
                    case_sensitive: req.case_sensitive.unwrap_or(defaults.case_sensitive),
                    backwards: req.backwards.unwrap_or(defaults.backwards),
                    wraps: req.wraps.unwrap_or(defaults.wraps),
                };
                ctx.webview().find(&req.text, opts, move |found| {
                    let _ = tx.send(found);
                });
                rx.recv_timeout(std::time::Duration::from_secs(10))
                    .ok()
                    .flatten()
            })
            .invoke("ui:webview.clearFind", |ctx: Context, ()| {
                ctx.webview().clear_find()
            })
            .invoke("ui:webview.print", |ctx: Context, ()| ctx.webview().print())
            .invoke_blocking(
                "ui:webview.pdf",
                |ctx: Context, ()| -> Result<crate::ipc::Bytes> {
                    let (tx, rx) = std::sync::mpsc::channel();
                    ctx.webview().pdf(move |bytes| {
                        let _ = tx.send(bytes);
                    });
                    match rx.recv_timeout(std::time::Duration::from_secs(60)) {
                        Ok(Some(bytes)) => Ok(crate::ipc::Bytes(bytes)),
                        Ok(None) => Err(Error::Unsupported(
                            "pdf export isn't available on this engine",
                        )),
                        Err(_) => Err(Error::Backend("pdf export timed out".into())),
                    }
                },
            );
    }
    if cfg.notify {
        b = b
            .invoke(
                "ui:notify",
                |ctx: Context, (title, body, opts): (String, Option<String>, Option<NotifyOptions>)| {
                    let opts = opts.unwrap_or_default();
                    let mut n = crate::app::Notification::new(title)
                        .body(body.unwrap_or_default())
                        .sound(opts.sound.unwrap_or(true));
                    if let Some(id) = opts.id {
                        n = n.id(id);
                    }
                    if let Some(sub) = opts.subtitle {
                        n = n.subtitle(sub);
                    }
                    ctx.app().show_notification(n)
                },
            )
            .invoke("ui:notification.remove", |ctx: Context, id: String| {
                ctx.app().remove_notification(&id);
            });
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_urls_are_web_or_mail_only() {
        for ok in [
            "https://example.com",
            "http://localhost:3000/x",
            "mailto:a@b.c",
            "HTTPS://X.Y",
        ] {
            assert!(external_url_allowed(ok), "{ok}");
        }
        for bad in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "myapp://x",
            "https:",
            "mailto:",
            "",
            "/relative",
        ] {
            assert!(!external_url_allowed(bad), "{bad}");
        }
    }

    #[test]
    fn dialog_option_names() {
        assert_eq!(parse_level(None).unwrap(), MessageLevel::Info);
        assert_eq!(
            parse_buttons(Some("yes_no_cancel")).unwrap(),
            MessageButtons::YesNoCancel
        );
        assert!(parse_level(Some("loud")).is_err());
        assert_eq!(result_name(MessageResult::No), "no");
    }
}
