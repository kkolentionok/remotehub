//! SSH-layer errors. Mapped to [`crate::CloseReason`] by the actor
//! before reporting to the UI.

use crate::CloseReason;

#[derive(Debug, thiserror::Error)]
pub enum SshError {
    #[error("network error: {0}")]
    Network(#[from] std::io::Error),

    #[error("russh error: {0}")]
    Russh(#[from] russh::Error),

    #[error("authentication failed ({method})")]
    AuthFailed { method: String },

    #[error("ssh-key authentication is not supported yet")]
    KeyAuthUnsupported,

    #[error("channel closed")]
    ChannelClosed,
}

impl SshError {
    /// Collapse into the UI-facing close reason.
    pub fn into_close_reason(self) -> CloseReason {
        match self {
            SshError::Network(e) => CloseReason::NetworkError {
                message: e.to_string(),
            },
            SshError::Russh(e) => CloseReason::NetworkError {
                message: e.to_string(),
            },
            SshError::AuthFailed { .. } | SshError::KeyAuthUnsupported => {
                CloseReason::AuthFailed
            }
            SshError::ChannelClosed => CloseReason::ServerDisconnected { message: None },
        }
    }
}
