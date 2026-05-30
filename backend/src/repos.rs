//! Repo management API: list, refresh from GitHub, track/clone/pull, status,
//! and bulk delete (local clone and/or remote).

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::PathBuf;

use crate::auth_mw::{self, Principal};
use crate::error::{ApiResult, AppError};
use crate::models::Repo;
use crate::state::AppState;
use crate::{github, gitops};

use sha2::{Digest, Sha256};

/// SHA-256 (lowercase hex) of a request body — bound into the Codex destructive
/// confirmation token so a confirmation can only authorize the exact payload.
fn body_hash(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    hex::encode(h.finalize())
}

const STAGING_BRANCH: &str = "repohub-staging";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/repos", get(list_repos).delete(delete_repos))
        .route("/api/repos/refresh", post(refresh_repos))
        .route("/api/repos/pull-all", post(pull_all))
        .route("/api/repos/fetch-all", post(fetch_all))
        .route("/api/repos/:id/track", post(track_repo))
        .route("/api/repos/:id/clone", post(clone_repo))
        .route("/api/repos/:id/pull", post(pull_repo))
        .route("/api/repos/:id/refresh-status", post(refresh_status))
}

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339()
}

async fn fetch_repo(state: &AppState, id: i64) -> ApiResult<Repo> {
    sqlx::query_as::<_, Repo>("SELECT * FROM repos WHERE id = ?1")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::msg(format!("repo {id} not found")))
}

fn broadcast(state: &AppState, event: serde_json::Value) {
    let _ = state.status_tx.send(event.to_string());
}

// ---------------------------------------------------------------------------
// GET /api/repos
// ---------------------------------------------------------------------------

async fn list_repos(State(state): State<AppState>) -> ApiResult<Json<Vec<Repo>>> {
    let rows = sqlx::query_as::<_, Repo>("SELECT * FROM repos ORDER BY full_name")
        .fetch_all(&state.db)
        .await?;
    Ok(Json(rows))
}

// ---------------------------------------------------------------------------
// POST /api/repos/refresh — gh list -> upsert
// ---------------------------------------------------------------------------

async fn refresh_repos(State(state): State<AppState>) -> ApiResult<Json<Vec<Repo>>> {
    let remote = github::list_repos().await?;
    let now = now_iso();

    for r in &remote {
        // Upsert by full_name, preserving local tracking/clone columns.
        sqlx::query(
            "INSERT INTO repos (full_name, name, owner, private, language, description, \
                default_branch, disk_kb, last_commit_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) \
             ON CONFLICT(full_name) DO UPDATE SET \
                name = excluded.name, owner = excluded.owner, private = excluded.private, \
                language = excluded.language, description = excluded.description, \
                default_branch = excluded.default_branch, disk_kb = excluded.disk_kb, \
                last_commit_at = excluded.last_commit_at, updated_at = excluded.updated_at",
        )
        .bind(&r.full_name)
        .bind(&r.name)
        .bind(&r.owner)
        .bind(r.is_private)
        .bind(&r.language)
        .bind(&r.description)
        .bind(&r.default_branch)
        .bind(r.disk_usage)
        .bind(if r.pushed_at.is_empty() {
            None
        } else {
            Some(r.pushed_at.clone())
        })
        .bind(&now)
        .execute(&state.db)
        .await?;
    }

    let rows = sqlx::query_as::<_, Repo>("SELECT * FROM repos ORDER BY full_name")
        .fetch_all(&state.db)
        .await?;
    broadcast(
        &state,
        json!({ "event": "repos_refreshed", "count": rows.len() }),
    );
    Ok(Json(rows))
}

// ---------------------------------------------------------------------------
// Clone / status helpers
// ---------------------------------------------------------------------------

fn local_path_for(state: &AppState, repo: &Repo) -> PathBuf {
    state.cfg.repos_dir().join(&repo.name)
}

/// Clone the repo if not already present and record its local path/status.
async fn do_clone(state: &AppState, repo: &Repo) -> ApiResult<()> {
    let dest = local_path_for(state, repo);
    if !dest.join(".git").exists() {
        let url = github::clone_url(&repo.full_name);
        gitops::clone(&url, &dest)
            .await
            .map_err(AppError::Anyhow)?;
    }
    sqlx::query(
        "UPDATE repos SET local_path = ?1, clone_status = 'cloned', updated_at = ?2 WHERE id = ?3",
    )
    .bind(dest.to_string_lossy().to_string())
    .bind(now_iso())
    .bind(repo.id)
    .execute(&state.db)
    .await?;
    Ok(())
}

