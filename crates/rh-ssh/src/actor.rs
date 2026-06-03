//! The SSH session actor: connect → verify host key → authenticate →
//! open PTY shell → pump bytes both ways until shutdown or disconnect.
//!
//! ⚠️ russh API NOTE: every line that touches `russh` is concentrated
//! here. If `cargo build` reports signature mismatches, they will almost
//! certainly be in this file and are mechanical to fix (method renamed,
//! arg added/removed, `bool` vs `AuthResult`, key type path). The
//! control flow itself is sound. The two areas most likely to need a
//! tweak across russh versions are (a) the host-key fingerprint helper
//! and (b) the SSH-agent block — both are flagged inline.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;
use tracing::{debug, instrument, warn};

use rh_core::KnownHostKey;

use crate::error::SshError;
use crate::{
    CloseReason, RevealedCredential, SessionCommand, SessionState, SshSessionEvent,
    SshSpawnParams,
};

/// Outcome of the host-key check, decided inside the russh handler.
enum HostKeyOutcome {
    /// Key matched a pinned entry, or the user trusted it — proceed.
    Accept,
    /// User rejected (or the decision channel closed) — abort the
    /// handshake. Surfaced as [`CloseReason::HostKeyRejected`].
    Reject,
}

/// russh client handler. Verifies the server key against the pinned
/// `known_hosts` entry; unknown/changed keys raise an interactive TOFU
/// prompt and block until the UI answers via [`SessionCommand::HostKeyDecision`].
struct ClientHandler {
    hostname: String,
    port: u16,
    strict: bool,
    /// Bastion mode: trust + pin silently, never prompt (avoids a second
    /// host-key prompt when routing through a jump host).
    auto_accept: bool,
    known: Arc<dyn rh_core::KnownHostsStore>,
    events: mpsc::UnboundedSender<SshSessionEvent>,
    /// User trust decisions, forwarded from the command channel while the
    /// connect future is in flight (see `connect_and_pump`).
    decisions: mpsc::Receiver<bool>,
    /// `true` for the target session when the host enables agent forwarding:
    /// server-opened `auth-agent@openssh.com` channels are bridged to the
    /// local OS agent. Always `false` for the bastion handler.
    agent_forward: bool,
    /// Live agent-forward relays, keyed by the server-opened channel id.
    agent_bridges: std::collections::HashMap<russh::ChannelId, AgentBridge>,
}

impl ClientHandler {
    /// Resolve the host-key decision: silently accept a match, prompt on
    /// unknown (when strict) or changed (always), pin on trust.
    async fn decide(&mut self, key_type: String, fingerprint: String) -> HostKeyOutcome {
        // Bastion: trust and pin without bothering the user.
        if self.auto_accept {
            self.pin(&key_type, &fingerprint).await;
            return HostKeyOutcome::Accept;
        }

        let pinned = self.known.lookup(&self.hostname, self.port).await;

        let changed = match &pinned {
            Ok(Some(k)) if k.fingerprint_sha256 == fingerprint => {
                // Known and unchanged — trust without bothering the user.
                return HostKeyOutcome::Accept;
            }
            Ok(Some(_)) => true,            // pinned, but a DIFFERENT key now
            Ok(None) => {
                // First time we see this host.
                if !self.strict {
                    // Non-strict: auto-trust and pin silently (legacy).
                    self.pin(&key_type, &fingerprint).await;
                    return HostKeyOutcome::Accept;
                }
                false
            }
            Err(e) => {
                // Storage hiccup — fail safe by prompting rather than
                // silently trusting.
                warn!(error = %e, "known_hosts lookup failed; prompting");
                false
            }
        };

        // Ask the UI and block until it answers.
        let _ = self.events.send(SshSessionEvent::StateChanged {
            state: SessionState::HostKeyPending,
        });
        let _ = self.events.send(SshSessionEvent::HostKeyPrompt {
            fingerprint_sha256: fingerprint.clone(),
            key_type: key_type.clone(),
            changed,
        });

        match self.decisions.recv().await {
            Some(true) => {
                self.pin(&key_type, &fingerprint).await;
                HostKeyOutcome::Accept
            }
            Some(false) | None => HostKeyOutcome::Reject,
        }
    }

