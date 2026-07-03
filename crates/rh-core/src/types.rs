//! Domain entity types: `Host`, `HostGroup`, `Credential`, and the
//! enums they reference.
//!
//! These are plain data structures — no behaviour, no I/O. Storage
//! adapters in `rh-storage` convert between these and SQL rows;
//! Tauri command handlers in `rh-app` convert between these and DTOs.
//!
//! All timestamps are UTC. Construction APIs (`new_*` factories) stamp
//! `created_at = updated_at = now()` for convenience; mutation helpers
//! refresh `updated_at` automatically.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::id::{CredentialId, ForwardId, GroupId, HostId, SnippetId};

/// Network protocol used for a connection.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    Ssh,
    Rdp,
}

impl Protocol {
    /// Default TCP port for this protocol per IANA / Microsoft convention.
    /// Used when the user creates a host without specifying a port.
    #[must_use]
    pub const fn default_port(self) -> u16 {
        match self {
            Protocol::Ssh => 22,
            Protocol::Rdp => 3389,
        }
    }

    /// Human-readable name for UI / logs.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Protocol::Ssh => "ssh",
            Protocol::Rdp => "rdp",
        }
    }
}

/// Direction of an SSH port forward (`ssh -L` / `-R` / `-D`). Lives here
/// (not in `rh-ssh`) so both the session layer and storage can share it;
/// `rh-ssh` re-exports it as `rh_ssh::ForwardKind`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ForwardKind {
    /// `-L`: listen locally, tunnel to a host reachable from the remote side.
    Local,
    /// `-R`: the server listens, tunnel back to a host reachable from us.
    Remote,
    /// `-D`: local SOCKS5 proxy; per-connection target chosen by the client.
    Dynamic,
}

impl ForwardKind {
    /// Stable lowercase tag used in storage (`forwards.kind` column) and
    /// matching the serde representation.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            ForwardKind::Local => "local",
            ForwardKind::Remote => "remote",
            ForwardKind::Dynamic => "dynamic",
        }
    }

    /// Parse the storage tag back into a kind. `None` for an unknown
    /// string (treated as malformed at the storage boundary).
    #[must_use]
    pub fn from_tag(s: &str) -> Option<Self> {
        match s {
            "local" => Some(ForwardKind::Local),
            "remote" => Some(ForwardKind::Remote),
            "dynamic" => Some(ForwardKind::Dynamic),
            _ => None,
        }
    }
}

/// Kind of credential. Determines what's stored in the keychain
/// and how it's used during authentication.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum CredentialKind {
    /// Password authentication. Keychain holds UTF-8 password bytes.
    Password,
    /// SSH key authentication. Keychain holds the PEM-encoded private
    /// key; if the key is encrypted, the passphrase lives at a separate
    /// keychain entry (see [`KeychainRef::for_passphrase`]).
    ///
    /// [`KeychainRef::for_passphrase`]: crate::KeychainRef::for_passphrase
    SshKey,
    /// SSH agent authentication. No secret in keychain — credential
    /// just identifies a username and signals "ask the OS-side SSH
    /// agent for signing". Reserved for post-MVP.
    SshKeyAgent,
}

impl CredentialKind {
    /// True if this kind requires a secret to be stored in the keychain.
    /// `SshKeyAgent` is the lone exception.
    #[must_use]
    pub const fn requires_keychain_secret(self) -> bool {
        !matches!(self, CredentialKind::SshKeyAgent)
    }
}

/// A single environment variable to inject into a session's shell on
/// connect. Persisted as part of `Host::env_vars`, serialized to the
/// `env_vars_json` column as a JSON array (order-preserving — the UI
/// shows them in the order the user typed). Duplicate keys are not
/// rejected at this layer; the validation boundary may dedupe.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EnvVar {
    pub key: String,
    pub value: String,
}

impl EnvVar {
    #[must_use]
    pub fn new(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            value: value.into(),
        }
    }
}

