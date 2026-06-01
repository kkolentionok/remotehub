//! SSH session actor for RemoteHub (Stage 2).
//!
//! Wraps `russh` as a Tokio actor. `spawn_session` returns immediately
//! with a [`SshSessionHandle`]; the real handshake runs in a spawned
//! task and reports progress via an `mpsc` stream of [`SshSessionEvent`]
//! (the `rh-app` layer bridges that stream into a Tauri `Channel`, so
//! this crate stays free of any `tauri` dependency).
//!
//! v1 scope has since grown: password, SSH-key (OpenSSH/PEM + PuTTY
//! .ppk), and SSH-agent auth, all tried in order. Host keys are pinned
//! via [`rh_core::KnownHostsStore`] with an interactive TOFU prompt
//! surfaced through [`SshSessionEvent::HostKeyPrompt`] and answered with
//! [`SessionCommand::HostKeyDecision`].

mod actor;
mod error;
mod ppk;
pub mod sftp;

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
    HostKeyPrompt {
        fingerprint_sha256: String,
        key_type: String,
        /// `true` when a key was already pinned for this host but the
        /// server presented a *different* one — a security-relevant
        /// change, surfaced more loudly in the UI than a first-time key.
        changed: bool,
    },
    Error { message: String },
    /// Detected remote OS slug (e.g. "ubuntu", "debian", "macos",
    /// "windows"). Emitted once, best-effort, shortly after Ready. The
    /// `rh-app` layer persists it to `hosts.detected_os`; the UI never
    /// sees this event (the session manager consumes it).
    DetectedOs { os: String },
    Closed { reason: CloseReason },
}

/// Commands the UI sends to a running actor.
#[derive(Debug)]
pub enum SessionCommand {
    /// Raw stdin bytes from xterm.js.
    SshInput(Vec<u8>),
    /// PTY resize, in character cells.
    Resize { cols: u16, rows: u16 },
    /// Answer to a pending [`SshSessionEvent::HostKeyPrompt`]: `true`
    /// trusts (and pins) the key, `false` rejects and ends the session.
    HostKeyDecision(bool),
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
    /// When `true`, an unknown host key triggers an interactive TOFU
    /// prompt before trusting it. When `false`, an unknown key is
    /// auto-trusted and pinned (legacy behavior) — but a *changed* key
    /// still always prompts, regardless of this flag.
    pub strict_host_key: bool,
}

impl Default for SshOpenOptions {
    fn default() -> Self {
        Self {
            cols: 80,
            rows: 24,
            term: "xterm-256color".to_string(),
            keepalive_interval: Some(Duration::from_secs(30)),
            strict_host_key: true,
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
    /// SSH key auth. The PEM may be OpenSSH/PKCS#8 or PuTTY .ppk (the
    /// actor converts the latter on the fly).
    Key {
        username: String,
        private_key_pem: RevealedSecret,
        passphrase: Option<RevealedSecret>,
    },
    /// SSH-agent auth (Pageant / OpenSSH agent). No secret travels
    /// through the app — the OS-side agent signs the challenge. The
    /// actor lists the agent's identities and tries each.
    Agent { username: String },
}

/// Bastion (ProxyJump) connection params. The actor connects here first,
/// then opens a direct-tcpip channel to the real target and runs the
/// target SSH transport over it.
pub struct JumpParams {
    pub hostname: String,
    pub port: u16,
    pub host_id: HostId,
    /// Auth methods for the bastion (same multi-method semantics).
    pub credentials: Vec<RevealedCredential>,
}

/// What the caller needs to spawn a session.
pub struct SshSpawnParams {
    pub id: SessionId,
    pub hostname: String,
    pub port: u16,
    pub host_id: HostId,
    /// All auth methods to try, in order. The actor attempts each until
    /// one succeeds (typically key(s) first, then password). Auth fails
    /// only when every method is rejected.
    pub credentials: Vec<RevealedCredential>,
    pub options: SshOpenOptions,
    /// Optional command run once after the shell is ready (sent as input).
    pub startup_command: Option<String>,
    /// Environment variables requested on the channel before the shell
    /// starts (`SSH_MSG_CHANNEL_REQUEST` "env"). Servers honor these
    /// only for names allowed by their `AcceptEnv`; unaccepted ones are
    /// silently ignored by the peer.
    pub env_vars: Vec<(String, String)>,
    /// Host-key pinning store for TOFU. The actor looks up the expected
    /// key during the handshake and persists the user's trust decision.
    pub known_hosts: std::sync::Arc<dyn rh_core::KnownHostsStore>,
    /// Optional bastion to route through (ProxyJump). One level only.
    pub jump: Option<JumpParams>,
    /// Forward the local SSH agent to the target (`ssh -A`).
    pub agent_forwarding: bool,
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
