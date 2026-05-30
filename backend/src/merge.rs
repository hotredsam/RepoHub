//! Merge model backend (P9).
//!
//! The chosen model is OPEN + AUTO-MERGE IMMEDIATELY: staged work on the
//! per-repo `repohub-staging` branch is pushed, a PR is opened against the
//! repo's default branch, and that PR is merged right away.
//!
//! IMPORTANT: `POST /api/merge/finish` merges directly to the repo's DEFAULT
//! BRANCH (e.g. `main`). It does not stop at "PR opened"; it completes the
//! merge. Branch-protection rules or required checks can cause `gh pr merge`
//! to fail — when that happens we report `gh`'s stderr verbatim and never
//! fabricate success.

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::PathBuf;
use tokio::process::Command;

use crate::error::{ApiResult, AppError};
use crate::models::Repo;
use crate::state::AppState;

/// Per-repo staging branch that Claude changes land on. Kept in sync with
/// `repos::staging_branch()`.
const STAGING_BRANCH: &str = "repohub-staging";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/merge/pending", get(pending))
        .route("/api/merge/finish", post(finish))
}

fn broadcast(state: &AppState, event: serde_json::Value) {
    let _ = state.status_tx.send(event.to_string());
}

// ---------------------------------------------------------------------------
// Small process helpers (args passed separately — never through a shell).
// ---------------------------------------------------------------------------

/// Output of a spawned command: success flag plus trimmed stdout/stderr.
struct CmdOut {
    ok: bool,
    stdout: String,
    stderr: String,
}

/// Run `git -C <cwd> <args>` capturing both streams. A spawn failure is folded
/// into a non-ok result with the error in `stderr`.
async fn git(cwd: &str, args: &[&str]) -> CmdOut {
    let mut full: Vec<&str> = Vec::with_capacity(args.len() + 2);
    full.push("-C");
    full.push(cwd);
    full.extend_from_slice(args);
    match Command::new("git").args(&full).output().await {
        Ok(o) => CmdOut {
            ok: o.status.success(),
            stdout: String::from_utf8_lossy(&o.stdout).trim().to_string(),
            stderr: String::from_utf8_lossy(&o.stderr).trim().to_string(),
        },
        Err(e) => CmdOut {
            ok: false,
            stdout: String::new(),
            stderr: format!("failed to spawn git {:?}: {e}", args),
        },
    }
}

/// Run `gh <args>` capturing both streams. A spawn failure is folded into a
/// non-ok result with the error in `stderr`.
async fn gh(args: &[&str]) -> CmdOut {
    match Command::new("gh").args(args).output().await {
        Ok(o) => CmdOut {
            ok: o.status.success(),
            stdout: String::from_utf8_lossy(&o.stdout).trim().to_string(),
            stderr: String::from_utf8_lossy(&o.stderr).trim().to_string(),
        },
        Err(e) => CmdOut {
            ok: false,
            stdout: String::new(),
            stderr: format!("failed to spawn gh {:?}: {e}", args),
        },
    }
}

// ---------------------------------------------------------------------------
// GET /api/merge/pending
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct PendingEntry {
    pub repo_id: i64,
    pub full_name: String,
    pub default_branch: String,
    pub commits_ahead: i64,
}

/// Tracked + cloned repos whose local `repohub-staging` branch is ahead of the
/// default branch (`git rev-list --count <default>..repohub-staging`). Repos
/// missing the staging branch report 0 and are omitted from the response.
async fn pending(State(state): State<AppState>) -> ApiResult<Json<Vec<PendingEntry>>> {
    let repos = sqlx::query_as::<_, Repo>(
        "SELECT * FROM repos WHERE tracked = 1 AND local_path IS NOT NULL ORDER BY full_name",
    )
    .fetch_all(&state.db)
    .await?;

    let mut out = Vec::new();
    for repo in repos {
        let Some(lp) = repo.local_path.as_ref() else {
            continue;
        };
        if !PathBuf::from(lp).join(".git").exists() {
            continue;
        }

        // Skip repos that don't have the staging branch at all (commits_ahead = 0).
        let has_branch = git(
            lp,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/heads/{STAGING_BRANCH}"),
            ],
        )
        .await;
        if !has_branch.ok {
            continue;
        }

        let range = format!("{}..{}", repo.default_branch, STAGING_BRANCH);
        let count = git(lp, &["rev-list", "--count", &range]).await;
        let commits_ahead: i64 = if count.ok {
            count.stdout.parse().unwrap_or(0)
        } else {
            0
        };

        if commits_ahead > 0 {
            out.push(PendingEntry {
                repo_id: repo.id,
                full_name: repo.full_name.clone(),
                default_branch: repo.default_branch.clone(),
                commits_ahead,
            });
        }
    }

    Ok(Json(out))
}