/// A target host (server, workstation, VM) the user wants to connect to.
///
/// Hosts are persisted in SQLite and identified by ULID. The `tags`
/// field is serialized as a JSON array in the database (column
/// `tags_json`) but exposed here as a plain `Vec<String>`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Host {
    pub id: HostId,
    pub name: String,
    /// Explicit user-facing label. `None` means "no label set" — the UI
    /// falls back to displaying `hostname`. Distinct from `name`, which
    /// is the canonical sort/search key and is always non-empty (the
    /// app layer keeps it = display_name-or-hostname). Added in Stage
    /// 1.8 to retire the fragile `name == hostname` auto-label heuristic.
    pub display_name: Option<String>,
    pub group_id: Option<GroupId>,
    pub protocol: Protocol,
    pub hostname: String,
    pub port: u16,
    /// Login user for the session. Lives on the host (not the credential)
    /// so one SSH key can be reused across hosts that log in as different
    /// users. Empty string means "unset"; the session layer then falls
    /// back to the credential's own username for backward compatibility.
    pub username: String,
    pub tags: Vec<String>,
    /// Hex color `#RRGGBB`, used in UI for visual grouping. Optional.
    pub color: Option<String>,
    /// Free-form notes (markdown). Optional, max 10 000 chars enforced
    /// at the validation boundary, not in the type itself.
    pub notes: Option<String>,
    /// Command executed automatically on session open (SSH). `None`
    /// means no startup command. Consumed by the Stage 2 session actor.
    pub startup_command: Option<String>,
    /// Environment variables injected into the session shell. Empty by
    /// default. Order-preserving.
    pub env_vars: Vec<EnvVar>,
    /// OS slug auto-detected after a successful connect (e.g. `"ubuntu"`,
    /// `"debian"`, `"windows"`). `None` until detection runs. Machine-set
    /// only — never written through the normal create/update path; the
    /// Stage 2.2 detection routine populates it. Drives the host icon.
    pub detected_os: Option<String>,
    pub default_credential_id: Option<CredentialId>,
    /// Optional bastion: another saved host to route this connection
    /// through (ProxyJump). The bastion's own hostname/port/username/
    /// credentials are reused. `None` = connect directly. One level only.
    pub jump_host_id: Option<HostId>,
    /// Forward the local SSH agent to this host (`ssh -A`). The actor
    /// requests agent forwarding and bridges the server's back-channels
    /// to the OS agent. Off by default (forwarding has security caveats).
    pub agent_forwarding: bool,
    /// User-pinned favorite. Surfaced in the tray's Favorites submenu and
    /// (optionally) starred in the UI. Set by the user via a star toggle.
    /// Defaults to false.
    pub favorite: bool,
    /// When a session to this host last reached the `Ready` state.
    /// Machine-set by the session layer (never through create/update);
    /// `None` until the first successful connect. Drives the
    /// "last connection" line in the host info panel.
    pub last_connected_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Host {
    /// Build a new host with the minimum required fields. Optional
    /// fields default to empty; timestamps are stamped to `now()`.
    ///
    /// If `port` is `None`, the protocol's default port is used.
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        hostname: impl Into<String>,
        protocol: Protocol,
        port: Option<u16>,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: HostId::new(),
            name: name.into(),
            display_name: None,
            group_id: None,
            protocol,
            hostname: hostname.into(),
            port: port.unwrap_or_else(|| protocol.default_port()),
            username: String::new(),
            tags: Vec::new(),
            color: None,
            notes: None,
            startup_command: None,
            env_vars: Vec::new(),
            detected_os: None,
            default_credential_id: None,
            jump_host_id: None,
            agent_forwarding: false,
            favorite: false,
            last_connected_at: None,
            created_at: now,
            updated_at: now,
        }
    }

    /// Touch the `updated_at` timestamp. Call after any mutating change.
    pub fn touch(&mut self) {
        self.updated_at = Utc::now();
    }
}

/// A folder that groups hosts. Hierarchical (`parent_id` may point to
/// another group). Cycle prevention is the storage layer's responsibility.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostGroup {
    pub id: GroupId,
    pub name: String,
    pub parent_id: Option<GroupId>,
    pub created_at: DateTime<Utc>,
}

impl HostGroup {
    #[must_use]
    pub fn new(name: impl Into<String>, parent_id: Option<GroupId>) -> Self {
        Self {
            id: GroupId::new(),
            name: name.into(),
            parent_id,
            created_at: Utc::now(),
        }
    }
}

/// A reusable command ("snippet") the user can run into an active session
/// or copy. Not a secret — stored in SQLite like hosts/groups.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Snippet {
    pub id: SnippetId,
    pub name: String,
    /// The command text (may be multi-line).
    pub command: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Snippet {
    #[must_use]
    pub fn new(name: impl Into<String>, command: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            id: SnippetId::new(),
            name: name.into(),
            command: command.into(),
            created_at: now,
            updated_at: now,
        }
    }
}

