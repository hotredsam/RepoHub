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
}

impl AppError {
    pub fn msg(s: impl Into<String>) -> Self {
        AppError::Message(s.into())
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let msg = self.to_string();
        tracing::error!(error = %msg, "request failed");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": msg })),
        )
            .into_response()
    }
}
