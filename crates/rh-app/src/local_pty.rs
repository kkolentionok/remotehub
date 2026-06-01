//! Local PTY session: spawns the system shell in a pseudo-terminal
//! (ConPTY on Windows, openpty on Unix via `portable-pty`) and bridges it
//! to the **same** `SshSessionEvent` / `SessionCommand` contract the UI
//! terminal already speaks. That lets the frontend render a local shell
//! with the existing xterm.js `Terminal` component unchanged — only the
//! IPC entry points differ.
//!
//! Like RDP, this gets its own thin registry rather than going through the
//! SSH `SessionManager`: a local shell has no host, no host-key TOFU and no
//! restore-on-reload, so the scrollback/ring-buffer machinery there doesn't
//! apply. We just spawn the worker, forward its events to the UI `Channel`,
//! and keep the command sender so the UI can type / resize / close.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::Arc;

use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use tauri::ipc::Channel;
use tokio::sync::{mpsc, Mutex};
use tokio::task::JoinHandle;
use tracing::instrument;

use rh_core::SessionId;
use rh_ssh::{CloseReason, SessionCommand, SessionState, SshSessionEvent};

struct LocalHandle {
    tx_cmd: mpsc::Sender<SessionCommand>,
    /// The event forwarder task. Dropping the handle drops `tx_cmd`,
    /// which closes the command channel and lets the worker wind down.
    _join: JoinHandle<()>,
}

#[derive(Clone, Default)]
pub struct LocalPtyManager {
    inner: Arc<Mutex<HashMap<SessionId, LocalHandle>>>,
}

impl LocalPtyManager {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Spawn a shell PTY and start forwarding its byte stream to the UI.
    /// `shell` overrides the system default when non-empty.
    #[instrument(level = "debug", skip(self, on_event))]
    pub async fn open(
        &self,
        id: SessionId,
        cols: u16,
        rows: u16,
        shell: Option<String>,
        on_event: Channel<SshSessionEvent>,
    ) -> Result<(), String> {
        let (tx_events, mut rx_events) = mpsc::unbounded_channel::<SshSessionEvent>();
        let (tx_cmd, rx_cmd) = mpsc::channel::<SessionCommand>(64);

        // Open the PTY + spawn worker threads. Fails fast (e.g. shell not
        // found) so the caller can surface an error before registering.
        spawn_pty(cols, rows, shell, rx_cmd, tx_events)?;

        // Forward events to the UI channel in order; clean up the registry
        // once the event stream ends (worker gone).
        let inner = self.inner.clone();
        let id_cleanup = id.clone();
        let forwarder = tokio::spawn(async move {
            while let Some(ev) = rx_events.recv().await {
                if on_event.send(ev).is_err() {
                    break; // UI channel gone (tab closed / webview reload)
                }
            }
            inner.lock().await.remove(&id_cleanup);
        });

        self.inner
            .lock()
            .await
            .insert(id, LocalHandle { tx_cmd, _join: forwarder });
        Ok(())
    }

    #[instrument(level = "debug", skip(self, cmd))]
    pub async fn send(&self, id: &SessionId, cmd: SessionCommand) {
        if let Some(h) = self.inner.lock().await.get(id) {
            let _ = h.tx_cmd.send(cmd).await;
        }
    }

    #[instrument(level = "debug", skip(self))]
    pub async fn close(&self, id: &SessionId) {
        if let Some(h) = self.inner.lock().await.remove(id) {
            let _ = h.tx_cmd.send(SessionCommand::Shutdown).await;
        }
    }
}

/// Pick the system shell. PowerShell on Windows; `$SHELL` (or bash) on Unix.
fn default_shell() -> String {
    #[cfg(windows)]
    {
        "powershell.exe".to_string()
    }
    #[cfg(not(windows))]
    {
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string())
    }
}

fn home_dir() -> Option<String> {
    #[cfg(windows)]
    {
        std::env::var("USERPROFILE").ok().filter(|s| !s.is_empty())
    }
    #[cfg(not(windows))]
    {
        std::env::var("HOME").ok().filter(|s| !s.is_empty())
    }
}

/// Open a PTY, spawn the shell, and wire two workers:
/// - a blocking **reader thread** that pumps PTY output → `Data` events,
///   and emits `Closed` on EOF (shell exited / PTY torn down);
/// - a **command task** that owns the write side + child and applies
///   `SshInput` / `Resize` / `Shutdown`.
fn spawn_pty(
    cols: u16,
    rows: u16,
    shell: Option<String>,
    mut rx_cmd: mpsc::Receiver<SessionCommand>,
    tx_events: mpsc::UnboundedSender<SshSessionEvent>,
) -> Result<(), String> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| format!("openpty: {e}"))?;

    let program = shell
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(default_shell);
    let mut cmd = CommandBuilder::new(program);
    if let Some(home) = home_dir() {
        cmd.cwd(home);
    }
    cmd.env("TERM", "xterm-256color");

    let mut child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| format!("spawn shell: {e}"))?;

    // Read + write handles are cloned/taken off the master before it moves
    // into the command task (where resize lives).
    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| format!("clone reader: {e}"))?;
    let mut writer = pair
        .master
        .take_writer()
        .map_err(|e| format!("take writer: {e}"))?;
    let master = pair.master;
    // Drop the slave so the child holds the only slave end — required for a
    // clean EOF on the reader when the shell exits.
    drop(pair.slave);

    // The shell is up; tell the UI it's ready (no connect phase locally).
    let _ = tx_events.send(SshSessionEvent::StateChanged {
        state: SessionState::Ready,
    });

    // Reader thread (blocking I/O): PTY output → Data; EOF/err → Closed.
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if tx_events
                        .send(SshSessionEvent::Data {
                            bytes: buf[..n].to_vec(),
                        })
                        .is_err()
                    {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let _ = tx_events.send(SshSessionEvent::Closed {
            reason: CloseReason::ServerDisconnected { message: None },
        });
        // `tx_events` drops here → the forwarder's stream ends → cleanup.
    });

    // Command task: owns writer + master + child. Writes are tiny
    // (keystrokes), so doing them inline in the async task is fine.
    tokio::spawn(async move {
        // Keep `master` alive for the lifetime of the task (resize target).
        let master = master;
        while let Some(cmd) = rx_cmd.recv().await {
            match cmd {
                SessionCommand::SshInput(bytes) => {
                    let _ = writer.write_all(&bytes);
                    let _ = writer.flush();
                }
                SessionCommand::Resize { cols, rows } => {
                    let _ = master.resize(PtySize {
                        rows,
                        cols,
                        pixel_width: 0,
                        pixel_height: 0,
                    });
                }
                SessionCommand::Shutdown => {
                    let _ = child.kill();
                    break;
                }
                // No host-key TOFU for a local shell.
                SessionCommand::HostKeyDecision(_) => {}
            }
        }
        let _ = child.kill();
    });

    Ok(())
}
