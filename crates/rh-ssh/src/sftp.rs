//! SFTP connection for the file browser.
//!
//! Opens its OWN SSH connection to the host (independent of any live shell
//! session) and the `sftp` subsystem on top, exactly as validated by
//! `examples/sftp_spike`. Holds the `SftpSession` + the russh `Handle`
//! (keeping the transport alive) and exposes directory listing.
//!
//! Auth mirrors the actor's per-credential logic (password + key, incl.
//! .ppk conversion). Agent auth is not wired here yet (follow-up). Host-key
//! verification is currently trust-all — the host is already pinned via the
//! shell-session path; wiring SFTP through the same TOFU store is a
//! follow-up.

use std::sync::Arc;

use serde::Serialize;

use crate::{RevealedCredential, SshError};
use rh_core::{KnownHostKey, KnownHostsStore};

/// One remote directory entry (wire-compatible with the UI `FsEntry`).
#[derive(Debug, Serialize)]
pub struct SftpEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    /// Unix epoch seconds of last modification, if the server reported it.
    pub modified: Option<i64>,
    /// POSIX permission string like "-rw-r--r--", if reported.
    pub perms: Option<String>,
}

/// Result of listing a remote directory (mirrors the UI `FsListResponse`).
#[derive(Debug, Serialize)]
pub struct SftpListing {
    pub path: String,
    pub parent: Option<String>,
    pub entries: Vec<SftpEntry>,
}

/// Host-key handler for SFTP connections. Silent TOFU against the shared
/// `known_hosts` store: first sight pins the key, a matching key is accepted,
/// a *changed* key is rejected (the interactive prompt lives on the shell
/// path; SFTP reuses whatever that pinned).
struct SftpHostKey {
    known: Arc<dyn KnownHostsStore>,
    hostname: String,
    port: u16,
}

#[async_trait::async_trait]
impl russh::client::Handler for SftpHostKey {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &russh::keys::key::PublicKey,
    ) -> Result<bool, Self::Error> {
        let fingerprint = crate::actor::fingerprint_sha256(server_public_key);
        let key_type = server_public_key.name().to_string();
        match self.known.lookup(&self.hostname, self.port).await {
            Ok(Some(k)) if k.fingerprint_sha256 == fingerprint => Ok(true),
            Ok(Some(_)) => Ok(false), // changed key → refuse
            Ok(None) => {
                // trust on first use
                let entry = KnownHostKey { key_type, fingerprint_sha256: fingerprint };
                let _ = self.known.remember(&self.hostname, self.port, &entry).await;
                Ok(true)
            }
            Err(_) => Ok(true), // store unavailable → don't lock the user out
        }
    }
}

/// A live SFTP connection: the subsystem session plus the transport handle
/// that must outlive it.
pub struct SftpConn {
    sftp: russh_sftp::client::SftpSession,
    /// Keep the SSH transport alive for the session's lifetime.
    _handle: russh::client::Handle<SftpHostKey>,
}

impl SftpConn {
    /// Connect, authenticate (trying each credential in order), and open
    /// the `sftp` subsystem. Host key is TOFU-pinned via `known`.
    pub async fn connect(
        hostname: &str,
        port: u16,
        credentials: Vec<RevealedCredential>,
        known: Arc<dyn KnownHostsStore>,
    ) -> Result<Self, SshError> {
        let config = Arc::new(russh::client::Config::default());
        let handler = SftpHostKey {
            known,
            hostname: hostname.to_string(),
            port,
        };
        let mut handle = russh::client::connect(config, (hostname, port), handler).await?;

        let mut authed = false;
        for cred in credentials {
            if try_auth(&mut handle, cred).await {
                authed = true;
                break;
            }
        }
        if !authed {
            return Err(SshError::AuthFailed {
                method: "sftp".into(),
            });
        }

        let channel = handle.channel_open_session().await?;
        channel.request_subsystem(true, "sftp").await?;
        let sftp = russh_sftp::client::SftpSession::new(channel.into_stream())
            .await
            .map_err(|e| SshError::Sftp(format!("sftp init: {e}")))?;

        Ok(Self {
            sftp,
            _handle: handle,
        })
    }

