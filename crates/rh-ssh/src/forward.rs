//! SSH port forwarding — slice 2 (`-L` local, `-R` remote, `-D` dynamic SOCKS).
//!
//! A forward is a self-contained unit: it opens its *own* SSH connection to a
//! saved host (reusing that host's credentials + one-level ProxyJump) and runs
//! independently of any open terminal session. Three directions:
//!
//! * **Local (`-L`)** — bind a local `TcpListener`; each accepted connection
//!   opens a `direct-tcpip` channel to `target:port` reachable from the
//!   *remote* side and pumps bytes both ways.
//! * **Remote (`-R`)** — ask the server to listen (`tcpip_forward`); each
//!   `forwarded-tcpip` channel the server opens is bridged to `target:port`
//!   reachable from *our* side (`TcpStream::connect`).
//! * **Dynamic (`-D`)** — bind a local SOCKS5 proxy; each client connection
//!   does a SOCKS5 CONNECT handshake, and we open a `direct-tcpip` channel to
//!   the address the client requested (target is decided per-connection).
//!
//! The connect/auth path reuses the proven pieces from [`crate::actor`]
//! (`ClientHandler::trusting`, `try_all_auth`, `CONNECT_TIMEOUT`) and the
//! exact `channel_open_direct_tcpip` + `Channel::into_stream` shape that the
//! ProxyJump bastion already uses.
//!
//! Out of scope here (slice 3): persisting forward definitions, and an
//! interactive TOFU prompt (forwards still auto-accept like the bastion).

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, Notify};
use tokio::task::{AbortHandle, JoinHandle};
use tracing::{info, instrument, warn};

use crate::actor::{try_all_auth, ClientHandler, CONNECT_TIMEOUT};
use crate::error::SshError;
use crate::{JumpParams, RevealedCredential, SshSessionEvent};

type FwdChannel = russh::Channel<russh::client::Msg>;

/// Which direction a forward runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ForwardKind {
    /// `ssh -L`: listen locally, tunnel to `target` reachable from the remote.
    Local,
    /// `ssh -R`: server listens, tunnel back to `target` reachable from us.
    Remote,
    /// `ssh -D`: local SOCKS5 proxy; per-connection target chosen by the client.
    Dynamic,
}

/// A forward's binding + target description (serialized to the UI).
#[derive(Debug, Clone, Serialize)]
pub struct ForwardSpec {
    pub kind: ForwardKind,
    /// Local bind address for `-L`/`-D` (typically `127.0.0.1`); for `-R`
    /// this is the address the *server* binds (`127.0.0.1`, `0.0.0.0`, …).
    pub bind_host: String,
    pub bind_port: u16,
    /// For `-L`: host reachable from the remote side. For `-R`: host
    /// reachable from our side. For `-D`: unused (per-connection).
    pub target_host: String,
    pub target_port: u16,
}

/// Lifecycle of a forward. Serialized snake_case.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ForwardState {
    Connecting,
    Listening,
    Error,
    Closed,
}

/// Events pushed from the forward actor toward the UI. `{ "kind": ... }`.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ForwardEvent {
    StateChanged { state: ForwardState },
    /// Number of currently-open tunneled connections changed.
    ActiveChanged { active: u32 },
    Error { message: String },
    Closed { message: Option<String> },
}

/// Connection params (mirror of the SSH session connect inputs). Secrets
/// are already revealed; dropped — and zeroized — once auth completes.
pub struct ForwardConnectParams {
    pub hostname: String,
    pub port: u16,
    pub credentials: Vec<RevealedCredential>,
    pub known_hosts: Arc<dyn rh_core::KnownHostsStore>,
    pub jump: Option<JumpParams>,
    pub keepalive_interval: Option<Duration>,
}

/// Everything needed to spawn a forward.
pub struct ForwardSpawnParams {
    pub connect: ForwardConnectParams,
    pub spec: ForwardSpec,
}

/// Remote control for a running forward actor.
pub struct ForwardHandle {
    pub abort: AbortHandle,
    shutdown: Arc<AtomicBool>,
    cancel: Arc<Notify>,
}

impl ForwardHandle {
    /// Request a graceful stop: the loop breaks, the listener (if any) is
    /// dropped, and dropping the SSH handle closes the live channels.
    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
        self.cancel.notify_waiters();
    }
}