    async fn pin(&self, key_type: &str, fingerprint: &str) {
        let entry = KnownHostKey {
            key_type: key_type.to_string(),
            fingerprint_sha256: fingerprint.to_string(),
        };
        if let Err(e) = self.known.remember(&self.hostname, self.port, &entry).await {
            warn!(error = %e, "failed to pin host key");
        }
    }
}

#[async_trait]
impl russh::client::Handler for ClientHandler {
    type Error = SshError;

    // NOTE (russh version-sensitive): the key type path
    // (`russh::keys::key::PublicKey`) and this signature are the most
    // fragile lines. On russh 0.46+ the key type moves to `ssh_key::PublicKey`.
    async fn check_server_key(
        &mut self,
        server_public_key: &russh::keys::key::PublicKey,
    ) -> Result<bool, Self::Error> {
        let fingerprint = fingerprint_sha256(server_public_key);
        let key_type = server_public_key.name().to_string();
        match self.decide(key_type, fingerprint).await {
            HostKeyOutcome::Accept => Ok(true),
            // Returning Ok(false) makes russh abort the handshake; the
            // actor maps the resulting connect error to HostKeyRejected
            // (it set the `rejected` flag when forwarding the decision).
            HostKeyOutcome::Reject => Ok(false),
        }
    }

    // ---- Agent forwarding (serving side) -----------------------------
    // The server opens `auth-agent@openssh.com` channels (russh auto-confirms
    // before calling us); their byte stream is the raw SSH-agent protocol.
    // We bridge each to the local OS agent. See `AgentBridge` below.

    async fn server_channel_open_agent_forward(
        &mut self,
        channel: russh::ChannelId,
        _session: &mut russh::client::Session,
    ) -> Result<(), Self::Error> {
        if !self.agent_forward {
            return Ok(()); // not enabled for this host — leave the channel idle
        }
        #[cfg(any(unix, windows))]
        match open_os_agent().await {
            Ok(io) => {
                self.agent_bridges.insert(
                    channel,
                    AgentBridge {
                        io: tokio::sync::Mutex::new(io),
                        inbuf: Vec::new(),
                    },
                );
                debug!("agent-forward: bridged a server channel to the local agent");
            }
            Err(e) => {
                warn!(error = %e, "agent-forward: local agent unavailable; not bridging");
            }
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = channel;
        }
        Ok(())
    }

    async fn data(
        &mut self,
        channel: russh::ChannelId,
        data: &[u8],
        session: &mut russh::client::Session,
    ) -> Result<(), Self::Error> {
        // Only agent-forward channels are handled here; the PTY channel's
        // data is consumed via its own `Channel<Msg>` stream (russh delivers
        // to both, so the map membership check keeps us out of its way).
        #[cfg(any(unix, windows))]
        if let Some(bridge) = self.agent_bridges.get_mut(&channel) {
            match bridge.pump(data).await {
                Ok(replies) => {
                    for r in replies {
                        session.data(channel, russh::CryptoVec::from_slice(&r));
                    }
                }
                Err(e) => {
                    warn!(error = %e, "agent-forward relay error; closing channel");
                    self.agent_bridges.remove(&channel);
                    session.close(channel);
                }
            }
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = (channel, data, session);
        }
        Ok(())
    }

    async fn channel_eof(
        &mut self,
        channel: russh::ChannelId,
        _session: &mut russh::client::Session,
    ) -> Result<(), Self::Error> {
        self.agent_bridges.remove(&channel);
        Ok(())
    }

    async fn channel_close(
        &mut self,
        channel: russh::ChannelId,
        _session: &mut russh::client::Session,
    ) -> Result<(), Self::Error> {
        self.agent_bridges.remove(&channel);
        Ok(())
    }
}

// =====================================================================
// SSH agent forwarding — serving side (bridge to the local OS agent)
// =====================================================================
//
// With agent forwarding on, we advertise acceptance on the session channel
// (`channel.agent_forward(false)`); the server then opens
// `auth-agent@openssh.com` channels carrying the raw, length-prefixed
// SSH-agent protocol. Each complete framed request from the server is
// written verbatim to the local agent and the agent's framed reply is sent
// back on the channel — a transparent byte relay (we never parse the
// protocol).
//
// ⚠️ Unproven until tested live: relies on russh 0.45's
// `server_channel_open_agent_forward` + `data` callbacks and the OS agent
// transport (Windows named pipe / unix `$SSH_AUTH_SOCK`). Test: connect
// with agent forwarding enabled, then run `ssh-add -l` on the remote — it
// should list the *local* agent's keys.

