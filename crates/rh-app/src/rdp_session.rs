//! Minimal live-RDP session registry.
//!
//! The SSH `SessionManager` carries scrollback, restore-on-reload and a
//! per-session ring buffer — none of which an RDP framebuffer needs (it's
//! a live raster, not a replayable byte stream). So RDP gets its own thin
//! registry: spawn the actor, forward its events to the UI `Channel`, and
//! keep the command sender so the UI can close (and, in 2b-2, send input).

use std::collections::HashMap;
use std::sync::Arc;

use tauri::ipc::Channel;
use tokio::sync::{mpsc, Mutex};
use tokio::task::JoinHandle;
use tracing::instrument;

use rh_core::SessionId;
use rh_rdp::{RdpCommand, RdpInputEvent, RdpSessionEvent};

struct RdpHandle {
    tx_cmd: mpsc::Sender<RdpCommand>,
    /// The actor's supervising task. Kept so the handle owns it; dropping
    /// the handle drops `tx_cmd`, which closes the command channel and lets
    /// the actor wind down gracefully.
    _join: JoinHandle<()>,
}

#[derive(Clone, Default)]
pub struct RdpSessionManager {
    inner: Arc<Mutex<HashMap<SessionId, RdpHandle>>>,
}

impl RdpSessionManager {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a freshly-spawned actor: start forwarding its events to the
    /// UI channel and remember its command sender.
    #[instrument(level = "debug", skip(self, tx_cmd, join, rx_events, on_event))]
    pub async fn register(
        &self,
        id: SessionId,
        tx_cmd: mpsc::Sender<RdpCommand>,
        join: JoinHandle<()>,
        mut rx_events: mpsc::UnboundedReceiver<RdpSessionEvent>,
        on_event: Channel<RdpSessionEvent>,
    ) {
        let inner = self.inner.clone();
        let id_for_cleanup = id.clone();
        tokio::spawn(async move {
            // Forward every event in order. Frames are region-diffed (small,
            // and each updates a *different* rectangle), so they must not be
            // dropped/coalesced — losing one would leave a stale patch on
            // screen. Their small size keeps the channel from backing up.
            while let Some(ev) = rx_events.recv().await {
                if on_event.send(ev).is_err() {
                    break; // UI channel gone (webview reloaded / tab closed)
                }
            }
            // Event stream ended → the actor is done. Drop the handle.
            inner.lock().await.remove(&id_for_cleanup);
        });

        self.inner
            .lock()
            .await
            .insert(id, RdpHandle { tx_cmd, _join: join });
    }

    /// Close a session: signal the actor, then drop the handle (which also
    /// closes the command channel as a backstop).
    #[instrument(level = "debug", skip(self))]
    pub async fn close(&self, id: &SessionId) {
        if let Some(handle) = self.inner.lock().await.remove(id) {
            let _ = handle.tx_cmd.send(RdpCommand::Shutdown).await;
        }
    }

    /// Send an input event to a live session (used from 2b-2 onward).
    #[instrument(level = "debug", skip(self, ev))]
    pub async fn send_input(&self, id: &SessionId, ev: RdpInputEvent) {
        if let Some(handle) = self.inner.lock().await.get(id) {
            let _ = handle.tx_cmd.send(RdpCommand::Input(ev)).await;
        }
    }

    pub async fn set_clipboard(&self, id: &SessionId, text: String) {
        if let Some(handle) = self.inner.lock().await.get(id) {
            let _ = handle.tx_cmd.send(RdpCommand::SetClipboard(text)).await;
        }
    }

    pub async fn set_clipboard_image(&self, id: &SessionId, width: u32, height: u32, rgba: Vec<u8>) {
        if let Some(handle) = self.inner.lock().await.get(id) {
            let _ = handle
                .tx_cmd
                .send(RdpCommand::SetClipboardImage {
                    width,
                    height,
                    rgba,
                })
                .await;
        }
    }

    pub async fn resize(&self, id: &SessionId, width: u16, height: u16) {
        if let Some(handle) = self.inner.lock().await.get(id) {
            let _ = handle
                .tx_cmd
                .send(RdpCommand::Resize { width, height })
                .await;
        }
    }
}
