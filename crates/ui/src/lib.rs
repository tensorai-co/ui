//! A from-scratch, zero-third-party-crate Rust UI shell and webview framework for single-page desktop apps.

pub mod app;
mod call;
pub mod dialog;
pub mod embed;
pub mod error;
pub mod event;
pub mod ipc;
pub mod menu;
pub mod platform;
pub mod webview;
pub mod window;

mod backend;
mod js_shim;

pub use app::{App, SingleInstance, run};
pub use call::Stream;
pub use dialog::Dialog;
pub use error::{Error, Result};
pub use ipc::{Bytes, DecodeError, FromIpc, IntoIpc, RpcResult, Value};
pub use menu::Menu;
pub use webview::{Ctx, Nav, NavRequest, Profile, Request, Response, WebView};
pub use window::{Backdrop, Tabbing, TitleBar, Window};