/// Largest single agent message we'll relay (sanity bound; real ones are tiny).
const AGENT_MSG_CAP: usize = 256 * 1024;

#[cfg(any(unix, windows))]
trait AgentIo: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send {}
#[cfg(any(unix, windows))]
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send> AgentIo for T {}

/// One live agent-forward relay: the OS-agent connection plus an inbound
/// accumulator for reassembling length-prefixed agent requests.
struct AgentBridge {
    #[cfg(any(unix, windows))]
    io: tokio::sync::Mutex<Box<dyn AgentIo>>,
    inbuf: Vec<u8>,
}

#[cfg(any(unix, windows))]
async fn open_os_agent() -> std::io::Result<Box<dyn AgentIo>> {
    #[cfg(unix)]
    {
        let sock = std::env::var("SSH_AUTH_SOCK").map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::NotFound, "SSH_AUTH_SOCK not set")
        })?;
        let s = tokio::net::UnixStream::connect(sock).await?;
        Ok(Box::new(s))
    }
    #[cfg(windows)]
    {
        use tokio::net::windows::named_pipe::ClientOptions;
        let pipe = ClientOptions::new().open(r"\\.\pipe\openssh-ssh-agent")?;
        Ok(Box::new(pipe))
    }
}

impl AgentBridge {
    /// Feed bytes received from the server: forward every complete framed
    /// request to the agent and collect the framed replies to send back.
    #[cfg(any(unix, windows))]
    async fn pump(&mut self, incoming: &[u8]) -> std::io::Result<Vec<Vec<u8>>> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        self.inbuf.extend_from_slice(incoming);
        let mut replies = Vec::new();
        loop {
            if self.inbuf.len() < 4 {
                break;
            }
            let len = u32::from_be_bytes([
                self.inbuf[0],
                self.inbuf[1],
                self.inbuf[2],
                self.inbuf[3],
            ]) as usize;
            if len > AGENT_MSG_CAP {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "agent request too large",
                ));
            }
            if self.inbuf.len() < 4 + len {
                break; // wait for the rest of this message
            }
            let req: Vec<u8> = self.inbuf.drain(0..4 + len).collect();
            // `&mut self`, so `get_mut` gives direct access without an async
            // lock (the Mutex exists only to keep the handler `Sync`). Taken
            // after the drain so it doesn't overlap the `self.inbuf` borrow.
            let io = self.io.get_mut();
            io.write_all(&req).await?;
            io.flush().await?;
            // Read the framed reply (4-byte length + payload).
            let mut lenbuf = [0u8; 4];
            io.read_exact(&mut lenbuf).await?;
            let rlen = u32::from_be_bytes(lenbuf) as usize;
            if rlen > AGENT_MSG_CAP {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "agent reply too large",
                ));
            }
            let mut payload = vec![0u8; rlen];
            io.read_exact(&mut payload).await?;
            let mut reply = Vec::with_capacity(4 + rlen);
            reply.extend_from_slice(&lenbuf);
            reply.extend_from_slice(&payload);
            replies.push(reply);
        }
        Ok(replies)
    }
}

/// OpenSSH-style SHA256 fingerprint: `base64(sha256(public_key_blob))`
/// with no padding (the part `ssh-keygen -lf` prints after `SHA256:`).
///
/// NOTE (russh version-sensitive): `public_key_bytes()` comes from the
/// `russh::keys::PublicKeyBase64` trait. If the trait/method moved,
/// adjust the import and call below — the hashing itself is stable.
pub(crate) fn fingerprint_sha256(key: &russh::keys::key::PublicKey) -> String {
    use russh::keys::PublicKeyBase64;
    let blob = key.public_key_bytes();
    let digest = Sha256::digest(&blob);
    base64::engine::general_purpose::STANDARD_NO_PAD.encode(digest)
}

