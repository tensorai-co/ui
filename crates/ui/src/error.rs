//! Error and result types returned across `ui`.

use std::fmt;

use crate::ipc::DecodeError;

/// Unified result type for `ui` operations.
pub type Result<T> = std::result::Result<T, Error>;

/// Errors produced by host windowing, webview engines, IPC, or packaging integration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Operation is unsupported on the active OS or engine (for example `open_devtools()` on `webkit`).
    Unsupported(&'static str),
    /// Native OS windowing or framework error.
    Platform(String),
    /// Embedded webview engine error.
    Backend(String),
    /// Typed IPC encoding or decoding error.
    Ipc(DecodeError),
    /// Operation attempted in an invalid lifecycle state (for example closed window or non-main thread init).
    InvalidState(String),
    /// Filesystem or OS process error.
    Io(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported(msg) => write!(f, "unsupported: {msg}"),
            Self::Platform(msg) => write!(f, "platform error: {msg}"),
            Self::Backend(msg) => write!(f, "backend error: {msg}"),
            Self::Ipc(err) => write!(f, "ipc decode error: {err}"),
            Self::InvalidState(msg) => write!(f, "invalid state: {msg}"),
            Self::Io(msg) => write!(f, "io error: {msg}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Ipc(err) => Some(err),
            _ => None,
        }
    }
}

impl From<DecodeError> for Error {
    fn from(err: DecodeError) -> Self {
        Self::Ipc(err)
    }
}

impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err.to_string())
    }
}
