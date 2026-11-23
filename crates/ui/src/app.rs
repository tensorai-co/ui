//! Application entrypoint (`ui::run`), global `App` handle, and single-instance lock.

use std::path::PathBuf;
use std::sync::Arc;

use crate::Result;
use crate::backend::{self, Runtime};
use crate::event::Event;
use crate::menu::Menu;
use crate::window::{Monitor, Theme, Window, WindowSpec};

/// Starts the desktop UI application, handles CEF subprocess re-entry transparently,
/// runs `init(&app)` on the main thread, and drives the host run-loop until exit.
pub fn run<F>(init: F) -> Result<()>
where
    F: FnOnce(&App) -> Result<()>,
{
    backend::run(init)
}

/// Cheap, `Clone + Send + Sync` handle to the running application.
#[derive(Clone)]
pub struct App {
    pub(crate) runtime: Arc<Runtime>,
}

impl App {
    pub fn backend(&self) -> &'static str {
        self.runtime.backend_name()
    }

    pub fn quit(&self) {
        self.runtime.quit();
    }

    pub fn notify(&self, title: &str, body: &str) {
        self.runtime.notify(title.to_owned(), body.to_owned());
    }

    /// Shows a notification and returns its id. Clicking it delivers
    /// `Event::NotificationClicked(id)` (and `ui:notification:click` to pages).
    ///
    /// From an `.app` bundle this uses the system notification center (asking
    /// for permission the first time) with the app's own icon. An unbundled
    /// `cargo run` binary falls back to `osascript`: no clicks or removal.
    pub fn show_notification(&self, notification: Notification) -> String {
        self.runtime.show_notification(notification)
    }

    /// Removes a notification shown (or queued) with [`show_notification`](Self::show_notification).
    pub fn remove_notification(&self, id: &str) {
        self.runtime.remove_notification(id.to_owned());
    }

    /// Reports whether notifications may be shown, without asking. `f` runs on
    /// a background thread.
    pub fn notification_permission(&self, f: impl FnOnce(NotificationPermission) + Send + 'static) {
        self.runtime.notification_permission(Box::new(f));
    }

    /// Asks for permission to show notifications (the system prompt appears
    /// once). `f` gets the answer on a background thread.
    pub fn request_notification_permission(&self, f: impl FnOnce(bool) + Send + 'static) {
        self.runtime.request_notification_permission(Box::new(f));
    }

    pub fn open_external(&self, url: &str) -> Result<()> {
        self.runtime.open_external(url)
    }

    /// The app's display name and, when bundled, its version.
    pub fn info(&self) -> AppInfo {
        self.runtime.app_info()
    }

    /// Plain text on the system clipboard.
    pub fn clipboard_text(&self) -> Option<String> {
        self.runtime.clipboard_text()
    }

    /// Replaces the system clipboard with plain text.
    pub fn set_clipboard_text(&self, text: &str) {
        self.runtime.set_clipboard_text(text.to_owned());
    }

    pub fn theme(&self) -> Theme {
        self.runtime.app_theme()
    }

    /// Connected monitors.
    pub fn monitors(&self) -> Vec<Monitor> {
        self.runtime.displays()
    }

    #[deprecated(since = "0.2.0", note = "renamed to `monitors`")]
    pub fn displays(&self) -> Vec<Monitor> {
        self.monitors()
    }

    /// Handle to an item of the current app menu by id, to change its title,
    /// enabled state or check mark.
    pub fn menu_item(&self, id: impl Into<String>) -> crate::menu::MenuItemHandle {
        crate::menu::MenuItemHandle {
            id: id.into(),
            runtime: self.runtime.clone(),
        }
    }

    pub fn menu(&self, menu: Menu) {
        self.runtime.set_menu(menu);
    }

    pub fn dispatch(&self, f: impl FnOnce(&App) + Send + 'static) {
        let app = self.clone();
        self.runtime.dispatch_on_main(Box::new(move || f(&app)));
    }

    pub fn on_event(&self, f: impl FnMut(Event) + Send + 'static) {
        self.runtime.set_app_event_handler(Box::new(f));
    }

    pub(crate) fn open_window(&self, spec: WindowSpec) -> Result<Window> {
        self.runtime.open_window(spec)
    }
}

impl std::fmt::Debug for App {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("App")
            .field("backend", &self.backend())
            .finish()
    }
}

/// Holds a process-lifetime single-instance lock backed by a Unix domain socket (macOS/Linux)
/// or named lock file/pipe.
///
/// A second launch's [`acquire`](Self::acquire) returns `Ok(None)` and sends its
/// command-line arguments to the running instance. There, call
/// [`listen`](Self::listen) to receive them as `Event::SecondInstance(args)`,
/// with any URL arguments also delivered as `Event::OpenUrl`.
pub struct SingleInstance {
    #[cfg(unix)]
    listener: std::os::unix::net::UnixListener,
    path: PathBuf,
}