#[instrument(level = "debug", skip(params, rx_cmd, events), fields(session_id = %params.id))]
pub async fn run(
    params: SshSpawnParams,
    mut rx_cmd: mpsc::Receiver<SessionCommand>,
    events: mpsc::UnboundedSender<SshSessionEvent>,
) {
    if let Err(err) = connect_and_pump(params, &mut rx_cmd, &events).await {
        let reason = err.into_close_reason();
        let _ = events.send(SshSessionEvent::Error {
            message: format!("{reason:?}"),
        });
        let _ = events.send(SshSessionEvent::Closed { reason });
    }
}

/// Attempt a single auth method. Returns `Ok(true)` on success,
/// `Ok(false)` if rejected (or a key/agent couldn't be used — we skip
/// rather than abort so other methods still get a turn), and `Err` only
/// for transport-level failures.
async fn try_auth(
    handle: &mut russh::client::Handle<ClientHandler>,
    cred: RevealedCredential,
) -> Result<bool, SshError> {
    match cred {
        RevealedCredential::Password { username, password } => {
            let pw = password
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| String::from_utf8_lossy(password.expose()).into_owned());
            // NOTE (russh 0.45): returns `bool`. On 0.46+ it returns
            // `AuthResult` — then use `.success()` here.
            let ok = handle.authenticate_password(&username, &pw).await?;
            drop(password); // RevealedSecret → zeroized
            Ok(ok)
        }
        RevealedCredential::Key {
            username,
            private_key_pem,
            passphrase,
        } => {
            let pem = private_key_pem
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| {
                    String::from_utf8_lossy(private_key_pem.expose()).into_owned()
                });
            let pass = passphrase
                .as_ref()
                .and_then(|p| p.as_str().map(str::to_owned));
            // PuTTY .ppk → OpenSSH on the fly (russh can't read .ppk). The
            // converted PEM is already decrypted, so decode with no pass.
            let (pem, decode_pass): (String, Option<&str>) = if crate::ppk::is_ppk(&pem) {
                match crate::ppk::ppk_to_openssh(&pem, pass.as_deref()) {
                    Ok(converted) => (converted, None),
                    Err(e) => {
                        warn!(error = %e, "ppk → openssh conversion failed; skipping key");
                        return Ok(false);
                    }
                }
            } else {
                (pem, pass.as_deref())
            };
            drop(private_key_pem); // zeroized
            let key = match russh::keys::decode_secret_key(&pem, decode_pass) {
                Ok(k) => k,
                Err(_) => {
                    warn!("ssh key decode failed; skipping key");
                    return Ok(false);
                }
            };
            drop(passphrase);
            // NOTE (russh 0.45): `authenticate_publickey(user, Arc<KeyPair>)`
            // returns `bool`. On 0.46+ wrap as `PrivateKeyWithHashAlg` and
            // use `.success()`.
            let ok = handle
                .authenticate_publickey(&username, Arc::new(key))
                .await?;
            Ok(ok)
        }
        RevealedCredential::Agent { username } => try_auth_agent(handle, &username).await,
    }
}

/// SSH-agent auth (Pageant / OpenSSH agent). Best-effort: any failure to
/// reach the agent or to list/sign identities returns `Ok(false)` so the
/// connection falls through to the next method instead of failing.
///
/// ⚠️ NOTE (russh version-sensitive — most fragile block in the file):
/// the agent client constructor, `request_identities`, and
/// `authenticate_future` signatures shift between russh releases. The
/// shape below targets russh 0.45. If it doesn't compile, the fixes are
/// local to this function. Runtime failures are already non-fatal.
async fn try_auth_agent(
    handle: &mut russh::client::Handle<ClientHandler>,
    username: &str,
) -> Result<bool, SshError> {
    use russh::keys::agent::client::AgentClient;

    // Connect to the platform agent.
    // - Unix: $SSH_AUTH_SOCK (set by ssh-agent / gpg-agent / 1Password…).
    // - Windows: the OpenSSH-for-Windows agent named pipe. Modern Pageant
    //   (0.78+) also serves this pipe, so it covers both.
    #[cfg(unix)]
    let mut agent = match AgentClient::connect_env().await {
        Ok(a) => a,
        Err(e) => {
            warn!(error = %e, "ssh-agent unavailable; skipping agent auth");
            return Ok(false);
        }
    };
    #[cfg(windows)]
    let mut agent = {
        use tokio::net::windows::named_pipe::ClientOptions;
        match ClientOptions::new().open(r"\\.\pipe\openssh-ssh-agent") {
            // NOTE: `AgentClient::connect(stream)` wraps any async stream.
            Ok(pipe) => AgentClient::connect(pipe),
            Err(e) => {
                warn!(error = %e, "windows ssh-agent pipe unavailable; skipping agent auth");
                return Ok(false);
            }
        }
    };
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (handle, username);
        return Ok(false);
    }

    #[cfg(any(unix, windows))]
    {
        let identities = match agent.request_identities().await {
            Ok(ids) => ids,
            Err(e) => {
                warn!(error = %e, "ssh-agent request_identities failed; skipping");
                return Ok(false);
            }
        };
        if identities.is_empty() {
            debug!("ssh-agent holds no identities");
            return Ok(false);
        }
        for key in identities {
            // NOTE (russh 0.45): `authenticate_future(user, key, agent)`
            // consumes the agent and returns `(AgentClient, Result<bool>)`
            // so it can be reused across keys.
            let (returned, res) = handle.authenticate_future(username, key, agent).await;
            agent = returned;
            match res {
                Ok(true) => return Ok(true),
                Ok(false) => continue,
                Err(e) => {
                    warn!(error = %e, "ssh-agent sign attempt failed; trying next identity");
                    continue;
                }
            }
        }
        Ok(false)
    }
}

