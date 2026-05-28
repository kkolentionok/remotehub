//! IPC-facing error type.
//!
//! All Tauri commands return `Result<T, ApiError>`. On the UI side
//! this becomes a rejected promise; the `kind` field discriminates
//! the case. Internal details (sqlx errors, OS keychain errors,
//! backtraces) are NOT forwarded to the UI — they're logged via
//! `tracing` and the UI sees a sanitized message.
//!
//! Why a custom error type instead of stringifying `CoreError`:
//! UI needs to make decisions ("this was an auth error, prompt for
//! credentials" vs. "this was a storage error, show a generic
//! retry") and a discriminated union is the typed contract for
//! that. The `kind` tag is stable across releases; the human-readable
//! parts may evolve.

use serde::Serialize;
use thiserror::Error;
use tracing::warn;

use rh_core::{CoreError, RevealError, SecretError, SessionError, StorageError};

/// Error returned to the UI. Serialized as a tagged JSON object.
///
/// Example:
/// ```json
/// { "kind": "not_found",  "entity": "host" }
/// { "kind": "validation", "field": "hostname", "reason": "must not be empty" }
/// { "kind": "conflict",   "message": "name already taken" }
/// ```
#[derive(Debug, Serialize, Error)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ApiError {
    #[error("not found: {entity}")]
    NotFound { entity: String },

    #[error("validation: {field}: {reason}")]
    Validation { field: String, reason: String },

    #[error("storage error: {message}")]
    Storage { message: String },

    #[error("secret store error: {message}")]
    Secret { message: String },

    #[error("session error: {message}")]
    Session { message: String },

    #[error("conflict: {message}")]
    Conflict { message: String },

    /// Generic "something went wrong". The detailed cause is logged
    /// but not exposed to the UI; the user sees just a friendly
    /// message and is encouraged to retry / check logs.
    ///
    /// Currently unused — Stage 1.4 routes everything through more
    /// specific variants. Reserved for catch-all cases in later stages
    /// (e.g. session crashes, unexpected actor panics).
    #[allow(dead_code)]
    #[error("internal error: {message}")]
    Internal { message: String },

    /// A feature is recognized by Tauri but not yet implemented in
    /// this build. Used by Stage 1.4 to stub session commands.
    #[error("not implemented: {feature}")]
    NotImplemented { feature: String },
}

impl ApiError {
    /// Helper for validation failures so call sites stay short.
    pub fn validation(field: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::Validation {
            field: field.into(),
            reason: reason.into(),
        }
    }

    pub fn not_found(entity: impl Into<String>) -> Self {
        Self::NotFound {
            entity: entity.into(),
        }
    }

    pub fn not_implemented(feature: impl Into<String>) -> Self {
        Self::NotImplemented {
            feature: feature.into(),
        }
    }
}

// =====================================================================
// Conversions from domain errors. Each one logs at an appropriate level
// before stripping internal details — that way, the user gets a clean
// message and we still have full context in the trace file.
// =====================================================================

impl From<StorageError> for ApiError {
    fn from(err: StorageError) -> Self {
        match err {
            StorageError::Conflict(msg) => ApiError::Conflict { message: msg },
            StorageError::ForeignKey(msg) => ApiError::Validation {
                field: "reference".to_string(),
                reason: msg,
            },
            other => {
                warn!(error = ?other, "storage error surfaced to UI");
                ApiError::Storage {
                    message: other.to_string(),
                }
            }
        }
    }
}

impl From<SecretError> for ApiError {
    fn from(err: SecretError) -> Self {
        warn!(error = ?err, "secret error surfaced to UI");
        ApiError::Secret {
            message: err.to_string(),
        }
    }
}

impl From<SessionError> for ApiError {
    fn from(err: SessionError) -> Self {
        warn!(error = ?err, "session error surfaced to UI");
        ApiError::Session {
            message: err.to_string(),
        }
    }
}

impl From<RevealError> for ApiError {
    fn from(err: RevealError) -> Self {
        match err {
            RevealError::Storage(e) => e.into(),
            RevealError::Secret(e) => e.into(),
        }
    }
}

impl From<CoreError> for ApiError {
    fn from(err: CoreError) -> Self {
        match err {
            CoreError::HostNotFound(_) => ApiError::not_found("host"),
            CoreError::CredentialNotFound(_) => ApiError::not_found("credential"),
            CoreError::GroupNotFound(_) => ApiError::not_found("group"),
            CoreError::Validation { field, reason } => ApiError::Validation {
                field: field.to_string(),
                reason,
            },
            CoreError::Storage(e) => e.into(),
            CoreError::Secret(e) => e.into(),
            CoreError::Session(e) => e.into(),
        }
    }
}

/// Convenience alias for command handler return types.
pub type ApiResult<T> = Result<T, ApiError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialization_uses_tag_kind() {
        let e = ApiError::NotFound {
            entity: "host".into(),
        };
        let json = serde_json::to_string(&e).unwrap();
        assert!(json.contains(r#""kind":"not_found""#));
        assert!(json.contains(r#""entity":"host""#));
    }

    #[test]
    fn validation_serializes_field_and_reason() {
        let e = ApiError::Validation {
            field: "hostname".into(),
            reason: "must not be empty".into(),
        };
        let v: serde_json::Value = serde_json::to_value(&e).unwrap();
        assert_eq!(v["kind"], "validation");
        assert_eq!(v["field"], "hostname");
        assert_eq!(v["reason"], "must not be empty");
    }

    #[test]
    fn storage_conflict_maps_to_api_conflict() {
        let e: ApiError = StorageError::Conflict("name taken".into()).into();
        assert!(matches!(e, ApiError::Conflict { .. }));
    }

    #[test]
    fn storage_foreign_key_maps_to_validation() {
        let e: ApiError = StorageError::ForeignKey("fk: group missing".into()).into();
        assert!(matches!(e, ApiError::Validation { .. }));
    }

    #[test]
    fn other_storage_errors_map_to_storage_variant() {
        let e: ApiError = StorageError::Io("disk full".into()).into();
        assert!(matches!(e, ApiError::Storage { .. }));
    }

    #[test]
    fn core_not_found_uses_singular_entity_name() {
        let e: ApiError = CoreError::HostNotFound(rh_core::HostId::from_raw("x")).into();
        match e {
            ApiError::NotFound { entity } => assert_eq!(entity, "host"),
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn reveal_error_dispatches_to_storage_or_secret() {
        let e: ApiError = RevealError::Secret(SecretError::NotFound).into();
        assert!(matches!(e, ApiError::Secret { .. }));

        let e: ApiError = RevealError::Storage(StorageError::Io("x".into())).into();
        assert!(matches!(e, ApiError::Storage { .. }));
    }
}