/// Spawn a forward actor. Returns immediately; progress arrives on `events`.
pub fn spawn_forward(
    params: ForwardSpawnParams,
    events: mpsc::UnboundedSender<ForwardEvent>,
) -> (ForwardHandle, JoinHandle<()>) {
    let shutdown = Arc::new(AtomicBool::new(false));
    let cancel = Arc::new(Notify::new());
    let join = tokio::spawn(run(params, events, shutdown.clone(), cancel.clone()));
    let abort = join.abort_handle();
    (
        ForwardHandle {
            abort,
            shutdown,
            cancel,
        },
        join,
    )
}

#[instrument(level = "debug", skip(params, events, shutdown, cancel))]
async fn run(
    params: ForwardSpawnParams,
    events: mpsc::UnboundedSender<ForwardEvent>,
    shutdown: Arc<AtomicBool>,
    cancel: Arc<Notify>,
) {
    let emit = |e: ForwardEvent| {
        let _ = events.send(e);
    };
    emit(ForwardEvent::StateChanged {
        state: ForwardState::Connecting,
    });

    let spec = params.spec;

    // Remote (`-R`) needs a sink for server-opened forwarded-tcpip channels,
    // installed on the handler *before* connecting.
    let (fwd_tx, fwd_rx) = match spec.kind {
        ForwardKind::Remote => {
            let (tx, rx) = mpsc::unbounded_channel::<FwdChannel>();
            (Some(tx), Some(rx))
        }
        _ => (None, None),
    };

    let (mut handle, _bastion_keepalive) = match connect(params.connect, fwd_tx).await {
        Ok(pair) => pair,
        Err(e) => {
            emit(ForwardEvent::Error {
                message: e.to_string(),
            });
            emit(ForwardEvent::Closed {
                message: Some(e.to_string()),
            });
            return;
        }
    };

    let active = Arc::new(AtomicU32::new(0));

    match spec.kind {
        ForwardKind::Remote => {
            run_remote(
                &mut handle,
                &spec,
                fwd_rx.expect("remote sets fwd_rx"),
                &events,
                &active,
                &shutdown,
                &cancel,
            )
            .await;
        }
        ForwardKind::Local | ForwardKind::Dynamic => {
            run_listener(&mut handle, &spec, &events, &active, &shutdown, &cancel).await;
        }
    }

    emit(ForwardEvent::Closed { message: None });
    // `handle` (and the bastion) drop here → all channels close.
}