/// A pinned SSH host key, looked up by `(hostname, port)`.
///
/// Stored in SQLite (it's public material — not a secret) to support
/// TOFU: on first connect the user trusts the presented key and we
/// persist its fingerprint; on later connects a mismatch is surfaced as
/// a "host key changed" warning. The fingerprint is the OpenSSH SHA256
/// form — `base64(sha256(public_key_blob))` without padding, i.e. the
/// part `ssh-keygen -lf` prints after `SHA256:`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct KnownHostKey {
    /// Algorithm name, e.g. `ssh-ed25519`, `rsa-sha2-512`.
    pub key_type: String,
    /// OpenSSH SHA256 fingerprint, no `SHA256:` prefix, no base64 padding.
    pub fingerprint_sha256: String,
}

/// A full pinned-host record for the management list (identity + when
/// it was trusted). [`KnownHostKey`] is the lookup-shaped subset.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct KnownHostEntry {
    pub hostname: String,
    pub port: u16,
    pub key_type: String,
    pub fingerprint_sha256: String,
    pub created_at: DateTime<Utc>,
}

/// A trusted RDP server certificate (TOFU pin), the RDP analog of
/// [`KnownHostKey`]. `subject` is the cert's CN/subject for display.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TrustedCert {
    pub fingerprint_sha256: String,
    pub subject: String,
    pub trusted_at: DateTime<Utc>,
}

/// A pinned RDP cert with its host identity, for the management list.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RdpCertEntry {
    pub hostname: String,
    pub port: u16,
    pub fingerprint_sha256: String,
    pub subject: String,
    pub trusted_at: DateTime<Utc>,
}

/// A reusable credential — login material for one or more hosts.
///
/// The actual secret is **not** in this struct; it lives in the OS
/// keychain at `keychain_ref`. The struct holds only metadata that's
/// safe to persist in SQLite.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Credential {
    pub id: CredentialId,
    pub name: String,
    pub kind: CredentialKind,
    /// SSH/RDP username for authentication. May be empty for
    /// `SshKeyAgent` where the agent handles user lookup.
    pub username: String,
    /// Opaque pointer to the secret in OS keychain. Constructed via
    /// [`KeychainRef::for_credential`]; storage layer reconstructs
    /// this from the credential ID on load — it's kept on the struct
    /// for convenience and to make the dependency explicit.
    ///
    /// [`KeychainRef::for_credential`]: crate::KeychainRef::for_credential
    pub keychain_ref: crate::id::KeychainRef,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Credential {
    /// Build a new credential record. The secret itself is NOT taken
    /// here — storage layer stores it in keychain in a separate call.
    /// Splitting construction from secret-handling keeps `rh-core`
    /// I/O-free.
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        kind: CredentialKind,
        username: impl Into<String>,
    ) -> Self {
        let id = CredentialId::new();
        let keychain_ref = crate::id::KeychainRef::for_credential(&id);
        let now = Utc::now();
        Self {
            id,
            name: name.into(),
            kind,
            username: username.into(),
            keychain_ref,
            created_at: now,
            updated_at: now,
        }
    }

    pub fn touch(&mut self) {
        self.updated_at = Utc::now();
    }
}

