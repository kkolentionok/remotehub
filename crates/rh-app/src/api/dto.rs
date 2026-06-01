//! Data Transfer Objects for the Tauri command surface.
//!
//! These are deliberately separate from `rh_core` domain types:
//!
//! - DTOs may omit sensitive fields (e.g. credentials never serialize
//!   their `keychain_ref` — UI has no business knowing).
//! - DTOs may shape differently from domain types when that matches
//!   UI ergonomics better (e.g. timestamps as RFC 3339 strings, not
//!   `chrono::DateTime` which serde-flattens as a different shape on
//!   each chrono major version).
//! - DTO evolution is decoupled from domain evolution: the latter is
//!   internal, the former is a wire contract.

use serde::{Deserialize, Serialize};

use rh_core::{
    Credential, CredentialId, CredentialKind, EnvVar, GroupId, Host, HostGroup, HostId, Protocol,
};

// =====================================================================
// Hosts
// =====================================================================

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostListRequest {
    #[serde(default)]
    pub group_id: Option<GroupId>,
    #[serde(default)]
    pub protocol: Option<Protocol>,
    #[serde(default)]
    pub search: Option<String>,
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Debug, Serialize)]
pub struct HostListResponse {
    pub hosts: Vec<HostDto>,
    pub total: u32,
}

