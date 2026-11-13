//! Parented native file and message dialogs (`NSOpenPanel`/`NSAlert` sheets on macOS,
//! owner-HWND dialogs on Windows, XDG Portal / transient parent on Linux).

use std::path::PathBuf;
use std::sync::Arc;

use crate::backend::Runtime;
use crate::window::WindowId;

/// Dialog handle attached to a parent `Window` (`win.dialog()`).
#[derive(Clone)]
pub struct Dialog {
    pub(crate) window: WindowId,
    pub(crate) runtime: Arc<Runtime>,
}

impl std::fmt::Debug for Dialog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Dialog")
            .field("window", &self.window)
            .finish()
    }
}

impl Dialog {
    /// Picks a single folder via a native modal/sheet dialog parented to this window.
    pub fn pick_folder(&self) -> Option<PathBuf> {
        self.file().pick_folder()
    }

    /// Picks a single file via a native modal/sheet dialog parented to this window.
    pub fn pick_file(&self) -> Option<PathBuf> {
        self.file().pick_file()
    }

    /// Picks one or more files via a native modal/sheet dialog parented to this window.
    pub fn pick_files(&self) -> Vec<PathBuf> {
        self.file().pick_files()
    }

    /// Shows a native Save File dialog parented to this window.
    pub fn save_file(&self) -> Option<PathBuf> {
        self.file().save_file()
    }

    /// Configures a custom file open/folder/save dialog parented to this window.
    pub fn file(&self) -> FileDialog {
        FileDialog {
            window: self.window,
            runtime: self.runtime.clone(),
            spec: FileDialogSpec::default(),
        }
    }

    /// Configures a native alert/confirmation dialog parented to this window.
    pub fn message(&self, title: impl Into<String>, body: impl Into<String>) -> MessageDialog {
        MessageDialog {
            window: self.window,
            runtime: self.runtime.clone(),
            spec: MessageDialogSpec {
                title: title.into(),
                body: body.into(),
                level: MessageLevel::Info,
                buttons: MessageButtons::Ok,
            },
        }
    }
}

/// Extension filter for file dialogs (for example `Filter::new("Images", ["png", "jpg"])`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filter {
    pub name: String,
    pub extensions: Vec<String>,
}

impl Filter {
    pub fn new<S: Into<String>>(
        name: impl Into<String>,
        extensions: impl IntoIterator<Item = S>,
    ) -> Self {
        Self {
            name: name.into(),
            extensions: extensions.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct FileDialogSpec {
    pub title: Option<String>,
    pub directory: Option<PathBuf>,
    pub default_name: Option<String>,
    pub filters: Vec<Filter>,
}

/// Builder for parented file/folder dialogs.
pub struct FileDialog {
    window: WindowId,
    runtime: Arc<Runtime>,
    spec: FileDialogSpec,
}

impl FileDialog {
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.spec.title = Some(title.into());
        self
    }

    pub fn directory(mut self, dir: impl Into<PathBuf>) -> Self {
        self.spec.directory = Some(dir.into());
        self
    }

    pub fn default_name(mut self, name: impl Into<String>) -> Self {
        self.spec.default_name = Some(name.into());
        self
    }

    pub fn filter(
        mut self,
        name: impl Into<String>,
        extensions: impl IntoIterator<Item = &'static str>,
    ) -> Self {
        self.spec.filters.push(Filter::new(name, extensions));
        self
    }

    pub fn pick_file(self) -> Option<PathBuf> {
        self.runtime
            .run_file_dialog(self.window, FileDialogKind::OpenFile, self.spec)
            .into_iter()
            .next()
    }

    pub fn pick_files(self) -> Vec<PathBuf> {
        self.runtime
            .run_file_dialog(self.window, FileDialogKind::OpenFiles, self.spec)
    }

    pub fn pick_folder(self) -> Option<PathBuf> {
        self.runtime
            .run_file_dialog(self.window, FileDialogKind::PickFolder, self.spec)
            .into_iter()
            .next()
    }

    pub fn save_file(self) -> Option<PathBuf> {
        self.runtime
            .run_file_dialog(self.window, FileDialogKind::SaveFile, self.spec)
            .into_iter()
            .next()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FileDialogKind {
    OpenFile,
    OpenFiles,
    PickFolder,
    SaveFile,
}

/// Severity icon shown in native message dialogs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MessageLevel {
    #[default]
    Info,
    Warning,
    Error,
}

/// Button set for native message dialogs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MessageButtons {
    #[default]
    Ok,
    OkCancel,
    YesNo,
    YesNoCancel,
}

/// User selection returned by `MessageDialog::show()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageResult {
    Ok,
    Cancel,
    Yes,
    No,
}

#[derive(Debug, Clone)]
pub(crate) struct MessageDialogSpec {
    pub title: String,
    pub body: String,
    pub level: MessageLevel,
    pub buttons: MessageButtons,
}

/// Builder for parented native alert/confirmation dialogs.
pub struct MessageDialog {
    window: WindowId,
    runtime: Arc<Runtime>,
    spec: MessageDialogSpec,
}

impl MessageDialog {
    pub fn level(mut self, level: MessageLevel) -> Self {
        self.spec.level = level;
        self
    }

    pub fn buttons(mut self, buttons: MessageButtons) -> Self {
        self.spec.buttons = buttons;
        self
    }

    pub fn show(self) -> MessageResult {
        self.runtime.run_message_dialog(self.window, self.spec)
    }
}
