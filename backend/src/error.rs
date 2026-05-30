//! Typed application error + axum response mapping.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

/// Convenience result alias used by API handlers.
pub type ApiResult<T> = Result<T, AppError>;

/// Application-level error. All variants surface as HTTP 500 with a JSON body
/// `{"error": "<message>"}`.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{0}")]
    Anyhow(#[from] anyhow::Error),

    #[error("database error: {0}")]
    Sqlx(#[from] sqlx::Error),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("{0}")]
    Message(String),

    /// 401 — authentication required / failed. Body `{"error": "<message>"}`.
    #[error("{0}")]
    Unauthorized(String),

    /// 403 — authenticated but not permitted. Body `{"error": "<message>"}`.
    #[error("{0}")]
    Forbidden(String),

    /// 409 — confirmation/precondition required. Body is the carried JSON value
    /// verbatim (e.g. `{"error":"confirmation required","confirm_token":..}`).
    #[error("conflict")]
    Conflict(serde_json::Value),
}

impl AppError {
    pub fn msg(s: impl Into<String>) -> Self {
        AppError::Message(s.into())
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        match self {
            AppError::Unauthorized(msg) => {
                tracing::warn!(error = %msg, "unauthorized");
                (StatusCode::UNAUTHORIZED, Json(json!({ "error": msg }))).into_response()
            }
            AppError::Forbidden(msg) => {
                tracing::warn!(error = %msg, "forbidden");
                (StatusCode::FORBIDDEN, Json(json!({ "error": msg }))).into_response()
            }
            // 409 carries an arbitrary JSON body verbatim (no `{"error":..}` wrap).
            AppError::Conflict(body) => (StatusCode::CONFLICT, Json(body)).into_response(),
            other => {
                let msg = other.to_string();
                tracing::error!(error = %msg, "request failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "error": msg })),
                )
                    .into_response()
            }
        }
    }
}
