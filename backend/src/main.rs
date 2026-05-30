//! RepoHub backend — local-first multi-repo command center.
//!
//! P0: boots an axum server on 127.0.0.1 with a health endpoint.
//! Later phases add: github, gitops, scheduler, db, claude_runner, pty,
//! integration, consistency, settings, transcripts.

mod config;
mod routes;

use std::net::SocketAddr;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| "repohub=debug,tower_http=info".into()))
        .with(tracing_subscriber::fmt::layer())
        .init();

    let cfg = config::Config::load()?;
    tracing::info!(?cfg, "loaded config");

    let app = routes::router(cfg.clone());

    let addr = SocketAddr::from((cfg.bind_host, cfg.port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("RepoHub backend listening on http://{addr}");

    axum::serve(listener, app).await?;
    Ok(())
}
