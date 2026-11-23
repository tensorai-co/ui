//! Application entrypoint (`ui::run`), global `App` handle, and single-instance lock.

use std::path::PathBuf;
use std::sync::Arc;

use crate::Result;
use crate::backend::{self, Runtime};
use crate::event::Event;
use crate::menu::Menu;
use crate::window::{Display, Theme, Window, WindowSpec};

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

    pub fn open_external(&self, url: &str) -> Result<()> {
        self.runtime.open_external(url)
    }

    pub fn theme(&self) -> Theme {
        self.runtime.app_theme()
    }

    pub fn displays(&self) -> Vec<Display> {
        self.runtime.displays()
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
pub struct SingleInstance {
    #[cfg(unix)]
    _listener: std::os::unix::net::UnixListener,
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
                if UnixStream::connect(&path).is_ok() {
                    return Ok(None);
                }
                let _ = std::fs::remove_file(&path);
            }
            match UnixListener::bind(&path) {
                Ok(listener) => {
                    let _ = listener.set_nonblocking(true);
                    Ok(Some(Self {
                        _listener: listener,
                        path,
                    }))
                }
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
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}
