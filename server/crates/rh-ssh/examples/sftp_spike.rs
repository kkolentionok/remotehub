//! Isolated russh-sftp connectivity spike (Stage SFTP-0).
//!
//! Mirrors the proven rh-ssh connect/auth path, then opens the `sftp`
//! subsystem and exercises the russh-sftp client API against a REAL host —
//! BEFORE we build the actor + two-pane UI. Project rule: spike unproven
//! russh surface in isolation first (this is the SFTP analogue of
//! `rdp_spike`).
//!
//! What it proves: connect → auth → `channel_open_session` →
//! `request_subsystem("sftp")` → `SftpSession::new` → `read_dir` →
//! (optional) download one file.
//!
//! Run (PowerShell), pointed at one of your SSH hosts:
//!
//!   $env:SFTP_HOST="1.2.3.4"; $env:SFTP_USER="root"; $env:SFTP_PASS="secret"
//!   cargo run -p rh-ssh --example sftp_spike
//!
//! Or with an OpenSSH key (NOT .ppk — the .ppk converter lives in the
//! private `ppk` module and isn't reachable from an example):
//!
//!   $env:SFTP_KEY="C:\Users\me\.ssh\id_ed25519"   # optional: SFTP_KEY_PASS
//!   cargo run -p rh-ssh --example sftp_spike
//!
//! Optional:
//!   $env:SFTP_PORT="2222"          # default 22
//!   $env:SFTP_PATH="/var/log"      # dir to list, default "."
//!   $env:SFTP_GET="/etc/hostname"  # download one file -> ./sftp_spike_out

use std::sync::Arc;

/// Spike host-key handler: trust everything. (Real code pins via TOFU.)
struct TrustAll;

#[async_trait::async_trait]
impl russh::client::Handler for TrustAll {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &russh::keys::key::PublicKey,
    ) -> Result<bool, Self::Error> {
        Ok(true)
    }
}

fn env(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|s| !s.is_empty())
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let host = env("SFTP_HOST").expect("set SFTP_HOST");
    let port: u16 = env("SFTP_PORT").and_then(|s| s.parse().ok()).unwrap_or(22);
    let user = env("SFTP_USER").expect("set SFTP_USER");
    let path = env("SFTP_PATH").unwrap_or_else(|| ".".to_string());

    // ---- connect (same Config + connect call as rh-ssh actor) ----
    let config = Arc::new(russh::client::Config::default());
    let mut handle = russh::client::connect(config, (host.as_str(), port), TrustAll).await?;
    println!("[ok] connected to {host}:{port}");

    // ---- auth: key if SFTP_KEY set, else password ----
    let authed = if let Some(keypath) = env("SFTP_KEY") {
        let pem = std::fs::read_to_string(&keypath)?;
        let pass = env("SFTP_KEY_PASS");
        let key = russh::keys::decode_secret_key(&pem, pass.as_deref())?;
        handle
            .authenticate_publickey(&user, Arc::new(key))
            .await?
    } else {
        let pw = env("SFTP_PASS").expect("set SFTP_PASS (or SFTP_KEY)");
        handle.authenticate_password(&user, &pw).await?
    };
    if !authed {
        return Err("authentication failed (all methods rejected)".into());
    }
    println!("[ok] authenticated as {user}");

    // ---- open the sftp subsystem ----
    let channel = handle.channel_open_session().await?;
    channel.request_subsystem(true, "sftp").await?;
    let sftp = russh_sftp::client::SftpSession::new(channel.into_stream()).await?;
    println!("[ok] sftp subsystem open");

    // ---- list a directory ----
    println!("--- read_dir({path}) ---");
    let entries = sftp.read_dir(path.as_str()).await?;
    let mut n = 0usize;
    for entry in entries {
        let meta = entry.metadata();
        let kind = if meta.is_dir() { 'd' } else { '-' };
        let size = meta.size.unwrap_or(0);
        println!("  {kind} {size:>12}  {}", entry.file_name());
        n += 1;
    }
    println!("[ok] {n} entries");

    // ---- optional: download one file ----
    if let Some(get) = env("SFTP_GET") {
        use tokio::io::AsyncReadExt;
        let mut f = sftp.open(get.as_str()).await?;
        let mut buf = Vec::new();
        f.read_to_end(&mut buf).await?;
        std::fs::write("sftp_spike_out", &buf)?;
        println!("[ok] downloaded {} bytes from {get} -> ./sftp_spike_out", buf.len());
    }

    println!("[done] SFTP spike OK");
    Ok(())
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("[SPIKE ERROR] {e}");
        std::process::exit(1);
    }
}