/// Recompute git status for a repo and persist it. No-op if not cloned.
async fn recompute_status(state: &AppState, id: i64) -> ApiResult<Repo> {
    let repo = fetch_repo(state, id).await?;
    if let Some(lp) = &repo.local_path {
        let path = PathBuf::from(lp);
        if path.join(".git").exists() {
            let st = gitops::status(&path).await.unwrap_or_default();
            let last_commit = gitops::last_commit_iso(&path).await.unwrap_or(None);
            sqlx::query(
                "UPDATE repos SET ahead = ?1, behind = ?2, dirty = ?3, last_commit_at = ?4, \
                 updated_at = ?5 WHERE id = ?6",
            )
            .bind(st.ahead)
            .bind(st.behind)
            .bind(st.dirty)
            .bind(last_commit)
            .bind(now_iso())
            .bind(id)
            .execute(&state.db)
            .await?;
        }
    }
    fetch_repo(state, id).await
}

// ---------------------------------------------------------------------------
// POST /api/repos/:id/track
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Serialize)]
pub struct TrackBody {
    pub tracked: bool,
    /// Echo of the confirmation token issued on the first (challenged) attempt.
    /// Only consulted for the Codex principal (P19 destructive-action guard).
    #[serde(default)]
    pub confirm_token: Option<String>,
}

async fn track_repo(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<i64>,
    Json(body): Json<TrackBody>,
) -> ApiResult<Json<Repo>> {
    // Tracking can trigger a clone (writes to disk). Codex must double-confirm.
    let supplied = body.confirm_token.clone();
    let mut for_hash = body;
    for_hash.confirm_token = None;
    let canonical = serde_json::to_vec(&for_hash).unwrap_or_default();
    let body = for_hash;
    auth_mw::require_confirmation_json(
        &state,
        &principal,
        "POST",
        &format!("/api/repos/{id}/track"),
        &canonical,
        supplied.as_deref(),
    )
    .await?;

    let repo = fetch_repo(&state, id).await?;

    sqlx::query("UPDATE repos SET tracked = ?1, updated_at = ?2 WHERE id = ?3")
        .bind(body.tracked)
        .bind(now_iso())
        .bind(id)
        .execute(&state.db)
        .await?;

    if body.tracked {
        // Tracking implies a local clone exists.
        let lp_exists = repo
            .local_path
            .as_ref()
            .map(|p| PathBuf::from(p).join(".git").exists())
            .unwrap_or(false);
        if !lp_exists {
            do_clone(&state, &repo).await?;
        }
    }

    let updated = recompute_status(&state, id).await?;
    broadcast(
        &state,
        json!({ "event": "repo_tracked", "id": id, "tracked": body.tracked }),
    );
    Ok(Json(updated))
}

// ---------------------------------------------------------------------------
// POST /api/repos/:id/clone
// ---------------------------------------------------------------------------