    /// List a remote directory. An empty or "." path resolves to the
    /// login directory (REALPATH) so navigation always uses absolute paths.
    pub async fn list(&self, path: &str) -> Result<SftpListing, SshError> {
        let listed = if path.is_empty() || path == "." {
            self.sftp
                .canonicalize(".")
                .await
                .unwrap_or_else(|_| ".".to_string())
        } else {
            path.to_string()
        };

        let read = self
            .sftp
            .read_dir(listed.clone())
            .await
            .map_err(|e| SshError::Sftp(format!("read_dir {listed}: {e}")))?;

        let mut entries: Vec<SftpEntry> = Vec::new();
        for entry in read {
            let name = entry.file_name();
            if name == "." || name == ".." {
                continue;
            }
            let meta = entry.metadata();
            let is_dir = meta.is_dir();
            let size = meta.size.unwrap_or(0);
            entries.push(SftpEntry {
                path: join_posix(&listed, &name),
                name,
                is_dir,
                size: if is_dir { 0 } else { size },
                modified: meta.mtime.map(|t| i64::from(t)),
                perms: meta.permissions.map(fmt_perms),
            });
        }
        entries.sort_by(|a, b| {
            b.is_dir
                .cmp(&a.is_dir)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });

        Ok(SftpListing {
            parent: parent_posix(&listed),
            path: listed,
            entries,
        })
    }

    /// Read a whole remote file into memory. (Buffered — streaming is a
    /// follow-up with the transfer queue.)
    pub async fn read_file(&self, remote_path: &str) -> Result<Vec<u8>, SshError> {
        use tokio::io::AsyncReadExt;
        let mut f = self
            .sftp
            .open(remote_path)
            .await
            .map_err(|e| SshError::Sftp(format!("open {remote_path}: {e}")))?;
        let mut buf = Vec::new();
        f.read_to_end(&mut buf).await?;
        Ok(buf)
    }

    /// Write `data` into remote `remote_dir` under `file_name`. Returns the
    /// remote destination path.
    pub async fn put_in_dir(
        &self,
        remote_dir: &str,
        file_name: &str,
        data: &[u8],
    ) -> Result<String, SshError> {
        use tokio::io::AsyncWriteExt;
        let dest = join_posix(remote_dir, file_name);
        let mut f = self
            .sftp
            .create(dest.clone())
            .await
            .map_err(|e| SshError::Sftp(format!("create {dest}: {e}")))?;
        f.write_all(data).await?;
        f.shutdown().await?;
        Ok(dest)
    }

    /// Download a remote file into `local_dir`, keeping its name. Returns
    /// the local destination path.
    pub async fn download(&self, remote_path: &str, local_dir: &str) -> Result<String, SshError> {
        let name = remote_path.rsplit('/').next().unwrap_or(remote_path);
        let dest = std::path::Path::new(local_dir).join(name);
        let data = self.read_file(remote_path).await?;
        std::fs::write(&dest, &data)?;
        Ok(dest.to_string_lossy().into_owned())
    }