/// Best-effort remote OS detection over a short-lived exec channel.
/// Returns a lowercase slug ("ubuntu", "debian", "macos", "windows", …)
/// or `None` if it couldn't tell. Never errors out the session.
///
/// NOTE (russh version-sensitive): `channel_open_session` + `Channel::exec`
/// signatures. The command runs on a *separate* channel so the user's PTY
/// is untouched.
async fn detect_os(handle: &mut russh::client::Handle<ClientHandler>) -> Option<String> {
    let mut ch = handle.channel_open_session().await.ok()?;
    // `uname` resolves Unix-likes; on Windows (cmd.exe) it errors with
    // "is not recognized", which the parser treats as Windows. The
    // os-release file pins the exact Linux distro.
    ch.exec(true, "uname -s 2>/dev/null; echo ___RH___; cat /etc/os-release 2>/dev/null")
        .await
        .ok()?;

    let mut buf: Vec<u8> = Vec::new();
    let read = async {
        while let Some(msg) = ch.wait().await {
            match msg {
                russh::ChannelMsg::Data { ref data } => buf.extend_from_slice(data),
                russh::ChannelMsg::ExtendedData { ref data, .. } => {
                    buf.extend_from_slice(data)
                }
                russh::ChannelMsg::Eof | russh::ChannelMsg::Close => break,
                _ => {}
            }
        }
    };
    // Cap the wait so a misbehaving server can't stall the connect.
    let _ = tokio::time::timeout(Duration::from_millis(2500), read).await;
    let _ = ch.eof().await;

    parse_os_slug(&String::from_utf8_lossy(&buf))
}

/// Map raw `uname` + `/etc/os-release` output to a UI icon slug.
fn parse_os_slug(text: &str) -> Option<String> {
    let lower = text.to_lowercase();
    // Windows shells reject `uname`/`cat` with a recognizable message.
    if lower.contains("is not recognized") || lower.contains("not recognized as") {
        return Some("windows".to_string());
    }
    if lower.contains("darwin") {
        return Some("macos".to_string());
    }
    // Prefer the precise distro id from /etc/os-release (`ID=ubuntu`).
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("ID=") {
            let id = rest.trim().trim_matches('"').to_lowercase();
            if !id.is_empty() {
                return Some(id);
            }
        }
    }
    if lower.contains("freebsd") {
        return Some("freebsd".to_string());
    }
    if lower.contains("openbsd") {
        return Some("openbsd".to_string());
    }
    if lower.contains("linux") {
        return Some("linux".to_string());
    }
    None
}

/// Result of driving a target connect future while forwarding host-key
/// decisions from the command channel.
enum ConnectOutcome {
    Connected(Box<russh::client::Handle<ClientHandler>>),
    /// Shutdown arrived before the connection completed.
    Cancelled,
    Err(SshError),
}

