//! Bulk Claude jobs: apply one prompt across many tracked repos on a per-repo
//! staging branch, committing the result.

use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::claude_runner;
use crate::error::{ApiResult, AppError};
use crate::models::{BulkJob, BulkJobItem, Repo};
use crate::repos::staging_branch;
use crate::state::AppState;
use crate::gitops;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/bulk/prompt", post(create_prompt))
        .route("/api/bulk/jobs", get(list_jobs))
        .route("/api/bulk/jobs/:id", get(get_job))
}

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339()
}

#[derive(Debug, Deserialize)]
pub struct BulkPromptBody {
    pub repo_ids: Vec<i64>,
    pub prompt: String,
    #[serde(default = "default_kind")]
    pub kind: String,
}

fn default_kind() -> String {
    "prompt".to_string()
}

#[derive(Debug, Serialize)]
pub struct BulkJobWithItems {
    #[serde(flatten)]
    pub job: BulkJob,
    pub items: Vec<BulkJobItem>,
}

async fn create_prompt(
    State(state): State<AppState>,
    Json(body): Json<BulkPromptBody>,
) -> ApiResult<Json<BulkJobWithItems>> {
    if body.prompt.trim().is_empty() {
        return Err(AppError::msg("prompt must not be empty"));
    }

    // Create the job row.
    let res = sqlx::query(
        "INSERT INTO bulk_jobs (kind, prompt, status, created_at) VALUES (?1, ?2, 'running', ?3)",
    )
    .bind(&body.kind)
    .bind(&body.prompt)
    .bind(now_iso())
    .execute(&state.db)
    .await?;
    let job_id = res.last_insert_rowid();

    // Create one item per requested repo.
    for repo_id in &body.repo_ids {
        sqlx::query(
            "INSERT INTO bulk_job_items (job_id, repo_id, status, branch) VALUES (?1, ?2, 'pending', ?3)",
        )
        .bind(job_id)
        .bind(repo_id)
        .bind(staging_branch())
        .execute(&state.db)
        .await?;
    }

    // Process each tracked, cloned repo synchronously.
    for repo_id in &body.repo_ids {
        let repo: Option<Repo> = sqlx::query_as::<_, Repo>("SELECT * FROM repos WHERE id = ?1")
            .bind(repo_id)
            .fetch_optional(&state.db)
            .await?;

        let Some(repo) = repo else {
            set_item(&state, job_id, *repo_id, "error", None, Some("repo not found")).await?;
            continue;
        };

        let local_path = match &repo.local_path {
            Some(p) if PathBuf::from(p).join(".git").exists() => PathBuf::from(p),
            _ => {
                set_item(&state, job_id, *repo_id, "skipped", None, Some("not cloned")).await?;
                continue;
            }
        };

        // Stage branch, run claude, commit.
        if let Err(e) = gitops::ensure_staging_branch(&local_path, staging_branch()).await {
            set_item(&state, job_id, *repo_id, "error", None, Some(&e.to_string())).await?;
            continue;
        }

        match claude_runner::run_collect(&local_path, &body.prompt).await {
            Ok(outcome) => {
                let committed = gitops::commit_all(
                    &local_path,
                    &format!("RepoHub bulk: {}", truncate(&body.prompt, 60)),
                )
                .await
                .unwrap_or(false);

                // Persist the claude exchange as a prompt row too.
                let _ = sqlx::query(
                    "INSERT INTO prompts (repo_id, scope, source, model, prompt, response, \
                        tokens_in, tokens_out, created_at) \
                     VALUES (?1, 'repo', 'bulk', NULL, ?2, ?3, ?4, ?5, ?6)",
                )
                .bind(repo_id)
                .bind(&body.prompt)
                .bind(&outcome.response)
                .bind(outcome.tokens_in)
                .bind(outcome.tokens_out)
                .bind(now_iso())
                .execute(&state.db)
                .await;

                let status = if committed { "committed" } else { "no-changes" };
                set_item(
                    &state,
                    job_id,
                    *repo_id,
                    status,
                    Some(&outcome.response),
                    None,
                )
                .await?;
            }
            Err(e) => {
                set_item(&state, job_id, *repo_id, "error", None, Some(&e.to_string())).await?;
            }
        }
    }

    sqlx::query("UPDATE bulk_jobs SET status = 'done' WHERE id = ?1")
        .bind(job_id)
        .execute(&state.db)
        .await?;

    let _ = state
        .status_tx
        .send(serde_json::json!({ "event": "bulk_done", "job_id": job_id }).to_string());

    load_job(&state, job_id).await.map(Json)
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n).collect::<String>() + "…"
    }
}

async fn set_item(
    state: &AppState,
    job_id: i64,
    repo_id: i64,
    status: &str,
    log: Option<&str>,
    error: Option<&str>,
) -> ApiResult<()> {
    sqlx::query(
        "UPDATE bulk_job_items SET status = ?1, log = ?2, error = ?3 \
         WHERE job_id = ?4 AND repo_id = ?5",
    )
    .bind(status)
    .bind(log)
    .bind(error)
    .bind(job_id)
    .bind(repo_id)
    .execute(&state.db)
    .await?;
    Ok(())
}

async fn load_job(state: &AppState, job_id: i64) -> ApiResult<BulkJobWithItems> {
    let job = sqlx::query_as::<_, BulkJob>("SELECT * FROM bulk_jobs WHERE id = ?1")
        .bind(job_id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::msg(format!("bulk job {job_id} not found")))?;
    let items = sqlx::query_as::<_, BulkJobItem>(
        "SELECT * FROM bulk_job_items WHERE job_id = ?1 ORDER BY id",
    )
    .bind(job_id)
    .fetch_all(&state.db)
    .await?;
    Ok(BulkJobWithItems { job, items })
}

async fn list_jobs(State(state): State<AppState>) -> ApiResult<Json<Vec<BulkJob>>> {
    let rows = sqlx::query_as::<_, BulkJob>("SELECT * FROM bulk_jobs ORDER BY created_at DESC")
        .fetch_all(&state.db)
        .await?;
    Ok(Json(rows))
}

async fn get_job(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<BulkJobWithItems>> {
    load_job(&state, id).await.map(Json)
}
