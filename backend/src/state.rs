//! Shared application state passed to every axum handler.

use std::sync::Arc;

/// Cloneable handle to all shared runtime resources.
#[derive(Clone)]
pub struct AppState {
    pub db: sqlx::SqlitePool,
    pub cfg: Arc<crate::config::Config>,
    /// Broadcasts JSON status events (repo status changes, scheduler ticks, etc.)
    /// to any subscribed `/ws/status` clients.
    pub status_tx: tokio::sync::broadcast::Sender<String>,
    /// Per-install signing keys for session + confirmation tokens (P18/P19).
    pub auth: Arc<crate::auth_session::AuthKeys>,
}
