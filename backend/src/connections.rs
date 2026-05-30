//! Connections / integration engine.
//!
//! Exposes the set of tracked, cloned repos as a simple graph and lets the user
//! pull a feature from one repo (the *source*) into another (the *target*) by
//! handing Claude a focused instruction. All changes land on the per-repo
//! `repohub-staging` branch — never on the default branch directly.

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::{Path, PathBuf};
use tokio::process::Command;

use crate::auth_mw::{self, Principal};
use crate::error::{ApiResult, AppError};
use crate::models::Repo;
use crate::state::AppState;
use crate::{claude_runner, gitops};

const STAGING_BRANCH: &str = "repohub-staging";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/connections/graph", get(graph))
        .route("/api/connections/integrate", post(integrate))
}

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn broadcast(state: &AppState, event: serde_json::Value) {
    let _ = state.status_tx.send(event.to_string());
}

async fn fetch_repo(state: &AppState, id: i64) -> ApiResult<Repo> {
    sqlx::query_as::<_, Repo>("SELECT * FROM repos WHERE id = ?1")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::msg(format!("repo {id} not found")))
}

/// Resolve a repo's on-disk path, erroring with a clear message when the repo
/// has not been cloned locally.
fn require_local_path(repo: &Repo, role: &str) -> ApiResult<PathBuf> {
    match &repo.local_path {
        Some(p) if !p.is_empty() && PathBuf::from(p).join(".git").exists() => {
            Ok(PathBuf::from(p))
        }
        _ => Err(AppError::msg(format!(
            "{role} repo '{}' is not cloned locally; clone it first",
            repo.full_name
        ))),
    }
}

/// Run `git -C <path> <args>` (best-effort), returning stdout. Each argument is
/// passed separately — nothing is routed through a shell.
async fn git_capture(path: &Path, args: &[&str]) -> anyhow::Result<(bool, String, String)> {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(path);
    for a in args {
        cmd.arg(a);
    }
    let out = cmd.output().await?;
    Ok((
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    ))
}

// ---------------------------------------------------------------------------
// GET /api/connections/graph
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct GraphNode {
    pub id: i64,
    pub full_name: String,
    pub name: String,
    pub language: Option<String>,
}

/// Tracked, cloned repos eligible to participate in an integration.
async fn graph(State(state): State<AppState>) -> ApiResult<Json<Vec<GraphNode>>> {
    let rows = sqlx::query_as::<_, Repo>(
        "SELECT * FROM repos WHERE tracked = 1 AND local_path IS NOT NULL ORDER BY full_name",
    )
    .fetch_all(&state.db)
    .await?;

    let nodes = rows
        .into_iter()
        .map(|r| GraphNode {
            id: r.id,
            full_name: r.full_name,
            name: r.name,
            language: r.language,
        })
        .collect();

    Ok(Json(nodes))
}

