//! Per-call state for `invoke_blocking` / `invoke_stream`: cancellation and streams.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, mpsc};
use std::time::Duration;

use crate::ipc::{IntoIpc, Value};
use crate::webview::WebView;
use crate::{Error, Result};

/// Event channel prefix for data the host writes into an open stream.
pub(crate) const STREAM_DATA_PREFIX: &str = "__ui_stream:";
/// Event channel prefix telling the page a stream is registered, so data and
/// `close()` it sends from now on can't overtake the open request.
pub(crate) const STREAM_OPEN_PREFIX: &str = "__ui_stream_open:";
/// Event channel prefix for a stream's final `{ ok }` / `{ error }` frame.
pub(crate) const STREAM_END_PREFIX: &str = "__ui_stream_end:";

/// Shared state for one page call that has a call id.
pub(crate) struct CallState {
    cancelled: AtomicBool,
    inbound_tx: Mutex<Option<mpsc::Sender<Value>>>,
    inbound_rx: Mutex<Option<mpsc::Receiver<Value>>>,
}

impl CallState {
    pub(crate) fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            cancelled: AtomicBool::new(false),
            inbound_tx: Mutex::new(Some(tx)),
            inbound_rx: Mutex::new(Some(rx)),
        }
    }

    /// Marks the call cancelled and disconnects the inbound channel, so a
    /// blocked `Stream::recv` returns `None`.
    pub(crate) fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        self.inbound_tx
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    pub(crate) fn push_inbound(&self, value: Value) {
        if let Some(tx) = self
            .inbound_tx
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
        {
            let _ = tx.send(value);
        }
    }

    pub(crate) fn take_inbound(&self) -> Option<mpsc::Receiver<Value>> {
        self.inbound_rx
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
    }
}

/// Both ends of one `ipc.stream(..)` call, handed to an `invoke_stream` handler.
///
/// `send` writes frames the page receives with `stream.on("data", ..)`. `recv`
/// reads what the page writes with `stream.send(..)`. The handler's return value
/// resolves the page's `stream.closed` promise; an `Err` rejects it.
///
/// When the page calls `stream.close()`, or the webview goes away, the stream is
/// closed: `send` fails, `recv` returns `None`, and `is_closed` turns true, so the
/// handler can stop its work (and drop the child process, file, or socket).
pub struct Stream {
    pub(crate) webview: WebView,
    pub(crate) data_channel: String,
    pub(crate) state: std::sync::Arc<CallState>,
    pub(crate) inbound: mpsc::Receiver<Value>,
}

impl Stream {
    /// Sends one frame to the page. `Bytes` travel as raw binary frames.
    pub fn send(&self, data: impl IntoIpc) -> Result<()> {
        if self.state.is_cancelled() {
            return Err(Error::InvalidState("stream closed by the page".into()));
        }
        self.webview.emit(&self.data_channel, data);
        Ok(())
    }

    /// Blocks until the page sends data, or returns `None` once the stream is closed.
    pub fn recv(&self) -> Option<Value> {
        self.inbound.recv().ok()
    }

    /// Returns data the page already sent, without blocking.
    pub fn try_recv(&self) -> Option<Value> {
        self.inbound.try_recv().ok()
    }

    /// Like `recv`, giving up after `timeout`.
    pub fn recv_timeout(
        &self,
        timeout: Duration,
    ) -> std::result::Result<Value, mpsc::RecvTimeoutError> {
        self.inbound.recv_timeout(timeout)
    }

    /// True once the page closed the stream or the webview went away.
    pub fn is_closed(&self) -> bool {
        self.state.is_cancelled()
    }

    /// Ends the stream; `value` resolves the page's `stream.closed`.
    pub fn close_with<T>(self, value: T) -> T {
        value
    }

    /// The webview this stream belongs to.
    pub fn webview(&self) -> WebView {
        self.webview.clone()
    }
}

impl std::fmt::Debug for Stream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Stream")
            .field("channel", &self.data_channel)
            .field("closed", &self.is_closed())
            .finish()
    }
}