    /// Upload a local file into remote `remote_dir`, keeping its name.
    /// Returns the remote destination path.
    pub async fn upload(&self, local_path: &str, remote_dir: &str) -> Result<String, SshError> {
        let name = std::path::Path::new(local_path)
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "file".to_string());
        let data = std::fs::read(local_path)?;
        self.put_in_dir(remote_dir, &name, &data).await
    }

    /// Stream a remote file to `local_dir` in chunks, reporting bytes copied
    /// and honouring the cancel flag. Returns the local destination path.
    pub async fn download_stream(
        &self,
        remote: &str,
        local_dir: &str,
        dst_name: Option<&str>,
        offset: u64,
        cancel: &std::sync::atomic::AtomicBool,
        progress: &mut (dyn FnMut(u64) + Send),
    ) -> Result<String, SshError> {
        use std::io::Write;
        use tokio::io::{AsyncReadExt, AsyncSeekExt};
        let name = dst_name.unwrap_or_else(|| remote.rsplit('/').next().unwrap_or(remote));
        let dest = std::path::Path::new(local_dir).join(name);
        let mut rf = self
            .sftp
            .open(remote)
            .await
            .map_err(|e| SshError::Sftp(format!("open {remote}: {e}")))?;
        // Resume: seek the remote read cursor and append to the local file
        // instead of truncating it.
        let mut lf = if offset > 0 {
            rf.seek(std::io::SeekFrom::Start(offset))
                .await
                .map_err(|e| SshError::Sftp(format!("seek {remote}: {e}")))?;
            std::fs::OpenOptions::new().create(true).append(true).open(&dest)?
        } else {
            std::fs::File::create(&dest)?
        };
        let mut buf = vec![0u8; TRANSFER_CHUNK];
        let mut total: u64 = offset;
        progress(total);
        loop {
            if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                return Err(SshError::Sftp("cancelled".to_string()));
            }
            let n = rf.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            lf.write_all(&buf[..n])?;
            total += n as u64;
            progress(total);
        }
        lf.flush()?;
        Ok(dest.to_string_lossy().into_owned())
    }

    /// Stream a local file to remote `remote_dir` in chunks, reporting bytes
    /// copied and honouring the cancel flag. Returns the remote dest path.
    pub async fn upload_stream(
        &self,
        local: &str,
        remote_dir: &str,
        dst_name: Option<&str>,
        offset: u64,
        cancel: &std::sync::atomic::AtomicBool,
        progress: &mut (dyn FnMut(u64) + Send),
    ) -> Result<String, SshError> {
        use std::io::{Read, Seek};
        use tokio::io::AsyncWriteExt;
        let name = match dst_name {
            Some(n) => n.to_string(),
            None => std::path::Path::new(local)
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "file".to_string()),
        };
        let dest = join_posix(remote_dir, &name);
        let mut lf = std::fs::File::open(local)?;
        // Resume: skip the already-uploaded prefix locally and append on the
        // remote (open WRITE|CREATE|APPEND) instead of truncating.
        let mut rf = if offset > 0 {
            lf.seek(std::io::SeekFrom::Start(offset))?;
            use russh_sftp::protocol::OpenFlags;
            self.sftp
                .open_with_flags(
                    dest.clone(),
                    OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::APPEND,
                )
                .await
                .map_err(|e| SshError::Sftp(format!("open(append) {dest}: {e}")))?
        } else {
            self.sftp
                .create(dest.clone())
                .await
                .map_err(|e| SshError::Sftp(format!("create {dest}: {e}")))?
        };
        let mut buf = vec![0u8; TRANSFER_CHUNK];
        let mut total: u64 = offset;
        progress(total);
        loop {
            if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                return Err(SshError::Sftp("cancelled".to_string()));
            }
            let n = lf.read(&mut buf)?;
            if n == 0 {
                break;
            }
            rf.write_all(&buf[..n]).await?;
            total += n as u64;
            progress(total);
        }
        rf.shutdown().await?;
        Ok(dest)
    }

    /// Create a directory `name` inside remote `parent`.
    pub async fn mkdir(&self, parent: &str, name: &str) -> Result<(), SshError> {
        let dest = join_posix(parent, name);
        self.sftp
            .create_dir(dest.clone())
            .await
            .map_err(|e| SshError::Sftp(format!("mkdir {dest}: {e}")))?;
        Ok(())
    }

    /// Change POSIX permission bits on a remote entry.
    pub async fn chmod(&self, path: &str, mode: u32) -> Result<(), SshError> {
        let attrs = russh_sftp::protocol::FileAttributes {
            permissions: Some(mode),
            ..Default::default()
        };
        self.sftp
            .set_metadata(path.to_string(), attrs)
            .await
            .map_err(|e| SshError::Sftp(format!("chmod {path}: {e}")))?;
        Ok(())
    }

    /// Size of a remote file in bytes (0 if missing/unknown). Used to
    /// compute the resume offset for an interrupted transfer.
    pub async fn size(&self, path: &str) -> u64 {
        self.sftp
            .metadata(path.to_string())
            .await
            .ok()
            .and_then(|m| m.size)
            .unwrap_or(0)
    }

    /// Rename a remote entry in place (same parent directory).
    pub async fn rename(&self, path: &str, new_name: &str) -> Result<(), SshError> {
        let parent = parent_posix(path).unwrap_or_else(|| "/".to_string());
        let dest = join_posix(&parent, new_name);
        self.sftp
            .rename(path.to_string(), dest)
            .await
            .map_err(|e| SshError::Sftp(format!("rename: {e}")))?;
        Ok(())
    }

    /// Remove a remote file, or a directory and everything under it.
    pub async fn remove(&self, path: &str, is_dir: bool) -> Result<(), SshError> {
        if is_dir {
            self.remove_dir_recursive(path.to_string()).await
        } else {
            self.sftp
                .remove_file(path.to_string())
                .await
                .map_err(|e| SshError::Sftp(format!("remove_file: {e}")))
        }
    }

    fn remove_dir_recursive<'a>(
        &'a self,
        dir: String,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), SshError>> + Send + 'a>> {
        Box::pin(async move {
            let rd = self
                .sftp
                .read_dir(dir.clone())
                .await
                .map_err(|e| SshError::Sftp(format!("read_dir {dir}: {e}")))?;
            for entry in rd {
                let name = entry.file_name();
                if name == "." || name == ".." {
                    continue;
                }
                let full = join_posix(&dir, &name);
                if entry.metadata().is_dir() {
                    self.remove_dir_recursive(full).await?;
                } else {
                    self.sftp
                        .remove_file(full.clone())
                        .await
                        .map_err(|e| SshError::Sftp(format!("remove_file {full}: {e}")))?;
                }
            }
            self.sftp
                .remove_dir(dir.clone())
                .await
                .map_err(|e| SshError::Sftp(format!("rmdir {dir}: {e}")))?;
            Ok(())
        })
    }
}

