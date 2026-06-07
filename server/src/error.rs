//! HTTP error type. Maps to status codes + a small JSON `{ "error": ... }`
//! body. Internal causes (sqlx, crypto) are logged but never leaked to the
//! client.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{0}")]
    BadRequest(String),
    #[error("unauthorized")]
    Unauthorized,
    #[error("{0}")]
    Forbidden(String),
    #[error("conflict: remote changed since last pull")]
    Conflict,
    #[error("vault already exists")]
    PreconditionFailed,
    #[error("payload too large")]
    TooLarge,
    #[error("internal error")]
    Internal,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = match self {
            AppError::BadRequest(_) => StatusCode::BAD_REQUEST,
            AppError::Unauthorized => StatusCode::UNAUTHORIZED,
            AppError::Forbidden(_) => StatusCode::FORBIDDEN,
            AppError::Conflict => StatusCode::CONFLICT,
            AppError::PreconditionFailed => StatusCode::PRECONDITION_FAILED,
            AppError::TooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            AppError::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, Json(json!({ "error": self.to_string() }))).into_response()
    }
}

impl From<sqlx::Error> for AppError {
    fn from(e: sqlx::Error) -> Self {
        tracing::error!("database error: {e}");
        AppError::Internal
    }
}