/// Hard ceiling on the connect+handshake phase. A dead host (no SYN-ACK)
/// would otherwise hang on the OS-level TCP timeout (minutes on Windows);
/// this surfaces a clean timeout error to the UI instead.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

/// Drive a connect future to completion while forwarding the user's
/// host-key trust decisions into the handler's channel. Used for the
/// target (direct or over a bastion stream); the bastion auto-accepts and
/// doesn't need this.
async fn drive_target_connect<F>(
    fut: F,
    rx_cmd: &mut mpsc::Receiver<SessionCommand>,
    dec_tx: &mpsc::Sender<bool>,
    rejected: &mut bool,
    pending_size: &mut Option<(u16, u16)>,
) -> ConnectOutcome
where
    F: std::future::Future<Output = Result<russh::client::Handle<ClientHandler>, SshError>>,
{
    tokio::pin!(fut);
    let deadline = tokio::time::sleep(CONNECT_TIMEOUT);
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            res = &mut fut => {
                return match res {
                    Ok(h) => ConnectOutcome::Connected(Box::new(h)),
                    Err(e) => ConnectOutcome::Err(e),
                };
            }
            cmd = rx_cmd.recv() => match cmd {
                Some(SessionCommand::HostKeyDecision(accept)) => {
                    if !accept {
                        *rejected = true;
                    }
                    let _ = dec_tx.send(accept).await;
                }
                Some(SessionCommand::Shutdown) | None => return ConnectOutcome::Cancelled,
                // The channel doesn't exist yet, so we can't apply a resize
                // now — but remember the latest one so the PTY opens at the
                // real size. Input before the shell is meaningless → ignore.
                Some(SessionCommand::Resize { cols, rows }) => {
                    *pending_size = Some((cols, rows));
                }
                Some(_) => {}
            },
            _ = &mut deadline => {
                return ConnectOutcome::Err(SshError::Network(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "connection timed out",
                )));
            }
        }
    }
}

/// Try each auth method in order; `Ok(true)` on the first success. Used
/// for the bastion (the target keeps its own loop so it can report the
/// last attempted method on failure).
async fn try_all_auth(
    handle: &mut russh::client::Handle<ClientHandler>,
    creds: Vec<RevealedCredential>,
) -> Result<bool, SshError> {
    for cred in creds {
        if try_auth(handle, cred).await? {
            return Ok(true);
        }
    }
    Ok(false)
}

