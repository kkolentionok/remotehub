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

use crate::id::{CredentialId, GroupId, HostId};

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
