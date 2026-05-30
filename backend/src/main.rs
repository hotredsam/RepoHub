//! RepoHub backend — local-first multi-repo command center.
//!
//! Boots an axum server on `127.0.0.1`, wires up the SQLite database, spawns the
//! background fetch scheduler, and merges every feature router under one app.

mod agents_config;
mod audit;
mod auth_google;
mod auth_mw;
mod auth_session;
mod bulk;
mod codex;
mod claude_api;
mod claude_config;
mod claude_fs;
mod claude_runner;
mod config;
mod connections;
mod consistency;
mod db;
mod error;
mod evals_gcloud;
mod gcloud;
mod gh_perms;
mod github;
mod gitops;
mod infra;
mod knowledge;
mod mcp_config;
mod merge;
mod models;
mod prompts_api;
mod repos;
mod scheduler;
mod settings_api;
mod state;
mod tailscale;
mod terminal;
mod tickets;
mod transcripts;
mod ws_origin;
mod ws_status;

use std::net::SocketAddr;
use std::sync::Arc;

use axum::http::{HeaderValue, Method};
use axum::middleware::from_fn_with_state;
use axum::{routing::get, Json, Router};
use serde_json::json;
use tower_http::cors::{AllowOrigin, CorsLayer};
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

    // P18/P19: load (or generate + persist) the per-install auth signing key.
    let auth = Arc::new(auth_session::load_or_create_signing_key(&pool).await?);

    let state = AppState {
        db: pool,
        cfg: Arc::new(cfg.clone()),
        status_tx,
        auth,
    };

    scheduler::spawn(state.clone());

    // Cross-origin allowlist (P18): loopback (API port + Vite dev server) PLUS
    // https origins on this Mac's tailnet (`*.tail97ef37.ts.net`), so the SPA
    // served behind Tailscale serve can call the API with credentials. Anything
    // else is rejected. (WebSocket Origin enforcement lives in ws_origin.)
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::predicate(
            |origin: &HeaderValue, _req: &_| cors_origin_allowed(origin),
        ))
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
        .allow_headers([
            axum::http::header::CONTENT_TYPE,
            axum::http::header::AUTHORIZATION,
        ])
        .allow_credentials(true);

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
        .merge(terminal::router())
        .merge(connections::router())
        .merge(consistency::router())
        .merge(merge::router())
        .merge(claude_config::router())
        .merge(agents_config::router())
        .merge(mcp_config::router())
        .merge(knowledge::router())
        .merge(evals_gcloud::router())
        .merge(tickets::router())
        // P18/P19/P20 routers.
        .merge(auth_google::router())
        .merge(tailscale::router())
        .merge(codex::router())
        .merge(audit::router())
        .merge(gh_perms::router())
        // Auth + audit middleware. Layers run outermost-last, so the LAST
        // `.layer(..)` is the OUTERMOST (runs first on the way in). We want
        // `gate` to run before `record` so the stamped Principal is visible to
        // the audit layer — hence `record` is added first (inner) and `gate`
        // last (outer).
        .layer(from_fn_with_state(state.clone(), audit::record))
        .layer(from_fn_with_state(state.clone(), auth_mw::gate))
        .with_state(state)
        .layer(TraceLayer::new_for_http())
        .layer(cors);

    let addr = SocketAddr::from((cfg.bind_host, cfg.port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("RepoHub backend listening on http://{addr}");

    // `into_make_service_with_connect_info` exposes the peer `SocketAddr` via the
    // `ConnectInfo` extension so the auth gate can classify loopback vs remote.
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}

/// CORS predicate: allow loopback origins (any port) and https origins on this
/// Mac's tailnet (`*.tail97ef37.ts.net`). Everything else is rejected.
fn cors_origin_allowed(origin: &HeaderValue) -> bool {
    let Ok(o) = origin.to_str() else {
        return false;
    };
    // Loopback (http) on any port — covers the API port and the Vite dev server.
    if let Some(rest) = o.strip_prefix("http://") {
        let host = rest.split('/').next().unwrap_or(rest);
        let hostname = host.rsplit_once(':').map(|(h, _)| h).unwrap_or(host);
        if hostname == "127.0.0.1" || hostname == "localhost" || hostname == "[::1]" {
            return true;
        }
    }
    // https on the tailnet. Require the exact suffix on the host (no port: serve
    // terminates TLS on 443) and reject lookalike suffixes.
    if let Some(rest) = o.strip_prefix("https://") {
        let host = rest.split('/').next().unwrap_or(rest);
        // No explicit port permitted for the tailnet origin.
        if host.contains(':') {
            return false;
        }
        if host == "tail97ef37.ts.net" || host.ends_with(".tail97ef37.ts.net") {
            return true;
        }
    }
    false
}

async fn health() -> Json<serde_json::Value> {
    Json(json!({
        "ok": true,
        "service": "repohub",
        "version": env!("CARGO_PKG_VERSION"),
    }))
}
