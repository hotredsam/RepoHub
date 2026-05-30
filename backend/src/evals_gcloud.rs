//! Evals + Google Cloud configuration API (P16).
//!
//! Two concerns, one router:
//!
//! 1. **Google Cloud config block.** A single global config — `gcloud_project`
//!    and `gcloud_region` — stored as `scope='global'` settings rows (the same
//!    block every cloud feature keys off). `GET /api/gcloud` returns the
//!    best-effort [`gcloud::status`] probe (CLI present? ADC present? configured
//!    project/region?); `PUT /api/gcloud` upserts the project/region settings.
//!
//! 2. **Eval suites + runs.** Suites live in `eval_suites` (global or per-repo
//!    scope), runs in `eval_runs`. Running a suite attempts a live Vertex AI Gen
//!    AI Evaluation only when gcloud is ready; otherwise it degrades gracefully,
//!    recording a run row whose `detail_json` explains that cloud is not
//!    configured. Either way the caller gets a row back so the UI can render it.
//!
//! Patterns mirror the sibling feature modules (`infra`, `settings_api`):
//! `pub fn router() -> Router<AppState>`, `State<AppState>`, handlers returning
//! `ApiResult<Json<..>>`, runtime sqlx (`query` / `query_as`, never the
//! compile-time macros), and NULL-aware settings upserts.

use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::FromRow;

use crate::error::{ApiResult, AppError};
use crate::gcloud::{self, GcloudStatus};
use crate::state::AppState;

/// Scope used for application-wide (non-repo) settings and suites.
const GLOBAL_SCOPE: &str = "global";

/// Settings keys for the single Google Cloud config block.
const PROJECT_KEY: &str = "gcloud_project";
const REGION_KEY: &str = "gcloud_region";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/gcloud", get(get_gcloud).put(put_gcloud))
        .route("/api/evals", get(list_evals).post(create_eval))
        .route("/api/evals/runs", get(list_runs))
        .route("/api/evals/:id", axum::routing::delete(delete_eval))
        .route("/api/evals/:id/run", post(run_eval))
}

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339()
}

// ---------------------------------------------------------------------------
// Google Cloud config block
// ---------------------------------------------------------------------------

/// GET /api/gcloud — best-effort probe of the local Google Cloud setup plus the
/// configured project/region. Never errors (the underlying probe is infallible).
async fn get_gcloud(State(state): State<AppState>) -> Json<GcloudStatus> {
    Json(gcloud::status(&state.db).await)
}

#[derive(Debug, Deserialize)]
pub struct GcloudConfigBody {
    /// GCP project id. `None` leaves the existing value untouched; an explicit
    /// empty string clears it.
    pub project: Option<String>,
    /// GCP region. Same semantics as `project`.
    pub region: Option<String>,
}

/// PUT /api/gcloud — upsert the `gcloud_project` / `gcloud_region` global
/// settings, then return the refreshed status.
async fn put_gcloud(
    State(state): State<AppState>,
    Json(body): Json<GcloudConfigBody>,
) -> ApiResult<Json<GcloudStatus>> {
    if let Some(project) = body.project.as_deref() {
        upsert_global_setting(&state, PROJECT_KEY, project.trim()).await?;
    }
    if let Some(region) = body.region.as_deref() {
        upsert_global_setting(&state, REGION_KEY, region.trim()).await?;
    }
    Ok(Json(gcloud::status(&state.db).await))
}