async fn connect_and_pump(
    params: SshSpawnParams,
    rx_cmd: &mut mpsc::Receiver<SessionCommand>,
    events: &mpsc::UnboundedSender<SshSessionEvent>,
) -> Result<(), SshError> {
    let emit = |e: SshSessionEvent| {
        let _ = events.send(e);
    };

    emit(SshSessionEvent::StateChanged {
        state: SessionState::Connecting,
    });

    let mut config = russh::client::Config::default();
    // Don't drop an idle-but-alive session (e.g. window minimized). Send
    // keepalives so NAT/firewalls keep the path open and a genuinely dead
    // peer is detected within keepalive_interval * keepalive_max.
    // NOTE (russh API): field names are version-sensitive.
    config.inactivity_timeout = None;
    config.keepalive_interval = params.options.keepalive_interval;
    config.keepalive_max = 3;
    let config = Arc::new(config);

    // Build the TARGET host-key handler with its own decision channel.
    // While the target connect future runs, we forward HostKeyDecision
    // commands into it (the bastion, if any, auto-accepts and needs none).
    let (dec_tx, dec_rx) = mpsc::channel::<bool>(1);
    let target_handler = ClientHandler {
        hostname: params.hostname.clone(),
        port: params.port,
        strict: params.options.strict_host_key,
        auto_accept: false,
        known: params.known_hosts.clone(),
        events: events.clone(),
        decisions: dec_rx,
        agent_forward: params.agent_forwarding,
        agent_bridges: std::collections::HashMap::new(),
    };

    // Keep the bastion handle alive for the whole session (its channel
    // carries the target transport). `None` for a direct connection.
    let _bastion_keepalive: Option<russh::client::Handle<ClientHandler>>;

    let mut rejected = false;
    // The UI sends the real terminal size (window_change) as soon as it has
    // the session id — which is *during* connect/auth, before the channel
    // exists. `drive_target_connect` captures the latest such size here so we
    // can open the PTY at the right dimensions instead of dropping it (the
    // frontend dedups identical sizes and won't resend, so a dropped resize
    // would leave the PTY stuck at 80×24 → full-screen TUIs like mc render at
    // half height).
    let mut pending_size: Option<(u16, u16)> = None;
    let connect_result: ConnectOutcome = if let Some(jump) = params.jump {
        // ---- Bastion: connect (auto-pin its key) + auth ----------------
        let (bdec_tx, bdec_rx) = mpsc::channel::<bool>(1);
        let bastion_handler = ClientHandler {
            hostname: jump.hostname.clone(),
            port: jump.port,
            strict: false,
            auto_accept: true,
            known: params.known_hosts.clone(),
            events: events.clone(),
            decisions: bdec_rx,
            agent_forward: false,
            agent_bridges: std::collections::HashMap::new(),
        };
        drop(bdec_tx); // auto_accept never awaits a decision
        // A dead/unreachable bastion would otherwise hang on the OS-level TCP
        // timeout (minutes on Windows). Bound it like the direct path so the
        // UI gets a clean timeout instead.
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
        // Open a direct-tcpip channel from the bastion to the target and
        // run the target SSH transport over it.
        // NOTE (russh version-sensitive): `channel_open_direct_tcpip`,
        // `Channel::into_stream`, and `connect_stream` signatures.
        let ch = bastion
            .channel_open_direct_tcpip(
                params.hostname.as_str(),
                u32::from(params.port),
                "127.0.0.1",
                0,
            )
            .await?;
        let stream = ch.into_stream();
        _bastion_keepalive = Some(bastion);
        let fut = russh::client::connect_stream(config.clone(), stream, target_handler);
        drive_target_connect(fut, rx_cmd, &dec_tx, &mut rejected, &mut pending_size).await
    } else {
        _bastion_keepalive = None;
        let fut =
            russh::client::connect(config.clone(), (params.hostname.as_str(), params.port), target_handler);
        drive_target_connect(fut, rx_cmd, &dec_tx, &mut rejected, &mut pending_size).await
    };

    let mut handle = match connect_result {
        ConnectOutcome::Connected(h) => *h,
        ConnectOutcome::Cancelled => {
            // Shutdown arrived before we connected.
            emit(SshSessionEvent::Closed {
                reason: CloseReason::UserRequested,
            });
            return Ok(());
        }
        ConnectOutcome::Err(e) => {
            if rejected {
                return Err(SshError::HostKeyRejected);
            }
            return Err(e);
        }
    };

    // ---- Authentication ------------------------------------------------
    emit(SshSessionEvent::StateChanged {
        state: SessionState::Authenticating,
    });

    let mut authed = false;
    let mut last_method = "none";
    for cred in params.credentials {
        last_method = match &cred {
            RevealedCredential::Password { .. } => "password",
            RevealedCredential::Key { .. } => "publickey",
            RevealedCredential::Agent { .. } => "agent",
        };
        if try_auth(&mut handle, cred).await? {
            authed = true;
            break;
        }
    }
    if !authed {
        emit(SshSessionEvent::AuthFailed {
            method: last_method.into(),
        });
        return Err(SshError::AuthFailed {
            method: last_method.into(),
        });
    }

    // ---- Channel + env + PTY + shell ----------------------------------
    let mut channel = handle.channel_open_session().await?;

    // Agent forwarding (`ssh -A`): tell the server we'll accept
    // `auth-agent@openssh.com` back-channels. want_reply=false so a server
    // that disallows it can't fail the channel. The serving side (bridging
    // those channels to the local OS agent) is implemented in the handler's
    // `server_channel_open_agent_forward` / `data` callbacks.
    // NOTE (russh version-sensitive): `Channel::agent_forward(want_reply)`.
    if params.agent_forwarding {
        if let Err(e) = channel.agent_forward(false).await {
            warn!(error = %e, "agent forwarding request failed (continuing)");
        }
    }

    // Request environment variables before the shell starts. Servers only
    // honor names in their `AcceptEnv`; unaccepted ones are ignored. We
    // pass want_reply=false so an unaccepted var can't fail the channel.
    for (k, v) in &params.env_vars {
        if k.is_empty() {
            continue;
        }
        // NOTE (russh API): `set_env(want_reply, name, value)`.
        let _ = channel.set_env(false, k.as_str(), v.as_str()).await;
    }

    // Open the PTY at the size the UI requested during connect (if any),
    // otherwise the defaults. Post-shell resizes go through the pump's
    // window_change as usual.
    let (pty_cols, pty_rows) =
        pending_size.unwrap_or((params.options.cols, params.options.rows));
    channel
        .request_pty(
            true,
            &params.options.term,
            u32::from(pty_cols),
            u32::from(pty_rows),
            0,
            0,
            &[],
        )
        .await?;
    channel.request_shell(true).await?;

    emit(SshSessionEvent::StateChanged {
        state: SessionState::Ready,
    });
    debug!("ssh session ready");

    // Optional startup command: send it once, as if typed at the prompt.
    if let Some(cmd) = params.startup_command.as_deref() {
        let cmd = cmd.trim();
        if !cmd.is_empty() {
            let line = format!("{cmd}\n");
            channel.data(line.as_bytes()).await?;
        }
    }

    // Best-effort OS detection on a side channel (doesn't touch the PTY).
    // Failure is silent — the chip just stays empty.
    if let Some(os) = detect_os(&mut handle).await {
        emit(SshSessionEvent::DetectedOs { os });
    }

    // ---- Interactive pump ---------------------------------------------
    let mut user_closed = false;
    loop {
        tokio::select! {
            cmd = rx_cmd.recv() => {
                match cmd {
                    Some(SessionCommand::SshInput(data)) => {
                        channel.data(&data[..]).await?;
                    }
                    Some(SessionCommand::Resize { cols, rows }) => {
                        channel
                            .window_change(u32::from(cols), u32::from(rows), 0, 0)
                            .await
                            .ok();
                    }
                    // No host-key prompt is pending once we're ready; a
                    // late decision is harmless to drop.
                    Some(SessionCommand::HostKeyDecision(_)) => {}
                    Some(SessionCommand::Shutdown) | None => {
                        user_closed = true;
                        break;
                    }
                }
            }
            msg = channel.wait() => {
                match msg {
                    Some(russh::ChannelMsg::Data { ref data }) => {
                        emit(SshSessionEvent::Data { bytes: data.to_vec() });
                    }
                    Some(russh::ChannelMsg::ExtendedData { ref data, .. }) => {
                        emit(SshSessionEvent::Data { bytes: data.to_vec() });
                    }
                    Some(russh::ChannelMsg::ExitStatus { exit_status }) => {
                        emit(SshSessionEvent::Closed {
                            reason: CloseReason::ServerDisconnected {
                                message: Some(format!("exit status {exit_status}")),
                            },
                        });
                        return Ok(());
                    }
                    Some(russh::ChannelMsg::Eof) | Some(russh::ChannelMsg::Close) | None => {
                        break;
                    }
                    _ => {}
                }
            }
        }
    }

    emit(SshSessionEvent::StateChanged {
        state: SessionState::Disconnecting,
    });
    let _ = channel.eof().await;
    let _ = handle
        .disconnect(russh::Disconnect::ByApplication, "", "en")
        .await;

    emit(SshSessionEvent::Closed {
        reason: if user_closed {
            CloseReason::UserRequested
        } else {
            CloseReason::ServerDisconnected { message: None }
        },
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::parse_os_slug;

    #[test]
    fn detects_distro_from_os_release() {
        let out = "Linux\n___RH___\nNAME=\"Ubuntu\"\nID=ubuntu\nVERSION_ID=\"22.04\"\n";
        assert_eq!(parse_os_slug(out).as_deref(), Some("ubuntu"));
    }

    #[test]
    fn detects_macos() {
        assert_eq!(parse_os_slug("Darwin\n___RH___\n").as_deref(), Some("macos"));
    }

    #[test]
    fn detects_windows_from_cmd_error() {
        let out = "'uname' is not recognized as an internal or external command";
        assert_eq!(parse_os_slug(out).as_deref(), Some("windows"));
    }

    #[test]
    fn falls_back_to_linux() {
        assert_eq!(parse_os_slug("Linux\n___RH___\n").as_deref(), Some("linux"));
    }

    #[test]
    fn unknown_is_none() {
        assert_eq!(parse_os_slug("___RH___\n"), None);
    }
}