impl SingleInstance {
    /// Attempts to claim the single-instance lock for `app_id`.
    /// Returns `Ok(Some(guard))` if this process is the primary instance, or `Ok(None)` if
    /// another live instance already holds the lock.
    pub fn acquire(app_id: &str) -> Result<Option<Self>> {
        let sanitized: String = app_id
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        #[cfg(unix)]
        {
            use std::os::unix::net::{UnixListener, UnixStream};

            let path = std::env::temp_dir().join(format!("ui-lock-{sanitized}.sock"));
            if path.exists() {
                if let Ok(mut stream) = UnixStream::connect(&path) {
                    use std::io::Write;
                    let args: Vec<String> = std::env::args().skip(1).collect();
                    let _ = stream.write_all(encode_args(&args).as_slice());
                    return Ok(None);
                }
                let _ = std::fs::remove_file(&path);
            }
            match UnixListener::bind(&path) {
                Ok(listener) => Ok(Some(Self { listener, path })),
                Err(err) => Err(crate::Error::Io(format!(
                    "single-instance lock failed at {}: {err}",
                    path.display()
                ))),
            }
        }
        #[cfg(not(unix))]
        {
            let path = std::env::temp_dir().join(format!("ui-lock-{sanitized}.lock"));
            match std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&path)
            {
                Ok(_) => Ok(Some(Self { path })),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(None),
                Err(e) => Err(crate::Error::Io(e.to_string())),
            }
        }
    }

    /// Delivers later launches' arguments to `app` as `Event::SecondInstance`
    /// (and URL arguments as `Event::OpenUrl`, also sent to pages). Call it once,
    /// inside `ui::run`.
    pub fn listen(&self, app: &App) {
        #[cfg(unix)]
        {
            use std::io::Read;
            let Ok(listener) = self.listener.try_clone() else {
                return;
            };
            let _ = listener.set_nonblocking(false);
            let runtime = app.runtime.clone();
            let _ = std::thread::Builder::new()
                .name("ui-single-instance".into())
                .spawn(move || {
                    for stream in listener.incoming() {
                        let Ok(mut stream) = stream else { continue };
                        let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(2)));
                        let mut buf = Vec::new();
                        if stream.read_to_end(&mut buf).is_err() || buf.is_empty() {
                            continue;
                        }
                        let args = decode_args(&buf);
                        let rt = runtime.clone();
                        runtime.dispatch_on_main(Box::new(move || rt.handle_second_instance(args)));
                    }
                });
        }
        #[cfg(not(unix))]
        let _ = app;
    }
}

/// Version byte that starts a forwarded launch, so one with no arguments still arrives.
const FORWARD_V1: u8 = 0x01;

/// Second-launch arguments on the wire: a version byte, then each argument
/// followed by a NUL byte.
#[cfg_attr(not(unix), allow(dead_code))]
fn encode_args(args: &[String]) -> Vec<u8> {
    let mut out = vec![FORWARD_V1];
    for a in args {
        out.extend_from_slice(a.replace('\0', "").as_bytes());
        out.push(0);
    }
    out
}

#[cfg_attr(not(unix), allow(dead_code))]
fn decode_args(buf: &[u8]) -> Vec<String> {
    let body = match buf.split_first() {
        Some((&FORWARD_V1, rest)) => rest,
        _ => buf,
    };
    body.split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_instance_args_round_trip() {
        let args = vec![
            "myapp://open?x=1".to_owned(),
            "--flag".to_owned(),
            "a b.txt".to_owned(),
        ];
        assert_eq!(decode_args(&encode_args(&args)), args);
        assert!(decode_args(&encode_args(&[])).is_empty());
    }
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// The app's name (`CFBundleName` or the process name) and bundle version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppInfo {
    pub name: String,
    pub version: Option<String>,
}

/// Permission to show notifications.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationPermission {
    /// Not asked yet; the first notification asks.
    NotDetermined,
    Denied,
    Authorized,
    /// Not running from an app bundle: notifications use a fallback that can't
    /// be clicked or removed.
    Unbundled,
}

/// A notification for [`App::show_notification`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    pub(crate) id: Option<String>,
    pub(crate) title: String,
    pub(crate) subtitle: Option<String>,
    pub(crate) body: String,
    pub(crate) sound: bool,
}

impl Notification {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            id: None,
            title: title.into(),
            subtitle: None,
            body: String::new(),
            sound: true,
        }
    }

    pub fn body(mut self, body: impl Into<String>) -> Self {
        self.body = body.into();
        self
    }

    pub fn subtitle(mut self, subtitle: impl Into<String>) -> Self {
        self.subtitle = Some(subtitle.into());
        self
    }

    /// A stable id: showing another notification with the same id replaces it.
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn sound(mut self, on: bool) -> Self {
        self.sound = on;
        self
    }
}