/// Local (`-L`) and dynamic (`-D`): bind a local listener and serve.
async fn run_listener(
    handle: &mut russh::client::Handle<ClientHandler>,
    spec: &ForwardSpec,
    events: &mpsc::UnboundedSender<ForwardEvent>,
    active: &Arc<AtomicU32>,
    shutdown: &Arc<AtomicBool>,
    cancel: &Arc<Notify>,
) {
    let emit = |e: ForwardEvent| {
        let _ = events.send(e);
    };
    let bind = format!("{}:{}", spec.bind_host, spec.bind_port);
    let listener = match TcpListener::bind(&bind).await {
        Ok(l) => l,
        Err(e) => {
            let msg = format!("bind {bind} failed: {e}");
            warn!("{msg}");
            emit(ForwardEvent::Error { message: msg });
            return;
        }
    };
    info!("forward {:?} listening on {bind}", spec.kind);
    emit(ForwardEvent::StateChanged {
        state: ForwardState::Listening,
    });

    loop {
        if shutdown.load(Ordering::SeqCst) {
            break;
        }
        tokio::select! {
            _ = cancel.notified() => break,
            res = listener.accept() => {
                let mut tcp = match res {
                    Ok((stream, _peer)) => stream,
                    Err(e) => { warn!("forward accept error: {e}"); continue; }
                };

                // Resolve the per-connection target.
                let (host, port) = match spec.kind {
                    ForwardKind::Local => (spec.target_host.clone(), spec.target_port),
                    ForwardKind::Dynamic => match socks5_accept(&mut tcp).await {
                        Ok(dst) => dst,
                        Err(e) => { warn!("socks5 handshake failed: {e}"); continue; }
                    },
                    ForwardKind::Remote => unreachable!("remote uses run_remote"),
                };

                let ch = match handle
                    .channel_open_direct_tcpip(host.as_str(), u32::from(port), "127.0.0.1", 0)
                    .await
                {
                    Ok(c) => {
                        if spec.kind == ForwardKind::Dynamic {
                            // SOCKS5 success reply (BND.ADDR/PORT = zeros).
                            let _ = tcp
                                .write_all(&[0x05, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
                                .await;
                        }
                        c
                    }
                    Err(e) => {
                        warn!("forward channel open to {host}:{port} failed: {e}");
                        if spec.kind == ForwardKind::Dynamic {
                            // SOCKS5 reply: connection refused (0x05).
                            let _ = tcp
                                .write_all(&[0x05, 0x05, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
                                .await;
                        } else {
                            emit(ForwardEvent::Error {
                                message: format!("tunnel to {host}:{port} failed: {e}"),
                            });
                        }
                        continue;
                    }
                };
                spawn_pump(tcp, ch, active.clone(), events.clone());
            }
        }
    }
}

/// Remote (`-R`): the server listens; bridge each forwarded channel to a
/// locally-reachable target.
async fn run_remote(
    handle: &mut russh::client::Handle<ClientHandler>,
    spec: &ForwardSpec,
    mut fwd_rx: mpsc::UnboundedReceiver<FwdChannel>,
    events: &mpsc::UnboundedSender<ForwardEvent>,
    active: &Arc<AtomicU32>,
    shutdown: &Arc<AtomicBool>,
    cancel: &Arc<Notify>,
) {
    let emit = |e: ForwardEvent| {
        let _ = events.send(e);
    };
    // Ask the server to open a listening socket.
    match handle
        .tcpip_forward(spec.bind_host.clone(), u32::from(spec.bind_port))
        .await
    {
        Ok(port) => info!(
            "forward -R: server listening on {}:{} -> {}:{}",
            spec.bind_host, port, spec.target_host, spec.target_port
        ),
        Err(e) => {
            let msg = format!("remote forward request denied: {e}");
            warn!("{msg}");
            emit(ForwardEvent::Error { message: msg });
            return;
        }
    }
    emit(ForwardEvent::StateChanged {
        state: ForwardState::Listening,
    });

    let target = (spec.target_host.clone(), spec.target_port);
    loop {
        if shutdown.load(Ordering::SeqCst) {
            break;
        }
        tokio::select! {
            _ = cancel.notified() => break,
            maybe = fwd_rx.recv() => {
                let ch = match maybe {
                    Some(c) => c,
                    None => break, // handler dropped (connection gone)
                };
                // Connect to the local-side target and pump.
                let addr = format!("{}:{}", target.0, target.1);
                let tcp = match TcpStream::connect(&addr).await {
                    Ok(s) => s,
                    Err(e) => {
                        warn!("remote forward: local connect to {addr} failed: {e}");
                        // Dropping `ch` closes the server-side connection.
                        continue;
                    }
                };
                spawn_pump(tcp, ch, active.clone(), events.clone());
            }
        }
    }
    // Best-effort: ask the server to stop listening.
    let _ = handle
        .cancel_tcpip_forward(spec.bind_host.clone(), u32::from(spec.bind_port))
        .await;
}

/// Bridge a TCP stream and an SSH channel, tracking the active-connection
/// count and emitting `ActiveChanged` on both ends.
fn spawn_pump(
    tcp: TcpStream,
    ch: FwdChannel,
    active: Arc<AtomicU32>,
    events: mpsc::UnboundedSender<ForwardEvent>,
) {
    let n = active.fetch_add(1, Ordering::SeqCst) + 1;
    let _ = events.send(ForwardEvent::ActiveChanged { active: n });
    tokio::spawn(async move {
        let mut tcp = tcp;
        let mut stream = ch.into_stream();
        if let Err(e) = tokio::io::copy_bidirectional(&mut tcp, &mut stream).await {
            warn!("forward pump ended: {e}");
        }
        let left = active.fetch_sub(1, Ordering::SeqCst).saturating_sub(1);
        let _ = events.send(ForwardEvent::ActiveChanged { active: left });
    });
}

/// Minimal SOCKS5 server handshake: no-auth greeting + a CONNECT request.
/// Returns the requested `(host, port)`. The success/failure reply is sent
/// by the caller once it knows whether the channel opened.
async fn socks5_accept(tcp: &mut TcpStream) -> std::io::Result<(String, u16)> {
    fn bad(m: &str) -> std::io::Error {
        std::io::Error::new(std::io::ErrorKind::InvalidData, m.to_string())
    }

    // ---- greeting: VER, NMETHODS, METHODS[NMETHODS] ----
    let mut head = [0u8; 2];
    tcp.read_exact(&mut head).await?;
    if head[0] != 0x05 {
        return Err(bad("not SOCKS5"));
    }
    let n = head[1] as usize;
    let mut methods = vec![0u8; n];
    tcp.read_exact(&mut methods).await?;
    // Select "no authentication required".
    tcp.write_all(&[0x05, 0x00]).await?;

    // ---- request: VER, CMD, RSV, ATYP, ADDR, PORT ----
    let mut req = [0u8; 4];
    tcp.read_exact(&mut req).await?;
    if req[0] != 0x05 {
        return Err(bad("bad SOCKS5 request version"));
    }
    if req[1] != 0x01 {
        // Only CONNECT is supported → reply "command not supported".
        let _ = tcp
            .write_all(&[0x05, 0x07, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
            .await;
        return Err(bad("only CONNECT is supported"));
    }
    let host = match req[3] {
        0x01 => {
            let mut a = [0u8; 4];
            tcp.read_exact(&mut a).await?;
            std::net::Ipv4Addr::from(a).to_string()
        }
        0x04 => {
            let mut a = [0u8; 16];
            tcp.read_exact(&mut a).await?;
            std::net::Ipv6Addr::from(a).to_string()
        }
        0x03 => {
            let mut len = [0u8; 1];
            tcp.read_exact(&mut len).await?;
            let mut d = vec![0u8; len[0] as usize];
            tcp.read_exact(&mut d).await?;
            String::from_utf8_lossy(&d).into_owned()
        }
        _ => {
            let _ = tcp
                .write_all(&[0x05, 0x08, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
                .await;
            return Err(bad("unsupported SOCKS5 address type"));
        }
    };
    let mut p = [0u8; 2];
    tcp.read_exact(&mut p).await?;
    Ok((host, u16::from_be_bytes(p)))
}

/// Connect + authenticate, reusing the actor's trusting handler and auth
/// helpers. `forwarded_tx` (remote `-R`) installs a sink for server-opened
/// forwarded-tcpip channels. Returns `(target_handle, optional_bastion)`.
async fn connect(
    p: ForwardConnectParams,
    forwarded_tx: Option<mpsc::UnboundedSender<FwdChannel>>,
) -> Result<
    (
        russh::client::Handle<ClientHandler>,
        Option<russh::client::Handle<ClientHandler>>,
    ),
    SshError,
> {
    let mut config = russh::client::Config::default();
    config.inactivity_timeout = None;
    config.keepalive_interval = p.keepalive_interval;
    config.keepalive_max = 3;
    let config = Arc::new(config);

    // Dummy SSH-event sink: the trusting handler never emits (auto_accept
    // short-circuits), but the field is required.
    let (ev_tx, _ev_rx) = mpsc::unbounded_channel::<SshSessionEvent>();

    let mut target_handler =
        ClientHandler::trusting(p.hostname.clone(), p.port, p.known_hosts.clone(), ev_tx.clone());
    if let Some(tx) = forwarded_tx {
        target_handler = target_handler.with_forwarded_sink(tx);
    }

    if let Some(jump) = p.jump {
        let bastion_handler = ClientHandler::trusting(
            jump.hostname.clone(),
            jump.port,
            p.known_hosts.clone(),
            ev_tx.clone(),
        );
        let mut bastion = match tokio::time::timeout(
            CONNECT_TIMEOUT,
            russh::client::connect(
                config.clone(),
                (jump.hostname.as_str(), jump.port),
                bastion_handler,
            ),
        )
        .await
        {
            Ok(r) => r?,
            Err(_) => {
                return Err(SshError::Network(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "jump host connection timed out",
                )))
            }
        };
        if !try_all_auth(&mut bastion, jump.credentials).await? {
            return Err(SshError::AuthFailed {
                method: "jump".into(),
            });
        }
        let ch = bastion
            .channel_open_direct_tcpip(p.hostname.as_str(), u32::from(p.port), "127.0.0.1", 0)
            .await?;
        let stream = ch.into_stream();
        let mut handle = match tokio::time::timeout(
            CONNECT_TIMEOUT,
            russh::client::connect_stream(config.clone(), stream, target_handler),
        )
        .await
        {
            Ok(r) => r?,
            Err(_) => {
                return Err(SshError::Network(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "target connection timed out",
                )))
            }
        };
        if !try_all_auth(&mut handle, p.credentials).await? {
            return Err(SshError::AuthFailed {
                method: "publickey".into(),
            });
        }
        Ok((handle, Some(bastion)))
    } else {
        let mut handle = match tokio::time::timeout(
            CONNECT_TIMEOUT,
            russh::client::connect(config.clone(), (p.hostname.as_str(), p.port), target_handler),
        )
        .await
        {
            Ok(r) => r?,
            Err(_) => {
                return Err(SshError::Network(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "connection timed out",
                )))
            }
        };
        if !try_all_auth(&mut handle, p.credentials).await? {
            return Err(SshError::AuthFailed {
                method: "publickey".into(),
            });
        }
        Ok((handle, None))
    }
}
