//! The SSH session actor: connect → authenticate → open PTY shell →
//! pump bytes both ways until shutdown or disconnect.
//!
//! ⚠️ russh API NOTE (Stage 2 first build): every line that touches
//! `russh` is concentrated here. If `cargo build` reports signature
//! mismatches, they will almost certainly be in this file and are
//! mechanical to fix (method renamed, arg added/removed, `bool` vs
//! `AuthResult`, key type path). The control flow itself is sound.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::mpsc;
use tracing::{debug, instrument};

use crate::error::SshError;
use crate::{
    CloseReason, RevealedCredential, SessionCommand, SessionState, SshSessionEvent,
    SshSpawnParams,
};

/// russh client handler. v1 accepts any server key (TOFU without a
/// prompt); `known_hosts` pinning + interactive confirmation come later.
struct ClientHandler;

#[async_trait]
impl russh::client::Handler for ClientHandler {
    type Error = SshError;

    // NOTE: the key type path (`russh::keys::key::PublicKey`) and this
    // method's exact signature are the most version-sensitive part.
    async fn check_server_key(
        &mut self,
        _server_public_key: &russh::keys::key::PublicKey,
    ) -> Result<bool, Self::Error> {
        Ok(true)
    }
}

#[instrument(level = "debug", skip(params, rx_cmd, events), fields(session_id = %params.id))]
pub async fn run(
    params: SshSpawnParams,
    mut rx_cmd: mpsc::Receiver<SessionCommand>,
    events: mpsc::UnboundedSender<SshSessionEvent>,
) {
    let emit = |e: SshSessionEvent| {
        // Send failures just mean the UI channel is gone; nothing to do.
        let _ = events.send(e);
    };

    if let Err(err) = connect_and_pump(params, &mut rx_cmd, &emit).await {
        let reason = err.into_close_reason();
        emit(SshSessionEvent::Error {
            message: format!("{reason:?}"),
        });
        emit(SshSessionEvent::Closed { reason });
    }
}

/// Attempt a single auth method. Returns `Ok(true)` on success,
/// `Ok(false)` if the server rejected it (or a key couldn't be decoded —
/// we skip rather than abort so other methods still get a turn), and
/// `Err` only for transport-level failures.
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
            // PuTTY .ppk keys can't be read by russh — convert to OpenSSH on
            // the fly. The converted PEM is already decrypted, so we then
            // decode with no passphrase. A plain OpenSSH key is decoded
            // directly (russh applies the passphrase if it's encrypted).
            let (pem, decode_pass): (String, Option<&str>) = if crate::ppk::is_ppk(&pem) {
                match crate::ppk::ppk_to_openssh(&pem, pass.as_deref()) {
                    Ok(converted) => (converted, None),
                    Err(e) => {
                        tracing::warn!(error = %e, "ppk → openssh conversion failed; skipping key");
                        return Ok(false);
                    }
                }
            } else {
                (pem, pass.as_deref())
            };
            drop(private_key_pem); // zeroized
            // russh re-exports key helpers at `russh::keys`. `decode_secret_key`
            // parses OpenSSH / PKCS#8 keys, deciphering with `pass` if encrypted.
            let key = match russh::keys::decode_secret_key(&pem, decode_pass) {
                Ok(k) => k,
                Err(_) => {
                    tracing::warn!("ssh key decode failed; skipping key");
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
    }
}

async fn connect_and_pump(
    params: SshSpawnParams,
    rx_cmd: &mut mpsc::Receiver<SessionCommand>,
    emit: &impl Fn(SshSessionEvent),
) -> Result<(), SshError> {
    emit(SshSessionEvent::StateChanged {
        state: SessionState::Connecting,
    });

    let mut config = russh::client::Config::default();
    // Don't drop an idle-but-alive session (e.g. window minimized). Instead
    // send keepalives so NAT/firewalls keep the path open and a genuinely
    // dead peer is detected within keepalive_interval * keepalive_max.
    // NOTE (russh API): field names are version-sensitive — adjust if the
    // build complains (`inactivity_timeout` / `keepalive_interval` /
    // `keepalive_max`).
    config.inactivity_timeout = None;
    config.keepalive_interval = Some(
        params
            .options
            .keepalive_interval
            .unwrap_or_else(|| Duration::from_secs(30)),
    );
    config.keepalive_max = 3;
    let config = Arc::new(config);
    let mut handle =
        russh::client::connect(config, (params.hostname.as_str(), params.port), ClientHandler)
            .await?;

    // ---- Authentication ------------------------------------------------
    emit(SshSessionEvent::StateChanged {
        state: SessionState::Authenticating,
    });

    // Try each available method (typically key(s) first, then password).
    // Auth fails only when every method is rejected; a bad/undecodable key
    // is skipped so a working password can still get us in.
    let mut authed = false;
    let mut last_method = "none";
    for cred in params.credentials {
        last_method = match &cred {
            RevealedCredential::Password { .. } => "password",
            RevealedCredential::Key { .. } => "publickey",
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

    // ---- Channel + PTY + shell ----------------------------------------
    let mut channel = handle.channel_open_session().await?;
    channel
        .request_pty(
            true,
            &params.options.term,
            u32::from(params.options.cols),
            u32::from(params.options.rows),
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

    // ---- Interactive pump ---------------------------------------------
    // NOTE: if the borrow checker objects to using `channel` in both the
    // command arm and `channel.wait()`, switch to `channel.into_stream()`
    // + `tokio::io::split` (and route resize through the session handle).
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
                        // stderr — xterm.js doesn't distinguish streams.
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
    // Best-effort graceful close.
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
