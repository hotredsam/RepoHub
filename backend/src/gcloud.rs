//! Google Cloud integration surface (Secret Manager, Vertex AI embeddings /
//! vector search / Gen AI evaluation).
//!
//! `gcloud` is NOT assumed to be installed in this environment. Every path here
//! DEGRADES GRACEFULLY: when the CLI is missing or Application Default
//! Credentials (ADC) are absent, the async client methods return a clear,
//! actionable `AppError` instead of crashing. All Google Cloud features key off
//! ONE config block stored as `scope='global'` settings rows: `gcloud_project`
//! and `gcloud_region`.
//!
//! The live calls are STUBBED (marked TODO) and routed through the `gcloud` CLI
//! once it is installed and authenticated; secrets are never logged.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::error::{ApiResult, AppError};

/// Settings scope and keys for the single Google Cloud config block.
const GLOBAL_SCOPE: &str = "global";
const PROJECT_KEY: &str = "gcloud_project";
const REGION_KEY: &str = "gcloud_region";

/// Standard, actionable message returned whenever Google Cloud is unavailable.
const NOT_CONFIGURED: &str =
    "Google Cloud not configured: install gcloud + run gcloud auth application-default login";

/// Snapshot of the local Google Cloud setup, surfaced to the UI so it can show
/// inherit-vs-override and guide the user toward enabling cloud features.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GcloudStatus {
    /// Whether the `gcloud` CLI is on `PATH`.
    pub installed: bool,
    /// Whether Application Default Credentials are present on disk.
    pub adc: bool,
    /// Configured GCP project id (from the `gcloud_project` global setting).
    pub project: Option<String>,
    /// Configured GCP region (from the `gcloud_region` global setting).
    pub region: Option<String>,
}

impl GcloudStatus {
    /// Whether live Google Cloud calls may be attempted.
    pub fn ready(&self) -> bool {
        self.installed && self.adc
    }
}

/// Path to the Application Default Credentials file
/// (`~/.config/gcloud/application_default_credentials.json`).
fn adc_path() -> Option<PathBuf> {
    std::env::var("HOME")
        .ok()
        .map(|home| PathBuf::from(home).join(".config/gcloud/application_default_credentials.json"))
}

/// Whether the `gcloud` CLI is installed and discoverable on `PATH`.
async fn gcloud_installed() -> bool {
    tokio::process::Command::new("which")
        .arg("gcloud")
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Read a single global setting value from the `settings` table.
async fn global_setting(db: &SqlitePool, key: &str) -> Option<String> {
    sqlx::query_as::<_, (String,)>(
        "SELECT value FROM settings WHERE scope = ?1 AND repo_id IS NULL AND key = ?2",
    )
    .bind(GLOBAL_SCOPE)
    .bind(key)
    .fetch_optional(db)
    .await
    .ok()
    .flatten()
    .map(|r| r.0)
    .filter(|s| !s.is_empty())
}

/// Probe the local Google Cloud setup: CLI presence, ADC file, and the
/// configured project/region (from settings). Never errors — always returns a
/// best-effort status.
pub async fn status(db: &SqlitePool) -> GcloudStatus {
    let installed = gcloud_installed().await;
    let adc = adc_path().map(|p| p.exists()).unwrap_or(false);
    let project = global_setting(db, PROJECT_KEY).await;
    let region = global_setting(db, REGION_KEY).await;
    GcloudStatus {
        installed,
        adc,
        project,
        region,
    }
}

/// Guard used by every client method: returns the standard "not configured"
/// error unless both the CLI and ADC are present.
fn require_ready(status: &GcloudStatus) -> ApiResult<()> {
    if status.ready() {
        Ok(())
    } else {
        Err(AppError::msg(NOT_CONFIGURED))
    }
}

/// Which embedding backend to use for a given scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EmbedProvider {
    /// Vertex AI hosted embeddings (requires gcloud + ADC).
    Vertex,
    /// Local embedding model (runs without Google Cloud).
    Local,
}

/// One vectorized item to upsert into the vector store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorItem {
    pub id: String,
    pub embedding: Vec<f32>,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

/// One match returned from a vector query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorMatch {
    pub id: String,
    pub score: f32,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

/// Fetch a secret payload from Google Cloud Secret Manager by name.
///
/// STUB: degrades gracefully when gcloud/ADC are missing. TODO: invoke
/// `gcloud secrets versions access` (args passed separately; payload never
/// logged) once the CLI is installed and authenticated.
pub async fn secret_get(db: &SqlitePool, name: &str) -> ApiResult<String> {
    let _ = name;
    let st = status(db).await;
    require_ready(&st)?;
    // TODO: live `gcloud secrets versions access latest --secret=<name>`.
    Err(AppError::msg(
        "Google Cloud Secret Manager access is not yet implemented (gcloud detected)",
    ))
}

/// Embed `texts` using the selected provider.
///
/// `Local` may run without Google Cloud (TODO: wire a local model). `Vertex`
/// degrades gracefully when gcloud/ADC are missing.
pub async fn embed(
    db: &SqlitePool,
    provider: EmbedProvider,
    texts: &[String],
) -> ApiResult<Vec<Vec<f32>>> {
    let _ = texts;
    match provider {
        EmbedProvider::Local => {
            // TODO: run a local embedding model.
            Err(AppError::msg(
                "Local embeddings are not yet implemented",
            ))
        }
        EmbedProvider::Vertex => {
            let st = status(db).await;
            require_ready(&st)?;
            // TODO: live Vertex AI text-embeddings prediction.
            Err(AppError::msg(
                "Vertex AI embeddings are not yet implemented (gcloud detected)",
            ))
        }
    }
}

/// Upsert vector items into Vertex AI Vector Search.
///
/// STUB: degrades gracefully when gcloud/ADC are missing. TODO: live upsert.
pub async fn vector_upsert(db: &SqlitePool, items: &[VectorItem]) -> ApiResult<()> {
    let _ = items;
    let st = status(db).await;
    require_ready(&st)?;
    // TODO: live Vertex AI Vector Search upsert.
    Err(AppError::msg(
        "Vertex AI Vector Search upsert is not yet implemented (gcloud detected)",
    ))
}

/// Query Vertex AI Vector Search for nearest neighbours of `q`.
///
/// STUB: degrades gracefully when gcloud/ADC are missing. TODO: live query.
pub async fn vector_query(db: &SqlitePool, q: &[f32]) -> ApiResult<Vec<VectorMatch>> {
    let _ = q;
    let st = status(db).await;
    require_ready(&st)?;
    // TODO: live Vertex AI Vector Search nearest-neighbour query.
    Err(AppError::msg(
        "Vertex AI Vector Search query is not yet implemented (gcloud detected)",
    ))
}

/// Run a Vertex AI Gen AI Evaluation with the given JSON config.
///
/// STUB: degrades gracefully when gcloud/ADC are missing. TODO: live eval.
pub async fn eval_run(db: &SqlitePool, cfg: &serde_json::Value) -> ApiResult<serde_json::Value> {
    let _ = cfg;
    let st = status(db).await;
    require_ready(&st)?;
    // TODO: live Vertex AI Gen AI Evaluation run.
    Err(AppError::msg(
        "Vertex AI Gen AI Evaluation is not yet implemented (gcloud detected)",
    ))
}