// ---------------------------------------------------------------------------
// POST /api/merge/finish
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct FinishBody {
    pub repo_ids: Vec<i64>,
    /// "squash" (default) | "merge".
    #[serde(default)]
    pub strategy: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct FinishResult {
    pub repo_id: i64,
    pub full_name: String,
    pub pr_url: Option<String>,
    pub merged: bool,
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct FinishResponse {
    pub summary: String,
    pub note: String,
    pub results: Vec<FinishResult>,
}

/// Look up an existing open PR for `repohub-staging` -> default branch, if any.
/// Returns its URL on success. Absence of a PR is reported as `Ok(None)`.
async fn find_existing_pr(full_name: &str) -> Result<Option<String>, String> {
    let out = gh(&[
        "pr",
        "list",
        "-R",
        full_name,
        "--head",
        STAGING_BRANCH,
        "--state",
        "open",
        "--json",
        "url",
        "--limit",
        "1",
    ])
    .await;
    if !out.ok {
        return Err(out.stderr);
    }
    // `gh pr list --json url` yields e.g. `[{"url":"https://..."}]` or `[]`.
    let parsed: serde_json::Value =
        serde_json::from_str(&out.stdout).map_err(|e| format!("parsing gh pr list JSON: {e}"))?;
    let url = parsed
        .as_array()
        .and_then(|a| a.first())
        .and_then(|o| o.get("url"))
        .and_then(|u| u.as_str())
        .map(|s| s.to_string());
    Ok(url)
}

/// Push staging, open (or reuse) a PR, then merge it to the default branch for a
/// single repo. Errors are returned as `Err(String)` carrying verbatim stderr.
async fn finish_one(
    repo: &Repo,
    merge_flag: &str,
) -> Result<(String, bool), (Option<String>, String)> {
    let Some(lp) = repo.local_path.as_ref() else {
        return Err((None, "repo is not cloned locally".to_string()));
    };
    if !PathBuf::from(lp).join(".git").exists() {
        return Err((None, "local clone is missing a .git directory".to_string()));
    }

    // 1) Push the staging branch and set upstream.
    let push = git(lp, &["push", "-u", "origin", STAGING_BRANCH]).await;
    if !push.ok {
        let detail = if push.stderr.is_empty() {
            push.stdout
        } else {
            push.stderr
        };
        return Err((None, format!("git push failed: {detail}")));
    }

    // 2) Open a PR, or reuse one that already exists.
    let create = gh(&[
        "pr",
        "create",
        "-R",
        &repo.full_name,
        "--base",
        &repo.default_branch,
        "--head",
        STAGING_BRANCH,
        "--title",
        "RepoHub: staged changes",
        "--body",
        "Merged via RepoHub",
    ])
    .await;

    let pr_url = if create.ok {
        // `gh pr create` prints the PR URL on stdout.
        create.stdout.clone()
    } else {
        // A pre-existing PR makes `gh pr create` fail; detect & reuse it.
        match find_existing_pr(&repo.full_name).await {
            Ok(Some(url)) => url,
            Ok(None) => {
                let detail = if create.stderr.is_empty() {
                    create.stdout
                } else {
                    create.stderr
                };
                return Err((None, format!("gh pr create failed: {detail}")));
            }
            Err(list_err) => {
                let detail = if create.stderr.is_empty() {
                    create.stdout
                } else {
                    create.stderr
                };
                return Err((
                    None,
                    format!("gh pr create failed: {detail}; reuse lookup failed: {list_err}"),
                ));
            }
        }
    };

    // 3) Merge the PR into the default branch (auto-merge immediately).
    let merge = gh(&[
        "pr",
        "merge",
        &pr_url,
        "-R",
        &repo.full_name,
        merge_flag,
        "--delete-branch",
    ])
    .await;

    if merge.ok {
        // Sync the LOCAL clone so `pending` reflects reality on the next load.
        // The merge advanced the *remote* default branch and deleted the
        // *remote* staging branch, but our local refs are untouched: the local
        // default branch is still behind and local `repohub-staging` still
        // carries its commits, so `git rev-list --count <default>..staging`
        // would keep reporting the repo as pending forever. Fast-forward the
        // local default branch from origin and drop the local staging branch.
        sync_local_after_merge(lp, &repo.default_branch).await;
        Ok((pr_url, true))
    } else {
        let detail = if merge.stderr.is_empty() {
            merge.stdout
        } else {
            merge.stderr
        };
        // PR exists but the merge failed (e.g. branch protection) — report it
        // verbatim and surface the PR URL so the user can act on it.
        Err((Some(pr_url), format!("gh pr merge failed: {detail}")))
    }
}

/// After a successful merge, bring the LOCAL clone in line with the remote so
/// `pending` recomputes to 0. Best-effort: every step is non-fatal — a failure
/// here does not undo the merge that already happened on the remote, it only
/// means the repo may still appear pending until the next manual sync.
async fn sync_local_after_merge(lp: &str, default_branch: &str) {
    // 1) Refresh remote-tracking refs; this also prunes the deleted remote
    //    staging branch.
    let _ = git(lp, &["fetch", "--prune", "origin"]).await;

    // 2) Advance the local default branch to match origin. If it is the
    //    currently checked-out branch we fast-forward it; otherwise we update
    //    the ref directly without needing a checkout.
    let current = git(lp, &["rev-parse", "--abbrev-ref", "HEAD"]).await;
    let on_default = current.ok && current.stdout == default_branch;
    if on_default {
        // Fast-forward only — never fabricate a merge commit locally.
        let _ = git(lp, &["merge", "--ff-only", &format!("origin/{default_branch}")]).await;
    } else {
        // Move the local branch ref to the remote-tracking ref. Safe because we
        // are not on it.
        let _ = git(
            lp,
            &[
                "update-ref",
                &format!("refs/heads/{default_branch}"),
                &format!("refs/remotes/origin/{default_branch}"),
            ],
        )
        .await;
    }

    // 3) Drop the now-merged local staging branch so it no longer shows ahead.
    //    A squash merge leaves its commits unreachable from the default branch,
    //    so force-delete (-D) rather than -d. If staging is somehow the checked
    //    out branch, deletion fails harmlessly and the branch simply remains.
    let _ = git(lp, &["branch", "-D", STAGING_BRANCH]).await;
}

async fn finish(
    State(state): State<AppState>,
    Json(body): Json<FinishBody>,
) -> ApiResult<Json<FinishResponse>> {
    let strategy = body.strategy.as_deref().unwrap_or("squash").to_lowercase();
    let merge_flag = match strategy.as_str() {
        "merge" => "--merge",
        "squash" => "--squash",
        other => {
            return Err(AppError::msg(format!(
                "unknown merge strategy '{other}' (expected 'squash' or 'merge')"
            )))
        }
    };

    let mut results = Vec::with_capacity(body.repo_ids.len());

    for id in &body.repo_ids {
        let repo = match sqlx::query_as::<_, Repo>("SELECT * FROM repos WHERE id = ?1")
            .bind(id)
            .fetch_optional(&state.db)
            .await?
        {
            Some(r) => r,
            None => {
                results.push(FinishResult {
                    repo_id: *id,
                    full_name: String::new(),
                    pr_url: None,
                    merged: false,
                    error: Some(format!("repo {id} not found")),
                });
                continue;
            }
        };

        match finish_one(&repo, merge_flag).await {
            Ok((pr_url, merged)) => {
                results.push(FinishResult {
                    repo_id: repo.id,
                    full_name: repo.full_name.clone(),
                    pr_url: Some(pr_url),
                    merged,
                    error: None,
                });
            }
            Err((pr_url, error)) => {
                results.push(FinishResult {
                    repo_id: repo.id,
                    full_name: repo.full_name.clone(),
                    pr_url,
                    merged: false,
                    error: Some(error),
                });
            }
        }
    }

    let merged_count = results.iter().filter(|r| r.merged).count();
    let total = results.len();
    let summary = format!(
        "Merged {merged_count}/{total} repo(s) to their default branch via {strategy}."
    );
    let note = "This endpoint merges the 'repohub-staging' branch directly into each \
        repo's DEFAULT branch (e.g. main). Failures (such as branch protection) report \
        gh's stderr verbatim and are never reported as success."
        .to_string();

    broadcast(
        &state,
        json!({ "event": "merge_finished", "merged": merged_count, "total": total }),
    );

    Ok(Json(FinishResponse {
        summary,
        note,
        results,
    }))
}