/// Hosts as exposed in list responses. `notes` is omitted to keep
/// list payloads small; use `host_get` for the full record. `display_name`
/// and `detected_os` are kept here (not just in the full DTO) because the
/// sidebar renders the label and the host icon from the list payload.
#[derive(Debug, Serialize)]
pub struct HostDto {
    pub id: HostId,
    pub name: String,
    pub display_name: Option<String>,
    pub group_id: Option<GroupId>,
    pub protocol: Protocol,
    pub hostname: String,
    pub port: u16,
    pub username: String,
    pub tags: Vec<String>,
    pub color: Option<String>,
    pub detected_os: Option<String>,
    pub default_credential_id: Option<CredentialId>,
    /// Optional bastion (ProxyJump): another saved host to route through.
    pub jump_host_id: Option<HostId>,
    pub agent_forwarding: bool,
    /// RFC 3339, or `null` if never connected. Stamped on first Ready.
    pub last_connected_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl From<Host> for HostDto {
    fn from(h: Host) -> Self {
        Self {
            id: h.id,
            name: h.name,
            display_name: h.display_name,
            group_id: h.group_id,
            protocol: h.protocol,
            hostname: h.hostname,
            port: h.port,
            username: h.username,
            tags: h.tags,
            color: h.color,
            detected_os: h.detected_os,
            default_credential_id: h.default_credential_id,
            jump_host_id: h.jump_host_id,
            agent_forwarding: h.agent_forwarding,
            last_connected_at: h.last_connected_at.map(|d| d.to_rfc3339()),
            created_at: h.created_at.to_rfc3339(),
            updated_at: h.updated_at.to_rfc3339(),
        }
    }
}

/// Full host record including `notes`, `startup_command` and `env_vars`.
/// Returned by `host_get` — these heavier fields are only needed in the
/// detail pane, not in list views.
#[derive(Debug, Serialize)]
pub struct HostFullDto {
    #[serde(flatten)]
    pub base: HostDto,
    pub notes: Option<String>,
    pub startup_command: Option<String>,
    pub env_vars: Vec<EnvVar>,
    /// All credentials linked to this host (default first). Lets the UI
    /// render every active auth method. Populated by `host_get`; empty
    /// from the `From<Host>` conversion (which has no store access).
    pub credential_ids: Vec<CredentialId>,
}

impl From<Host> for HostFullDto {
    fn from(h: Host) -> Self {
        let notes = h.notes.clone();
        let startup_command = h.startup_command.clone();
        let env_vars = h.env_vars.clone();
        Self {
            base: HostDto::from(h),
            notes,
            startup_command,
            env_vars,
            credential_ids: Vec::new(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostCreateRequest {
    pub name: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub group_id: Option<GroupId>,
    pub protocol: Protocol,
    pub hostname: String,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub startup_command: Option<String>,
    #[serde(default)]
    pub env_vars: Option<Vec<EnvVar>>,
    #[serde(default)]
    pub default_credential_id: Option<CredentialId>,
    #[serde(default)]
    pub jump_host_id: Option<HostId>,
    #[serde(default)]
    pub agent_forwarding: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct HostCreateResponse {
    pub id: HostId,
}

/// Partial host update.
///
/// The `OptionalField<T>` pattern handles the difference between
/// "field not present in request" (don't touch) and "field is null"
/// (clear it). We model this with `serde_with::skip_serializing_none`
/// elsewhere; here we use double `Option` and a custom helper.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostUpdateRequest {
    pub id: HostId,
    #[serde(default)]
    pub name: Option<String>,
    /// Double Option: `None` = not in request, `Some(None)` = clear label.
    #[serde(default, deserialize_with = "deserialize_optional_optional")]
    pub display_name: Option<Option<String>>,
    /// Double Option: `None` = not in request, `Some(None)` = set to NULL.
    #[serde(default, deserialize_with = "deserialize_optional_optional")]
    pub group_id: Option<Option<GroupId>>,
    #[serde(default)]
    pub protocol: Option<Protocol>,
    #[serde(default)]
    pub hostname: Option<String>,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    #[serde(default, deserialize_with = "deserialize_optional_optional")]
    pub color: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_optional_optional")]
    pub notes: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_optional_optional")]
    pub startup_command: Option<Option<String>>,
    /// Full-replace list (like `tags`): present = set, absent = leave.
    /// An empty array clears all environment variables.
    #[serde(default)]
    pub env_vars: Option<Vec<EnvVar>>,
    #[serde(default, deserialize_with = "deserialize_optional_optional")]
    pub default_credential_id: Option<Option<CredentialId>>,
    #[serde(default, deserialize_with = "deserialize_optional_optional")]
    pub jump_host_id: Option<Option<HostId>>,
    #[serde(default)]
    pub agent_forwarding: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostIdRequest {
    pub id: HostId,
}

// =====================================================================
// Host groups
// =====================================================================

#[derive(Debug, Serialize)]
pub struct HostGroupDto {
    pub id: GroupId,
    pub name: String,
    pub parent_id: Option<GroupId>,
    pub created_at: String,
}

impl From<HostGroup> for HostGroupDto {
    fn from(g: HostGroup) -> Self {
        Self {
            id: g.id,
            name: g.name,
            parent_id: g.parent_id,
            created_at: g.created_at.to_rfc3339(),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct GroupListResponse {
    pub groups: Vec<HostGroupDto>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GroupCreateRequest {
    pub name: String,
    #[serde(default)]
    pub parent_id: Option<GroupId>,
}

#[derive(Debug, Serialize)]
pub struct GroupCreateResponse {
    pub id: GroupId,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GroupRenameRequest {
    pub id: GroupId,
    pub name: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GroupMoveRequest {
    pub id: GroupId,
    /// `None` = move to root.
    #[serde(default)]
    pub parent_id: Option<GroupId>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GroupIdRequest {
    pub id: GroupId,
}

// =====================================================================
// Credentials
// =====================================================================

/// Credential metadata exposed to UI. Never includes the keychain ref —
/// the UI has no business knowing where secrets live.
#[derive(Debug, Serialize)]
pub struct CredentialDto {
    pub id: CredentialId,
    pub name: String,
    pub kind: CredentialKind,
    pub username: String,
    pub created_at: String,
    pub updated_at: String,
}

impl From<Credential> for CredentialDto {
    fn from(c: Credential) -> Self {
        Self {
            id: c.id,
            name: c.name,
            kind: c.kind,
            username: c.username,
            created_at: c.created_at.to_rfc3339(),
            updated_at: c.updated_at.to_rfc3339(),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct CredentialListResponse {
    pub credentials: Vec<CredentialDto>,
}

/// Secret bytes arriving from the UI. Base64-encoded text on the wire
/// to keep IPC payloads textual. On receive, command handlers decode
/// and immediately wrap into `SecretValue` (zeroize-on-drop).
#[derive(Debug, Deserialize)]
#[serde(transparent)]
pub struct SecretInput(pub String);

impl SecretInput {
    /// Decode the base64-wrapped bytes into a [`SecretValue`].
    /// Validation: non-empty when required happens at the call site.
    pub fn decode(&self) -> Result<rh_core::SecretValue, base64::DecodeError> {
        use base64::Engine;
        let bytes = base64::engine::general_purpose::STANDARD.decode(self.0.as_bytes())?;
        Ok(rh_core::SecretValue::new(bytes))
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CredentialCreateRequest {
    pub name: String,
    pub kind: CredentialKind,
    pub username: String,
    /// None only for kind=SshKeyAgent.
    #[serde(default)]
    pub secret: Option<SecretInput>,
    /// Only used when kind=SshKey with encrypted private key.
    #[serde(default)]
    pub passphrase: Option<SecretInput>,
}

#[derive(Debug, Serialize)]
pub struct CredentialCreateResponse {
    pub id: CredentialId,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CredentialUpdateRequest {
    pub id: CredentialId,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub username: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CredentialRotateSecretRequest {
    pub id: CredentialId,
    pub secret: SecretInput,
    #[serde(default)]
    pub passphrase: Option<SecretInput>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CredentialIdRequest {
    pub id: CredentialId,
}

/// Response for `credential_reveal`. `secret` is the UTF-8 decoding of
/// the stored bytes; `None` for ssh_key_agent kind. The UI is responsible
/// for not persisting this value beyond a short reveal window.
#[derive(Debug, Serialize)]
pub struct CredentialRevealResponse {
    pub kind: CredentialKind,
    pub username: String,
    pub secret: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CredentialLinkRequest {
    pub host_id: HostId,
    pub credential_id: CredentialId,
    #[serde(default)]
    pub set_as_default: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CredentialUnlinkRequest {
    pub host_id: HostId,
    pub credential_id: CredentialId,
}

// =====================================================================
// Sessions (Stage 1.4: stub commands)
// =====================================================================
//
// Fields here are deserialized by serde and then ignored by the stub
// command handlers (which immediately return `NotImplemented`). When
// real session actors arrive in Stage 2 (SSH) and Stage 4 (RDP),
// every field will be consumed; until then we silence dead_code so
// the warning list stays signal-only.

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionOpenRequest {
    pub host_id: HostId,
    #[serde(default)]
    pub credential_id: Option<CredentialId>,
    pub options: SessionOpenOptions,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(tag = "protocol", rename_all = "lowercase")]
pub enum SessionOpenOptions {
    Ssh {
        cols: u16,
        rows: u16,
        #[serde(default = "default_term")]
        term: String,
    },
    Rdp {
        width: u16,
        height: u16,
        color_depth: u8,
        keyboard_layout: String,
    },
}

fn default_term() -> String {
    "xterm-256color".to_string()
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionIdRequest {
    pub session_id: rh_core::SessionId,
}

#[derive(Debug, Serialize)]
pub struct SessionOpenResponse {
    pub session_id: rh_core::SessionId,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionInputRequest {
    pub session_id: rh_core::SessionId,
    pub data: Vec<u8>,
}

/// Open a local shell PTY at an initial terminal size (character cells).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalSessionOpenRequest {
    pub cols: u16,
    pub rows: u16,
}

/// Open an SFTP connection to a host.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SftpOpenRequest {
    pub host_id: rh_core::HostId,
}

/// List a remote directory on an open SFTP session. Empty/"." path = home.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SftpListRequest {
    pub session_id: rh_core::SessionId,
    pub path: String,
}

/// Download a remote file into a local directory (keeps the file name).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SftpDownloadRequest {
    pub session_id: rh_core::SessionId,
    pub remote_path: String,
    pub local_dir: String,
}

/// Upload a local file into a remote directory (keeps the file name).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SftpUploadRequest {
    pub session_id: rh_core::SessionId,
    pub local_path: String,
    pub remote_dir: String,
}

/// Copy a file directly between two open SFTP sessions (streamed through
/// the app — SFTP has no server-to-server copy).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SftpCopyRequest {
    pub from_session: rh_core::SessionId,
    pub remote_path: String,
    pub to_session: rh_core::SessionId,
    pub remote_dir: String,
}

/// Rename a remote entry in place (same directory).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SftpRenameRequest {
    pub session_id: rh_core::SessionId,
    pub path: String,
    pub new_name: String,
}

/// Delete a remote file or directory (directories recursively).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SftpRemoveRequest {
    pub session_id: rh_core::SessionId,
    pub path: String,
    pub is_dir: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SftpTransferKind {
    Download,
    Upload,
    Copy,
}

/// A single streamed transfer with byte-progress events on a `Channel`.
/// `session_id` is the remote endpoint (download source / upload dest /
/// copy source); `to_session` is the copy destination.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SftpTransferRequest {
    pub transfer_id: String,
    pub kind: SftpTransferKind,
    pub session_id: rh_core::SessionId,
    pub to_session: Option<rh_core::SessionId>,
    /// Source file: remote path for download/copy, local path for upload.
    pub src_path: String,
    /// Destination directory: local for download, remote for upload/copy.
    pub dst_dir: String,
    /// Optional destination filename (used for "keep both" conflict resolution);
    /// defaults to the source basename.
    pub dst_name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SftpTransferCancelRequest {
    pub transfer_id: String,
}

/// Create a directory `name` inside remote `parent`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SftpMkdirRequest {
    pub session_id: rh_core::SessionId,
    pub parent: String,
    pub name: String,
}

/// RDP input event from the UI. `event` is the tagged `RdpInputEvent`
/// union (mouse/keyboard/modifier-sync) deserialized by `rh-rdp`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RdpInputRequest {
    pub session_id: rh_core::SessionId,
    pub event: rh_rdp::RdpInputEvent,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionResizeRequest {
    pub session_id: rh_core::SessionId,
    pub width: u16,
    pub height: u16,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionAcceptHostKeyRequest {
    pub session_id: rh_core::SessionId,
    #[allow(dead_code)]
    pub fingerprint: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionReattachRequest {
    pub session_id: rh_core::SessionId,
}

/// One live session, for restore-on-reload. `opened_at` is an RFC 3339
/// string for a stable wire shape across chrono versions.
#[derive(Debug, Serialize)]
pub struct SessionSummaryDto {
    pub session_id: rh_core::SessionId,
    pub host_id: HostId,
    pub hostname: String,
    pub title: String,
    pub protocol: Protocol,
    pub state: rh_ssh::SessionState,
    pub opened_at: String,
}

#[derive(Debug, Serialize)]
pub struct SessionListResponse {
    pub sessions: Vec<SessionSummaryDto>,
}

/// Pinned host key for a host, for the technical-info panel.
#[derive(Debug, Serialize)]
pub struct KnownHostKeyDto {
    pub key_type: String,
    pub fingerprint_sha256: String,
}

#[derive(Debug, Serialize)]
pub struct KnownHostGetResponse {
    /// `None` if no key has been pinned for this host yet.
    pub key: Option<KnownHostKeyDto>,
}

/// One pinned host key, for the management list.
#[derive(Debug, Serialize)]
pub struct KnownHostEntryDto {
    pub hostname: String,
    pub port: u16,
    pub key_type: String,
    pub fingerprint_sha256: String,
    pub created_at: String,
}

#[derive(Debug, Serialize)]
pub struct KnownHostsListResponse {
    pub entries: Vec<KnownHostEntryDto>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KnownHostForgetRequest {
    pub hostname: String,
    pub port: u16,
}

/// One trusted RDP server cert, for the management list.
#[derive(Debug, Serialize)]
pub struct RdpCertEntryDto {
    pub hostname: String,
    pub port: u16,
    pub fingerprint_sha256: String,
    pub subject: String,
    pub trusted_at: String,
}

#[derive(Debug, Serialize)]
pub struct RdpCertsListResponse {
    pub entries: Vec<RdpCertEntryDto>,
}

// =====================================================================
// Settings
// =====================================================================

#[derive(Debug, Serialize)]
pub struct SettingsGetAllResponse {
    pub settings: rh_core::Settings,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsUpdateRequest {
    pub patches: serde_json::Value,
}

// =====================================================================
// App meta
// =====================================================================

#[derive(Debug, Serialize)]
pub struct AppVersionResponse {
    pub version: String,
    pub target: String,
}

// =====================================================================
// Helpers
// =====================================================================

/// Deserialize a field that may be omitted (→ `None`) OR explicitly
/// null (→ `Some(None)`) OR present (→ `Some(Some(value))`).
///
/// This is the only way to express "PATCH semantics": clearing a
/// field needs to be distinguishable from leaving it alone.
fn deserialize_optional_optional<'de, D, T>(
    deserializer: D,
) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_create_request_accepts_minimal() {
        let json = r#"{
            "name": "test",
            "protocol": "ssh",
            "hostname": "example.com"
        }"#;
        let req: HostCreateRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.name, "test");
        assert_eq!(req.protocol, Protocol::Ssh);
        assert!(req.port.is_none());
        assert!(req.tags.is_none());
    }

    #[test]
    fn host_create_request_rejects_unknown_fields() {
        let json = r#"{
            "name": "test",
            "protocol": "ssh",
            "hostname": "example.com",
            "rogue_field": "evil"
        }"#;
        let result: Result<HostCreateRequest, _> = serde_json::from_str(json);
        assert!(result.is_err(), "should reject unknown fields");
    }

    #[test]
    fn host_update_distinguishes_absent_from_null() {
        // Field absent from JSON → top-level None.
        let json_absent = r#"{ "id": "abc" }"#;
        let req: HostUpdateRequest = serde_json::from_str(json_absent).unwrap();
        assert!(req.group_id.is_none());

        // Field explicitly null → Some(None) = "clear it".
        let json_null = r#"{ "id": "abc", "group_id": null }"#;
        let req: HostUpdateRequest = serde_json::from_str(json_null).unwrap();
        assert_eq!(req.group_id, Some(None));

        // Field with value → Some(Some(value)).
        let json_value = r#"{ "id": "abc", "group_id": "grp-1" }"#;
        let req: HostUpdateRequest = serde_json::from_str(json_value).unwrap();
        assert_eq!(req.group_id, Some(Some(GroupId::from_raw("grp-1"))));
    }

    #[test]
    fn secret_input_decodes_base64() {
        let s = SecretInput("aHVudGVyMg==".to_string()); // "hunter2"
        let decoded = s.decode().unwrap();
        assert_eq!(decoded.expose(), b"hunter2");
    }

    #[test]
    fn host_dto_omits_notes() {
        let h = Host::new("t", "x.example.com", Protocol::Ssh, None);
        let dto: HostDto = h.into();
        let json = serde_json::to_value(&dto).unwrap();
        assert!(!json.as_object().unwrap().contains_key("notes"));
    }

    #[test]
    fn host_full_dto_includes_notes() {
        let mut h = Host::new("t", "x.example.com", Protocol::Ssh, None);
        h.notes = Some("a note".into());
        h.startup_command = Some("tmux".into());
        h.env_vars = vec![EnvVar::new("LANG", "C")];
        let dto: HostFullDto = h.into();
        let json = serde_json::to_value(&dto).unwrap();
        assert_eq!(json["notes"], "a note");
        assert_eq!(json["startup_command"], "tmux");
        assert_eq!(json["env_vars"][0]["key"], "LANG");
        // And the flattened base fields are still there.
        assert_eq!(json["name"], "t");
    }

    #[test]
    fn host_update_env_vars_is_full_replace_list() {
        // Absent → None (leave alone).
        let req: HostUpdateRequest = serde_json::from_str(r#"{ "id": "abc" }"#).unwrap();
        assert!(req.env_vars.is_none());
        // Empty array → Some(vec![]) (clear all).
        let req: HostUpdateRequest =
            serde_json::from_str(r#"{ "id": "abc", "env_vars": [] }"#).unwrap();
        assert_eq!(req.env_vars, Some(vec![]));
        // display_name distinguishes absent / null / value.
        let req: HostUpdateRequest =
            serde_json::from_str(r#"{ "id": "abc", "display_name": null }"#).unwrap();
        assert_eq!(req.display_name, Some(None));
        let req: HostUpdateRequest =
            serde_json::from_str(r#"{ "id": "abc", "display_name": "Pretty" }"#).unwrap();
        assert_eq!(req.display_name, Some(Some("Pretty".to_string())));
    }

    #[test]
    fn credential_dto_omits_keychain_ref() {
        let c = Credential::new("c", CredentialKind::Password, "alice");
        let dto: CredentialDto = c.into();
        let json = serde_json::to_value(&dto).unwrap();
        assert!(!json.as_object().unwrap().contains_key("keychain_ref"));
    }

    #[test]
    fn session_open_options_tagged_correctly() {
        let json = r#"{ "protocol": "ssh", "cols": 80, "rows": 24 }"#;
        let opts: SessionOpenOptions = serde_json::from_str(json).unwrap();
        match opts {
            SessionOpenOptions::Ssh { cols, rows, term } => {
                assert_eq!(cols, 80);
                assert_eq!(rows, 24);
                assert_eq!(term, "xterm-256color"); // default
            }
            _ => panic!("wrong variant"),
        }
    }
}
