//! RepoHub backend — local-first multi-repo command center.
//!
//! Boots an axum server on `127.0.0.1`, wires up the SQLite database, spawns the
//! background fetch scheduler, and merges every feature router under one app.

mod bulk;
mod claude_api;
mod claude_runner;
mod config;
mod db;
mod error;
mod github;
mod gitops;
mod infra;
mod models;
mod prompts_api;
mod repos;
mod scheduler;
mod settings_api;
mod state;
mod transcripts;
mod ws_status;

use std::net::SocketAddr;
use std::sync::Arc;

use axum::{routing::get, Json, Router};
use serde_json::json;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

use crate::state::AppState;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "repohub=debug,tower_http=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let cfg = config::Config::load()?;
    tracing::info!(?cfg, "loaded config");

    let pool = db::init(&cfg).await?;
    let (status_tx, _status_rx) = tokio::sync::broadcast::channel::<String>(256);

    let state = AppState {
        db: pool,
        cfg: Arc::new(cfg.clone()),
        status_tx,
    };

    scheduler::spawn(state.clone());

    let app = Router::new()
        .route("/api/health", get(health))
        .merge(repos::router())
        .merge(claude_api::router())
        .merge(bulk::router())
        .merge(infra::router())
        .merge(settings_api::router())
        .merge(prompts_api::router())
        .merge(transcripts::router())
        .merge(ws_status::router())
        .with_state(state)
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive());

    let addr = SocketAddr::from((cfg.bind_host, cfg.port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("RepoHub backend listening on http://{addr}");

    axum::serve(listener, app).await?;
    Ok(())
}

async fn health() -> Json<serde_json::Value> {
    Json(json!({
        "ok": true,
        "service": "repohub",
        "version": env!("CARGO_PKG_VERSION"),
    }))
}