/// Try one credential. Returns `true` on a successful authentication.
async fn try_auth(
    handle: &mut russh::client::Handle<SftpHostKey>,
    cred: RevealedCredential,
) -> bool {
    match cred {
        RevealedCredential::Password { username, password } => {
            let pw = String::from_utf8_lossy(password.expose()).into_owned();
            handle
                .authenticate_password(&username, &pw)
                .await
                .unwrap_or(false)
        }
        RevealedCredential::Key {
            username,
            private_key_pem,
            passphrase,
        } => {
            let pem = String::from_utf8_lossy(private_key_pem.expose()).into_owned();
            let pass = passphrase
                .as_ref()
                .and_then(|p| p.as_str().map(str::to_owned));
            // .ppk → OpenSSH on the fly (russh can't read .ppk); converted
            // PEM is already decrypted.
            let (pem, decode_pass): (String, Option<&str>) = if crate::ppk::is_ppk(&pem) {
                match crate::ppk::ppk_to_openssh(&pem, pass.as_deref()) {
                    Ok(converted) => (converted, None),
                    Err(_) => return false,
                }
            } else {
                (pem, pass.as_deref())
            };
            let key = match russh::keys::decode_secret_key(&pem, decode_pass) {
                Ok(k) => k,
                Err(_) => return false,
            };
            handle
                .authenticate_publickey(&username, Arc::new(key))
                .await
                .unwrap_or(false)
        }
        // SSH-agent auth (Pageant / OpenSSH agent), mirroring the shell actor.
        RevealedCredential::Agent { username } => try_auth_agent(handle, &username).await,
    }
}

/// SSH-agent auth for the SFTP transport. Best-effort: any failure to reach
/// the agent or sign returns `false` so the next credential is tried.
async fn try_auth_agent(handle: &mut russh::client::Handle<SftpHostKey>, username: &str) -> bool {
    use russh::keys::agent::client::AgentClient;

    #[cfg(unix)]
    let mut agent = match AgentClient::connect_env().await {
        Ok(a) => a,
        Err(_) => return false,
    };
    #[cfg(windows)]
    let mut agent = {
        use tokio::net::windows::named_pipe::ClientOptions;
        match ClientOptions::new().open(r"\\.\pipe\openssh-ssh-agent") {
            Ok(pipe) => AgentClient::connect(pipe),
            Err(_) => return false,
        }
    };
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (handle, username);
        return false;
    }

    #[cfg(any(unix, windows))]
    {
        let identities = match agent.request_identities().await {
            Ok(ids) => ids,
            Err(_) => return false,
        };
        for key in identities {
            let (returned, res) = handle.authenticate_future(username, key, agent).await;
            agent = returned;
            if matches!(res, Ok(true)) {
                return true;
            }
        }
        false
    }
}