// ---------------------------------------------------------------------------
// POST /api/connections/integrate
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Serialize)]
pub struct IntegrateBody {
    pub source_id: i64,
    pub target_id: i64,
    pub instruction: String,
    /// Echo of the confirmation token issued on the first (challenged) attempt.
    /// Only consulted for the Codex principal (P19 destructive-action guard).
    #[serde(default)]
    pub confirm_token: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct IntegrateResult {
    pub ok: bool,
    pub branch: String,
    pub committed: bool,
    pub diff_stat: String,
    pub response_excerpt: String,
}

/// Build the plain-text prompt handed to Claude as a single argument.
fn build_prompt(source_path: &Path, source_full_name: &str, instruction: &str) -> String {
    format!(
        "You are working INSIDE the target repo. There is a source repo at {} \
         (its name: {}). TASK: {}. Read what you need from the source path and \
         implement it here in the target repo. Keep changes focused.",
        source_path.display(),
        source_full_name,
        instruction
    )
}

/// Trim an instruction down to a short, single-line commit suffix.
fn short_instruction(instruction: &str) -> String {
    let one_line: String = instruction.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() > 72 {
        let truncated: String = one_line.chars().take(69).collect();
        format!("{truncated}...")
    } else {
        one_line
    }
}

async fn integrate(
    State(state): State<AppState>,
    principal: Principal,
    Json(body): Json<IntegrateBody>,
) -> ApiResult<Json<IntegrateResult>> {
    // Codex destructive-action guard: integrate runs an arbitrary `claude` agent
    // inside the target repo and commits — the most powerful Codex-reachable HTTP
    // surface. User/Local are exempt; Codex must double-confirm. Hash the body with
    // `confirm_token` cleared so challenge and retry bind to the same hash.
    let supplied = body.confirm_token.clone();
    let mut for_hash = body;
    for_hash.confirm_token = None;
    let canonical = serde_json::to_vec(&for_hash).unwrap_or_default();
    let body = for_hash;
    auth_mw::require_confirmation_json(
        &state,
        &principal,
        "POST",
        "/api/connections/integrate",
        &canonical,
        supplied.as_deref(),
    )
    .await?;

    let instruction = body.instruction.trim().to_string();
    if instruction.is_empty() {
        return Err(AppError::msg("instruction is required"));
    }
    if body.source_id == body.target_id {
        return Err(AppError::msg(
            "source and target must be different repos",
        ));
    }

    let source = fetch_repo(&state, body.source_id).await?;
    let target = fetch_repo(&state, body.target_id).await?;

    let source_path = require_local_path(&source, "source")?;
    let target_path = require_local_path(&target, "target")?;

    // Land all changes on the staging branch — never the default branch.
    gitops::ensure_staging_branch(&target_path, STAGING_BRANCH)
        .await
        .map_err(AppError::Anyhow)?;

    let prompt = build_prompt(&source_path, &source.full_name, &instruction);

    let outcome = claude_runner::run_collect(&target_path, &prompt)
        .await
        .map_err(AppError::Anyhow)?;

    // Stage everything, then commit only if there is something to commit.
    let _ = git_capture(&target_path, &["add", "-A"]).await;

    let commit_msg = format!("RepoHub integrate: {}", short_instruction(&instruction));
    let (committed, _out, _err) =
        git_capture(&target_path, &["commit", "-m", &commit_msg]).await?;
    // `git commit` exits non-zero when nothing changed — treat that as a no-op.

    // Best-effort diff stat of staging vs the default branch.
    let diff_spec = format!("{}...{}", target.default_branch, STAGING_BRANCH);
    let diff_stat = match git_capture(&target_path, &["diff", "--stat", &diff_spec]).await {
        Ok((true, out, _)) => out.trim().to_string(),
        _ => String::new(),
    };

    // Log the run as a Prompt row (scope=repo, source=connection).
    sqlx::query(
        "INSERT INTO prompts (repo_id, scope, source, model, prompt, response, \
            tokens_in, tokens_out, created_at) \
         VALUES (?1, 'repo', 'connection', NULL, ?2, ?3, ?4, ?5, ?6)",
    )
    .bind(target.id)
    .bind(&prompt)
    .bind(&outcome.response)
    .bind(outcome.tokens_in)
    .bind(outcome.tokens_out)
    .bind(now_iso())
    .execute(&state.db)
    .await?;

    let response_excerpt: String = {
        let resp = outcome.response.trim();
        if resp.chars().count() > 500 {
            let truncated: String = resp.chars().take(500).collect();
            format!("{truncated}...")
        } else {
            resp.to_string()
        }
    };

    broadcast(
        &state,
        json!({
            "event": "connection_integrated",
            "source_id": source.id,
            "target_id": target.id,
            "branch": STAGING_BRANCH,
            "committed": committed,
        }),
    );

    Ok(Json(IntegrateResult {
        ok: true,
        branch: STAGING_BRANCH.to_string(),
        committed,
        diff_stat,
        response_excerpt,
    }))
}