/// NULL-aware upsert of a single `scope='global'`, `repo_id IS NULL` setting.
///
/// Mirrors `settings_api::upsert_setting_row`: SQLite treats `NULL` as distinct
/// in unique indexes, so we update-then-insert in a transaction rather than rely
/// on `ON CONFLICT`. We do not depend on `settings_api`'s private helper so this
/// module stays self-contained (and only touches its own file).
async fn upsert_global_setting(state: &AppState, key: &str, value: &str) -> ApiResult<()> {
    let mut tx = state.db.begin().await?;

    let updated = sqlx::query(
        "UPDATE settings SET value = ?2 \
         WHERE scope = ?1 AND repo_id IS NULL AND key = ?3",
    )
    .bind(GLOBAL_SCOPE)
    .bind(value)
    .bind(key)
    .execute(&mut *tx)
    .await?;

    if updated.rows_affected() == 0 {
        sqlx::query(
            "INSERT INTO settings (scope, repo_id, key, value) VALUES (?1, NULL, ?2, ?3)",
        )
        .bind(GLOBAL_SCOPE)
        .bind(key)
        .bind(value)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Eval suites
// ---------------------------------------------------------------------------

/// One row of `eval_suites`. `config_json` is the raw stored JSON text (the
/// Vertex AI Gen AI Evaluation config); the UI parses it as needed.
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct EvalSuite {
    pub id: i64,
    pub name: String,
    pub scope: String,
    pub repo_id: Option<i64>,
    pub config_json: Option<String>,
    pub created_at: Option<String>,
}

/// One row of `eval_runs`. `passed` is stored as an integer count (not a bool);
/// `detail_json` carries the run outcome / not-configured explanation.
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct EvalRun {
    pub id: i64,
    pub suite_id: Option<i64>,
    pub model: Option<String>,
    pub score: Option<f64>,
    pub passed: Option<i64>,
    pub total: Option<i64>,
    pub detail_json: Option<String>,
    pub created_at: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ListEvalsQuery {
    pub scope: Option<String>,
    pub repo_id: Option<i64>,
}

/// GET /api/evals?scope=&repo_id= — list suites, optionally filtered by scope
/// and/or repo. `repo_id` filtering is NULL-aware so a global-scope query
/// (`repo_id` omitted) still matches the global suites whose `repo_id IS NULL`.
async fn list_evals(
    State(state): State<AppState>,
    Query(q): Query<ListEvalsQuery>,
) -> ApiResult<Json<Vec<EvalSuite>>> {
    let scope = q.scope.as_deref().map(str::trim).filter(|s| !s.is_empty());

    let rows: Vec<EvalSuite> = match (scope, q.repo_id) {
        (Some(scope), Some(repo_id)) => {
            sqlx::query_as::<_, EvalSuite>(
                "SELECT id, name, scope, repo_id, config_json, created_at FROM eval_suites \
                 WHERE scope = ?1 AND repo_id = ?2 ORDER BY created_at DESC, id DESC",
            )
            .bind(scope)
            .bind(repo_id)
            .fetch_all(&state.db)
            .await?
        }
        (Some(scope), None) => {
            sqlx::query_as::<_, EvalSuite>(
                "SELECT id, name, scope, repo_id, config_json, created_at FROM eval_suites \
                 WHERE scope = ?1 ORDER BY created_at DESC, id DESC",
            )
            .bind(scope)
            .fetch_all(&state.db)
            .await?
        }
        (None, Some(repo_id)) => {
            sqlx::query_as::<_, EvalSuite>(
                "SELECT id, name, scope, repo_id, config_json, created_at FROM eval_suites \
                 WHERE repo_id = ?1 ORDER BY scope, created_at DESC, id DESC",
            )
            .bind(repo_id)
            .fetch_all(&state.db)
            .await?
        }
        (None, None) => {
            sqlx::query_as::<_, EvalSuite>(
                "SELECT id, name, scope, repo_id, config_json, created_at FROM eval_suites \
                 ORDER BY scope, created_at DESC, id DESC",
            )
            .fetch_all(&state.db)
            .await?
        }
    };

    Ok(Json(rows))
}

#[derive(Debug, Deserialize)]
pub struct CreateEvalBody {
    pub name: String,
    /// Defaults to `"global"` when omitted.
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub repo_id: Option<i64>,
    /// Vertex AI Gen AI Evaluation config; stored verbatim as JSON text.
    #[serde(default)]
    pub config: Option<Value>,
}

/// POST /api/evals — create a suite in `eval_suites`.
async fn create_eval(
    State(state): State<AppState>,
    Json(body): Json<CreateEvalBody>,
) -> ApiResult<Json<EvalSuite>> {
    let name = body.name.trim();
    if name.is_empty() {
        return Err(AppError::msg("eval suite name must not be empty"));
    }
    let scope = body
        .scope
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(GLOBAL_SCOPE)
        .to_string();

    // Serialize the config object back to compact text for storage. `None`
    // stays NULL so the column round-trips cleanly.
    let config_json = match &body.config {
        Some(v) => Some(serde_json::to_string(v).map_err(|e| AppError::msg(e.to_string()))?),
        None => None,
    };
    let created_at = now_iso();

    let res = sqlx::query(
        "INSERT INTO eval_suites (name, scope, repo_id, config_json, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )
    .bind(name)
    .bind(&scope)
    .bind(body.repo_id)
    .bind(&config_json)
    .bind(&created_at)
    .execute(&state.db)
    .await?;

    let row = sqlx::query_as::<_, EvalSuite>(
        "SELECT id, name, scope, repo_id, config_json, created_at FROM eval_suites WHERE id = ?1",
    )
    .bind(res.last_insert_rowid())
    .fetch_one(&state.db)
    .await?;

    Ok(Json(row))
}

/// DELETE /api/evals/:id — remove a suite and its runs.
async fn delete_eval(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    // Drop dependent runs first so we never orphan rows (no FK cascade defined).
    sqlx::query("DELETE FROM eval_runs WHERE suite_id = ?1")
        .bind(id)
        .execute(&state.db)
        .await?;
    sqlx::query("DELETE FROM eval_suites WHERE id = ?1")
        .bind(id)
        .execute(&state.db)
        .await?;
    Ok(Json(json!({ "deleted": id })))
}

// ---------------------------------------------------------------------------
// Eval runs
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct RunEvalBody {
    /// Optional model override for this run; recorded on the run row.
    #[serde(default)]
    pub model: Option<String>,
}

/// POST /api/evals/:id/run — execute a suite.
///
/// When gcloud is ready we attempt a (currently stubbed) live Vertex AI Gen AI
/// Evaluation; otherwise we degrade gracefully. Either way we record an
/// `eval_runs` row whose `detail_json` captures the outcome, and return it so
/// the UI can show configured-vs-not state without a separate fetch.
async fn run_eval(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<RunEvalBody>,
) -> ApiResult<Json<EvalRun>> {
    // Suite must exist.
    let suite = sqlx::query_as::<_, EvalSuite>(
        "SELECT id, name, scope, repo_id, config_json, created_at FROM eval_suites WHERE id = ?1",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| AppError::msg(format!("eval suite {id} not found")))?;

    let model = body.model.as_deref().map(str::trim).filter(|s| !s.is_empty());

    let st = gcloud::status(&state.db).await;

    // Build the eval config: the suite's stored config, falling back to {}.
    let cfg: Value = suite
        .config_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_else(|| json!({}));

    // detail_json records the outcome regardless of configured-ness. We never
    // log or embed secrets here — only suite metadata and status flags.
    let detail: Value = if st.ready() {
        match gcloud::eval_run(&state.db, &cfg).await {
            Ok(result) => json!({
                "status": "ok",
                "configured": true,
                "result": result,
            }),
            // gcloud is present but the live call is still stubbed (TODO), or it
            // failed: capture the message so the UI can surface it.
            Err(e) => json!({
                "status": "error",
                "configured": true,
                "message": e.to_string(),
            }),
        }
    } else {
        json!({
            "status": "not_configured",
            "configured": false,
            "message": "Google Cloud not configured: install gcloud + run gcloud auth application-default login",
        })
    };

    let detail_json = serde_json::to_string(&detail).map_err(|e| AppError::msg(e.to_string()))?;
    let created_at = now_iso();

    // A stubbed/not-configured run has no real scores yet; leave them NULL.
    let res = sqlx::query(
        "INSERT INTO eval_runs (suite_id, model, score, passed, total, detail_json, created_at) \
         VALUES (?1, ?2, NULL, NULL, NULL, ?3, ?4)",
    )
    .bind(suite.id)
    .bind(model)
    .bind(&detail_json)
    .bind(&created_at)
    .execute(&state.db)
    .await?;

    let row = sqlx::query_as::<_, EvalRun>(
        "SELECT id, suite_id, model, score, passed, total, detail_json, created_at \
         FROM eval_runs WHERE id = ?1",
    )
    .bind(res.last_insert_rowid())
    .fetch_one(&state.db)
    .await?;

    Ok(Json(row))
}

#[derive(Debug, Deserialize)]
pub struct ListRunsQuery {
    pub suite_id: Option<i64>,
}

/// GET /api/evals/runs?suite_id= — list runs, newest first, optionally for one
/// suite.
async fn list_runs(
    State(state): State<AppState>,
    Query(q): Query<ListRunsQuery>,
) -> ApiResult<Json<Vec<EvalRun>>> {
    let rows: Vec<EvalRun> = match q.suite_id {
        Some(suite_id) => {
            sqlx::query_as::<_, EvalRun>(
                "SELECT id, suite_id, model, score, passed, total, detail_json, created_at \
                 FROM eval_runs WHERE suite_id = ?1 ORDER BY created_at DESC, id DESC",
            )
            .bind(suite_id)
            .fetch_all(&state.db)
            .await?
        }
        None => {
            sqlx::query_as::<_, EvalRun>(
                "SELECT id, suite_id, model, score, passed, total, detail_json, created_at \
                 FROM eval_runs ORDER BY created_at DESC, id DESC",
            )
            .fetch_all(&state.db)
            .await?
        }
    };
    Ok(Json(rows))
}
