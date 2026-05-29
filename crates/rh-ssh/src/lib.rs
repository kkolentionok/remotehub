//! SSH session actor for RemoteHub (Stage 2).
//!
//! Wraps `russh` as a Tokio actor. `spawn_session` returns immediately
//! with a [`SshSessionHandle`]; the real handshake runs in a spawned
//! task and reports progress via an `mpsc` stream of [`SshSessionEvent`]
//! (the `rh-app` layer bridges that stream into a Tauri `Channel`, so
//! this crate stays free of any `tauri` dependency).
//!
//! v1 scope: password authentication; host keys are accepted TOFU-style
//! (interactive confirmation + `known_hosts` pinning come later — the UI
//! already has the prompt surface for that). SSH-key auth is reserved.

mod actor;
mod error;

use std::time::Duration;

use serde::Serialize;
use tokio::sync::mpsc;
use tokio::task::AbortHandle;

use rh_core::{HostId, Protocol, RevealedSecret, SessionId};

pub use error::SshError;

// =====================================================================
// Public session types (wire-compatible with the UI contract in
// docs/specs/tauri-api.md). Serialization shapes MUST stay in sync with
// ui/src/lib/types.ts.
// =====================================================================

/// Lifecycle state of a session. Serialized snake_case.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    Connecting,
    Authenticating,
    HostKeyPending,
    Ready,
    Disconnecting,
    Closed,
    Failed,
}

/// Why a session ended. Serialized as `{ "kind": ... }`.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CloseReason {
    UserRequested,
    ServerDisconnected { message: Option<String> },
    NetworkError { message: String },
    AuthFailed,
    HostKeyRejected,
    Crashed { message: String },
}

/// Events pushed from the actor toward the UI. `{ "kind": ... }` tagged.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SshSessionEvent {
    StateChanged { state: SessionState },
    Data { bytes: Vec<u8> },
    AuthFailed { method: String },
    HostKeyPrompt { fingerprint_sha256: String, key_type: String },
    Error { message: String },
    Closed { reason: CloseReason },
}

/// Commands the UI sends to a running actor.
#[derive(Debug)]
pub enum SessionCommand {
    /// Raw stdin bytes from xterm.js.
    SshInput(Vec<u8>),
    /// PTY resize, in character cells.
    Resize { cols: u16, rows: u16 },
    /// Graceful shutdown.
    Shutdown,
}

/// PTY / connection options.
#[derive(Debug, Clone)]
pub struct SshOpenOptions {
    pub cols: u16,
    pub rows: u16,
    pub term: String,
    pub keepalive_interval: Option<Duration>,
}

impl Default for SshOpenOptions {
    fn default() -> Self {
        Self {
            cols: 80,
            rows: 24,
            term: "xterm-256color".to_string(),
            keepalive_interval: Some(Duration::from_secs(30)),
        }
    }
}

/// Resolved credential material (secret already revealed from keychain).
/// Dropped — and thus zeroized — by the actor as soon as auth completes.
pub enum RevealedCredential {
    Password {
        username: String,
        password: RevealedSecret,
    },
    /// Reserved — not implemented in v1.
    Key {
        username: String,
        private_key_pem: RevealedSecret,
        passphrase: Option<RevealedSecret>,
    },
}

/// What the caller needs to spawn a session.
pub struct SshSpawnParams {
    pub id: SessionId,
    pub hostname: String,
    pub port: u16,
    pub host_id: HostId,
    pub credential: RevealedCredential,
    pub options: SshOpenOptions,
    /// Optional command run once after the shell is ready (sent as input).
    pub startup_command: Option<String>,
}

/// Remote control for a running session actor.
#[derive(Debug)]
pub struct SshSessionHandle {
    pub id: SessionId,
    pub host_id: HostId,
    pub protocol: Protocol,
    pub opened_at: chrono::DateTime<chrono::Utc>,
    pub tx_cmd: mpsc::Sender<SessionCommand>,
    pub abort: AbortHandle,
}

/// Spawn an SSH session actor. Returns immediately; progress arrives on
/// `events`. The returned `JoinHandle` lets a supervisor remove the
/// session from its registry when the actor exits.
pub fn spawn_session(
    params: SshSpawnParams,
    events: mpsc::UnboundedSender<SshSessionEvent>,
) -> (SshSessionHandle, tokio::task::JoinHandle<()>) {
    let (tx_cmd, rx_cmd) = mpsc::channel::<SessionCommand>(64);
    let id = params.id.clone();
    let host_id = params.host_id.clone();
    let opened_at = chrono::Utc::now();

    let join = tokio::spawn(actor::run(params, rx_cmd, events));
    let abort = join.abort_handle();

    let handle = SshSessionHandle {
        id,
        host_id,
        protocol: Protocol::Ssh,
        opened_at,
        tx_cmd,
        abort,
    };
    (handle, join)
}