/// A persisted port-forward definition (Tools → Forwards). Bound to a
/// saved SSH host (whose credentials + one-level ProxyJump the forward
/// reuses). Field meaning depends on [`kind`](ForwardKind):
/// * `Local`  — `bind_*` = local listen, `target_*` = host reachable from the remote.
/// * `Remote` — `bind_*` = server listen, `target_*` = host reachable from us.
/// * `Dynamic`— `bind_*` = local SOCKS5; `target_*` unused (per-connection).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedForward {
    pub id: ForwardId,
    pub host_id: HostId,
    pub kind: ForwardKind,
    pub bind_host: String,
    pub bind_port: u16,
    pub target_host: String,
    pub target_port: u16,
    /// Start this forward automatically when the app launches.
    pub auto_start: bool,
    pub created_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_default_ports_are_iana() {
        assert_eq!(Protocol::Ssh.default_port(), 22);
        assert_eq!(Protocol::Rdp.default_port(), 3389);
    }

    #[test]
    fn protocol_serde_is_lowercase() {
        let json = serde_json::to_string(&Protocol::Ssh).unwrap();
        assert_eq!(json, "\"ssh\"");
        let back: Protocol = serde_json::from_str("\"rdp\"").unwrap();
        assert_eq!(back, Protocol::Rdp);
    }

    #[test]
    fn credential_kind_serde_is_snake_case() {
        let json = serde_json::to_string(&CredentialKind::SshKey).unwrap();
        assert_eq!(json, "\"ssh_key\"");
        let json2 = serde_json::to_string(&CredentialKind::SshKeyAgent).unwrap();
        assert_eq!(json2, "\"ssh_key_agent\"");
    }

    #[test]
    fn credential_kind_keychain_requirement() {
        assert!(CredentialKind::Password.requires_keychain_secret());
        assert!(CredentialKind::SshKey.requires_keychain_secret());
        assert!(!CredentialKind::SshKeyAgent.requires_keychain_secret());
    }

    #[test]
    fn host_new_uses_protocol_default_port() {
        let host = Host::new("test", "example.com", Protocol::Ssh, None);
        assert_eq!(host.port, 22);

        let host = Host::new("test", "example.com", Protocol::Rdp, None);
        assert_eq!(host.port, 3389);
    }

    #[test]
    fn host_new_respects_explicit_port() {
        let host = Host::new("test", "example.com", Protocol::Ssh, Some(2222));
        assert_eq!(host.port, 2222);
    }

    #[test]
    fn host_new_stamps_timestamps_equally() {
        let host = Host::new("test", "example.com", Protocol::Ssh, None);
        assert_eq!(host.created_at, host.updated_at);
    }

    #[test]
    fn host_touch_advances_updated_at() {
        let mut host = Host::new("test", "example.com", Protocol::Ssh, None);
        let original_updated = host.updated_at;
        let original_created = host.created_at;
        std::thread::sleep(std::time::Duration::from_millis(2));
        host.touch();
        assert!(host.updated_at > original_updated);
        assert_eq!(host.created_at, original_created, "created_at should not change");
    }

    #[test]
    fn host_serde_roundtrip() {
        let host = Host::new("prod-db-01", "db.example.com", Protocol::Ssh, Some(22));
        let json = serde_json::to_string(&host).unwrap();
        let back: Host = serde_json::from_str(&json).unwrap();
        assert_eq!(host, back);
    }

    #[test]
    fn host_new_defaults_stage18_fields() {
        let host = Host::new("test", "example.com", Protocol::Ssh, None);
        assert!(host.display_name.is_none());
        assert!(host.startup_command.is_none());
        assert!(host.env_vars.is_empty());
        assert!(host.detected_os.is_none());
    }

    #[test]
    fn host_serde_roundtrip_with_stage18_fields() {
        let mut host = Host::new("prod", "db.example.com", Protocol::Ssh, Some(22));
        host.display_name = Some("Prod DB".into());
        host.startup_command = Some("tmux attach || tmux".into());
        host.env_vars = vec![
            EnvVar::new("LANG", "en_US.UTF-8"),
            EnvVar::new("TERM", "xterm-256color"),
        ];
        host.detected_os = Some("ubuntu".into());
        let json = serde_json::to_string(&host).unwrap();
        let back: Host = serde_json::from_str(&json).unwrap();
        assert_eq!(host, back);
    }

    #[test]
    fn env_var_serde_shape() {
        let ev = EnvVar::new("KEY", "VAL");
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["key"], "KEY");
        assert_eq!(json["value"], "VAL");
    }

    #[test]
    fn credential_new_populates_keychain_ref_from_id() {
        let cred = Credential::new("test-cred", CredentialKind::Password, "alice");
        let expected = crate::id::KeychainRef::for_credential(&cred.id);
        assert_eq!(cred.keychain_ref, expected);
    }

    #[test]
    fn credential_serde_roundtrip() {
        let cred = Credential::new("test", CredentialKind::SshKey, "alice");
        let json = serde_json::to_string(&cred).unwrap();
        let back: Credential = serde_json::from_str(&json).unwrap();
        assert_eq!(cred, back);
    }

    #[test]
    fn host_group_new_no_parent() {
        let group = HostGroup::new("Prod", None);
        assert_eq!(group.name, "Prod");
        assert!(group.parent_id.is_none());
    }

    #[test]
    fn host_group_new_with_parent() {
        let parent = HostGroup::new("Servers", None);
        let child = HostGroup::new("Prod", Some(parent.id.clone()));
        assert_eq!(child.parent_id, Some(parent.id));
    }
}