/// Stream a file directly between two SFTP connections in chunks, reporting
/// bytes copied and honouring the cancel flag. The caller holds whatever
/// locks guard the two connections. Returns the remote destination path.
pub async fn copy_stream(
    src_conn: &SftpConn,
    dst_conn: &SftpConn,
    src_path: &str,
    dst_dir: &str,
    dst_name: Option<&str>,
    offset: u64,
    cancel: &std::sync::atomic::AtomicBool,
    progress: &mut (dyn FnMut(u64) + Send),
) -> Result<String, SshError> {
    use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
    let name = dst_name.unwrap_or_else(|| src_path.rsplit('/').next().unwrap_or(src_path));
    let dest = join_posix(dst_dir, name);
    let mut rf = src_conn
        .sftp
        .open(src_path)
        .await
        .map_err(|e| SshError::Sftp(format!("open {src_path}: {e}")))?;
    let mut wf = if offset > 0 {
        rf.seek(std::io::SeekFrom::Start(offset))
            .await
            .map_err(|e| SshError::Sftp(format!("seek {src_path}: {e}")))?;
        use russh_sftp::protocol::OpenFlags;
        dst_conn
            .sftp
            .open_with_flags(
                dest.clone(),
                OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::APPEND,
            )
            .await
            .map_err(|e| SshError::Sftp(format!("open(append) {dest}: {e}")))?
    } else {
        dst_conn
            .sftp
            .create(dest.clone())
            .await
            .map_err(|e| SshError::Sftp(format!("create {dest}: {e}")))?
    };
    let mut buf = vec![0u8; TRANSFER_CHUNK];
    let mut total: u64 = offset;
    progress(total);
    loop {
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(SshError::Sftp("cancelled".to_string()));
        }
        let n = rf.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        wf.write_all(&buf[..n]).await?;
        total += n as u64;
        progress(total);
    }
    wf.shutdown().await?;
    Ok(dest)
}

/// Chunk size for streamed transfers (256 KiB).
const TRANSFER_CHUNK: usize = 256 * 1024;

/// Format POSIX mode bits into an `ls -l`-style string, e.g. `-rwxr-xr-x`.
fn fmt_perms(mode: u32) -> String {
    let type_ch = match mode & 0o170000 {
        0o040000 => 'd',
        0o120000 => 'l',
        0o060000 => 'b',
        0o020000 => 'c',
        0o010000 => 'p',
        0o140000 => 's',
        _ => '-',
    };
    let rwx = |bits: u32| {
        [
            if bits & 0o4 != 0 { 'r' } else { '-' },
            if bits & 0o2 != 0 { 'w' } else { '-' },
            if bits & 0o1 != 0 { 'x' } else { '-' },
        ]
    };
    let mut s = String::with_capacity(10);
    s.push(type_ch);
    for group in [(mode >> 6) & 0o7, (mode >> 3) & 0o7, mode & 0o7] {
        for c in rwx(group) {
            s.push(c);
        }
    }
    s
}

/// Join a POSIX base path with a child name.
fn join_posix(base: &str, name: &str) -> String {
    if base == "/" {
        format!("/{name}")
    } else if base.ends_with('/') {
        format!("{base}{name}")
    } else {
        format!("{base}/{name}")
    }
}

/// POSIX parent directory, or `None` at the root.
fn parent_posix(path: &str) -> Option<String> {
    if path == "/" || path.is_empty() {
        return None;
    }
    let trimmed = path.trim_end_matches('/');
    match trimmed.rfind('/') {
        None => None,
        Some(0) => Some("/".to_string()),
        Some(i) => Some(trimmed[..i].to_string()),
    }
}
