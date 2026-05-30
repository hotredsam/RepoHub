//! HTTP routes. P0 exposes only /api/health; later phases mount more.

use axum::{routing::get, Json, Router};
use serde_json::json;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

use crate::config::Config;

pub fn router(cfg: Config) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .layer(TraceLayer::new_for_http())
        // Local dev: the Vite frontend (5173) calls the API (8787).
        .layer(CorsLayer::permissive())
        .with_state(cfg)
}

async fn health() -> Json<serde_json::Value> {
    Json(json!({
        "ok": true,
        "service": "repohub",
        "version": env!("CARGO_PKG_VERSION"),
    }))
}