async fn clone_repo(
    State(state): State<AppState>,
    principal: Principal,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<Json<Repo>> {
    auth_mw::require_confirmation_pathonly(
        &state,
        &principal,
        "POST",
        &format!("/api/repos/{id}/clone"),
        auth_mw::confirm_token_header(&headers).as_deref(),
    )
    .await?;

    let repo = fetch_repo(&state, id).await?;
    do_clone(&state, &repo).await?;
    let updated = recompute_status(&state, id).await?;
    broadcast(&state, json!({ "event": "repo_cloned", "id": id }));
    Ok(Json(updated))
}

// ---------------------------------------------------------------------------
// POST /api/repos/:id/pull
// ---------------------------------------------------------------------------

async fn pull_repo(
    State(state): State<AppState>,
    principal: Principal,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<Json<Repo>> {
    auth_mw::require_confirmation_pathonly(
        &state,
        &principal,
        "POST",
        &format!("/api/repos/{id}/pull"),
        auth_mw::confirm_token_header(&headers).as_deref(),
    )
    .await?;

    let repo = fetch_repo(&state, id).await?;
    if let Some(lp) = &repo.local_path {
        gitops::pull(&PathBuf::from(lp)).await.map_err(AppError::Anyhow)?;
    } else {
        return Err(AppError::msg("repo is not cloned locally"));
    }
    let updated = recompute_status(&state, id).await?;
    broadcast(&state, json!({ "event": "repo_pulled", "id": id }));
    Ok(Json(updated))
}

// ---------------------------------------------------------------------------
// POST /api/repos/pull-all  &  /api/repos/fetch-all
// ---------------------------------------------------------------------------

async fn tracked_cloned(state: &AppState) -> ApiResult<Vec<Repo>> {
    Ok(
        sqlx::query_as::<_, Repo>(
            "SELECT * FROM repos WHERE tracked = 1 AND local_path IS NOT NULL",
        )
        .fetch_all(&state.db)
        .await?,
    )
}

async fn pull_all(State(state): State<AppState>) -> ApiResult<Json<Vec<Repo>>> {
    for repo in tracked_cloned(&state).await? {
        if let Some(lp) = &repo.local_path {
            let _ = gitops::pull(&PathBuf::from(lp)).await;
            let _ = recompute_status(&state, repo.id).await;
        }
    }
    broadcast(&state, json!({ "event": "pull_all" }));
    list_repos(State(state)).await
}

async fn fetch_all(State(state): State<AppState>) -> ApiResult<Json<Vec<Repo>>> {
    for repo in tracked_cloned(&state).await? {
        if let Some(lp) = &repo.local_path {
            let _ = gitops::fetch(&PathBuf::from(lp)).await;
            sqlx::query("UPDATE repos SET last_fetch = ?1 WHERE id = ?2")
                .bind(now_iso())
                .bind(repo.id)
                .execute(&state.db)
                .await?;
            let _ = recompute_status(&state, repo.id).await;
        }
    }
    broadcast(&state, json!({ "event": "fetch_all" }));
    list_repos(State(state)).await
}

// ---------------------------------------------------------------------------
// POST /api/repos/:id/refresh-status
// ---------------------------------------------------------------------------

async fn refresh_status(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<Repo>> {
    let updated = recompute_status(&state, id).await?;
    broadcast(
        &state,
        json!({ "event": "repo_status", "id": id, "ahead": updated.ahead, "behind": updated.behind, "dirty": updated.dirty }),
    );
    Ok(Json(updated))
}

// ---------------------------------------------------------------------------
// DELETE /api/repos
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Serialize)]
pub struct DeleteBody {
    pub ids: Vec<i64>,
    #[serde(default)]
    pub delete_local: bool,
    #[serde(default)]
    pub delete_remote: bool,
    /// Echo of the confirmation token issued on the first (challenged) attempt.
    /// Only consulted for the Codex principal (P19 destructive-action guard).
    #[serde(default)]
    pub confirm_token: Option<String>,
}

async fn delete_repos(
    State(state): State<AppState>,
    principal: Principal,
    Json(body): Json<DeleteBody>,
) -> ApiResult<Json<serde_json::Value>> {
    // Codex destructive-action guard: User/Local are exempt; Codex must present a
    // valid confirmation token bound to this exact request body.
    //
    // CRITICAL: hash the body with `confirm_token` cleared, so the challenge
    // (token=None) and the retry (token=Some) bind to the SAME hash. Hashing the
    // body *including* the echoed token would make the retry re-challenge forever.
    let supplied = body.confirm_token.clone();
    let mut for_hash = body;
    for_hash.confirm_token = None;
    let bh = body_hash(serde_json::to_vec(&for_hash).unwrap_or_default().as_slice());
    let body = for_hash;
    auth_mw::require_confirmation(
        &state,
        &principal,
        "DELETE",
        "/api/repos",
        &bh,
        supplied.as_deref(),
    )
    .await?;

    let mut deleted = Vec::new();

    for id in &body.ids {
        let repo = match fetch_repo(&state, *id).await {
            Ok(r) => r,
            Err(_) => continue,
        };

        if body.delete_local {
            if let Some(lp) = &repo.local_path {
                let _ = tokio::fs::remove_dir_all(PathBuf::from(lp)).await;
            }
        }
        if body.delete_remote {
            github::delete_repo(&repo.full_name).await?;
        }

        sqlx::query("DELETE FROM repos WHERE id = ?1")
            .bind(id)
            .execute(&state.db)
            .await?;
        deleted.push(*id);
    }

    broadcast(&state, json!({ "event": "repos_deleted", "ids": deleted }));
    Ok(Json(json!({ "deleted": deleted })))
}

/// Exposed for the bulk module: name of the per-repo staging branch.
pub fn staging_branch() -> &'static str {
    STAGING_BRANCH
}
