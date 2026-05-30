//! Claude Code subagents config API (P16).
//!
//! Subagents are Markdown files with YAML frontmatter (`name`, `description`,
//! `model`, `tools`) plus a body that serves as the agent's system prompt:
//!
//! - GLOBAL agents live in `~/.claude/agents/<name>.md` and are applied LIVE.
//!   Writes/deletes snapshot the `~/.claude` safety-net repo first (via
//!   `claude_fs::snapshot`).
//! - PER-REPO agents live in `<repo>/.claude/agents/<name>.md`. They are NOT
//!   applied live: changes land on the `repohub-staging` branch and are
//!   committed there for review via the Merge tab (never on `main`).
//!
//! All git invocations go through `tokio::process` (via `gitops`) with arguments
//! passed separately — never shell-interpolated. Secrets are never logged.

use std::path::{Path as FsPath, PathBuf};

use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tokio::process::Command;

use crate::claude_fs::{self, Agent};
use crate::error::{ApiResult, AppError};
use crate::models::Repo;
use crate::state::AppState;

/// Branch where all per-repo (non-live) changes land — never `main` directly.
const STAGING_BRANCH: &str = "repohub-staging";

/// Configuration scope: the live GLOBAL `~/.claude` layer or a PER-REPO layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    /// `~/.claude/agents` — applied live.
    Global,
    /// `<repo>/.claude/agents` — staged on `repohub-staging`.
    Repo,
}

pub fn router() -> Router<AppState> {
    Router::new().route(
        "/api/config/agents",
        get(list_agents).put(put_agent).delete(delete_agent),
    )
}

// ---------------------------------------------------------------------------
// Repo helpers (mirror settings_api.rs).
// ---------------------------------------------------------------------------

/// Fetch a repo row by id, erroring if it is missing.
async fn fetch_repo(state: &AppState, id: i64) -> ApiResult<Repo> {
    sqlx::query_as::<_, Repo>("SELECT * FROM repos WHERE id = ?1")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::msg(format!("repo {id} not found")))
}

/// Resolve the on-disk path of a cloned repo, erroring if it is not local.
fn repo_local_path(repo: &Repo) -> ApiResult<PathBuf> {
    let lp = repo
        .local_path
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::msg("repo is not cloned locally"))?;
    Ok(PathBuf::from(lp))
}

/// Resolve the agents directory for a repo: `<repo>/.claude/agents`. Verifies
/// the repo is cloned (has a `.git`) so per-repo writes can be staged.
async fn repo_agents_dir(state: &AppState, repo_id: i64) -> ApiResult<(PathBuf, PathBuf)> {
    let repo = fetch_repo(state, repo_id).await?;
    let root = repo_local_path(&repo)?;
    if !root.join(".git").exists() {
        return Err(AppError::msg("repo is not cloned locally"));
    }
    let dir = root.join(".claude").join("agents");
    Ok((root, dir))
}

/// Resolve the scope + optional repo id into the concrete agents directory and,
/// for repo scope, the repo working-tree root (used to stage commits).
async fn resolve_dir(
    state: &AppState,
    scope: Scope,
    repo_id: Option<i64>,
) -> ApiResult<(PathBuf, Option<PathBuf>)> {
    match scope {
        Scope::Global => Ok((claude_fs::claude_dir().join("agents"), None)),
        Scope::Repo => {
            let id = repo_id.ok_or_else(|| AppError::msg("repo_id is required for repo scope"))?;
            let (root, dir) = repo_agents_dir(state, id).await?;
            Ok((dir, Some(root)))
        }
    }
}

// ---------------------------------------------------------------------------
// GET /api/config/agents?scope=&repo_id=  — list agents in a scope.
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct AgentsQuery {
    pub scope: Scope,
    #[serde(default)]
    pub repo_id: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct AgentsResponse {
    pub scope: Scope,
    pub repo_id: Option<i64>,
    /// Absolute path of the directory the agents were read from (for the UI).
    pub dir: String,
    pub agents: Vec<Agent>,
}

async fn list_agents(
    State(state): State<AppState>,
    Query(q): Query<AgentsQuery>,
) -> ApiResult<Json<AgentsResponse>> {
    let (dir, root) = resolve_dir(&state, q.scope, q.repo_id).await?;

    // Global agents live live on disk; per-repo agents live on the staging
    // branch (our writes restore the original working tree), so read those from
    // staging rather than the live `.claude/agents` directory.
    let agents = match (q.scope, &root) {
        (Scope::Repo, Some(repo_root)) => {
            let lp = repo_root.to_string_lossy().to_string();
            read_staged_agents(&lp).await?
        }
        _ => claude_fs::read_agents(&dir).await?,
    };

    Ok(Json(AgentsResponse {
        scope: q.scope,
        repo_id: q.repo_id,
        dir: dir.to_string_lossy().to_string(),
        agents,
    }))
}

/// Read all agents under `.claude/agents` AS THEY STAND ON THE STAGING BRANCH,
/// without disturbing the working tree. Returns `[]` when the branch or the
/// directory is absent.
async fn read_staged_agents(lp: &str) -> ApiResult<Vec<Agent>> {
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
        return Ok(Vec::new());
    }

    // List the agent files tracked on staging under .claude/agents.
    let spec = format!("{STAGING_BRANCH}:.claude/agents");
    let tree = git(lp, &["ls-tree", "--name-only", &spec]).await;
    if !tree.ok {
        return Ok(Vec::new());
    }

    let mut agents = Vec::new();
    for line in tree.stdout.lines() {
        let file = line.trim();
        if !file.ends_with(".md") {
            continue;
        }
        let stem = file.trim_end_matches(".md");
        if stem.is_empty() {
            continue;
        }
        let show_spec = format!("{STAGING_BRANCH}:.claude/agents/{file}");
        let show = git(lp, &["show", &show_spec]).await;
        if show.ok {
            agents.push(claude_fs::parse_agent_public(stem, &show.stdout));
        }
    }
    agents.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(agents)
}

// ---------------------------------------------------------------------------
// PUT /api/config/agents  — create/update an agent.
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct PutAgentBody {
    pub scope: Scope,
    #[serde(default)]
    pub repo_id: Option<i64>,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    /// Comma/space-separated tool list as authored in the frontmatter.
    #[serde(default)]
    pub tools: Option<String>,
    /// The Markdown system prompt (agent body).
    #[serde(default)]
    pub system_prompt: String,
}

#[derive(Debug, Serialize)]
pub struct WriteAgentResponse {
    pub ok: bool,
    pub scope: Scope,
    pub repo_id: Option<i64>,
    pub name: String,
    /// Absolute path of the written file (for the UI).
    pub path: String,
    /// For repo scope, the branch the change was committed to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
}

/// Normalize blank optional strings to `None` so the frontmatter stays clean.
fn nonblank(v: Option<String>) -> Option<String> {
    v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

async fn put_agent(
    State(state): State<AppState>,
    Json(body): Json<PutAgentBody>,
) -> ApiResult<Json<WriteAgentResponse>> {
    if body.name.trim().is_empty() {
        return Err(AppError::msg("agent name is required"));
    }

    let agent = Agent {
        name: body.name.trim().to_string(),
        description: nonblank(body.description),
        model: nonblank(body.model),
        tools: nonblank(body.tools),
        body: body.system_prompt,
    };

    let (dir, root) = resolve_dir(&state, body.scope, body.repo_id).await?;
    let file_path = dir.join(format!("{}.md", agent.name));

    match body.scope {
        Scope::Global => {
            // Snapshot the safety-net repo BEFORE the live write.
            claude_fs::snapshot("repohub: before change").await?;
            claude_fs::write_agent(&dir, &agent).await?;
            Ok(Json(WriteAgentResponse {
                ok: true,
                scope: body.scope,
                repo_id: body.repo_id,
                name: agent.name,
                path: file_path.to_string_lossy().to_string(),
                branch: None,
            }))
        }
        Scope::Repo => {
            // PER-REPO: stage on repohub-staging, commit there (never live/main).
            let root = root.expect("repo scope yields a working-tree root");
            let rel = agent_rel_path(&agent.name);
            stage_and_commit(
                &root,
                &rel,
                &format!("RepoHub: update Claude Code agent {}", agent.name),
                |dir| {
                    let dir = dir.to_path_buf();
                    let agent = agent.clone();
                    async move { claude_fs::write_agent(&dir, &agent).await }
                },
            )
            .await?;
            Ok(Json(WriteAgentResponse {
                ok: true,
                scope: body.scope,
                repo_id: body.repo_id,
                name: agent.name,
                path: file_path.to_string_lossy().to_string(),
                branch: Some(STAGING_BRANCH.to_string()),
            }))
        }
    }
}

// ---------------------------------------------------------------------------
// DELETE /api/config/agents  — remove an agent.
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct DeleteAgentBody {
    pub scope: Scope,
    #[serde(default)]
    pub repo_id: Option<i64>,
    pub name: String,
}

#[derive(Debug, Serialize)]
pub struct DeleteAgentResponse {
    pub ok: bool,
    pub scope: Scope,
    pub repo_id: Option<i64>,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
}

async fn delete_agent(
    State(state): State<AppState>,
    Json(body): Json<DeleteAgentBody>,
) -> ApiResult<Json<DeleteAgentResponse>> {
    if body.name.trim().is_empty() {
        return Err(AppError::msg("agent name is required"));
    }
    let name = body.name.trim().to_string();

    let (dir, root) = resolve_dir(&state, body.scope, body.repo_id).await?;

    match body.scope {
        Scope::Global => {
            // Snapshot the safety-net repo BEFORE the live delete.
            claude_fs::snapshot("repohub: before change").await?;
            claude_fs::delete_agent(&dir, &name).await?;
            Ok(Json(DeleteAgentResponse {
                ok: true,
                scope: body.scope,
                repo_id: body.repo_id,
                name,
                branch: None,
            }))
        }
        Scope::Repo => {
            let root = root.expect("repo scope yields a working-tree root");
            let del_name = name.clone();
            let rel = agent_rel_path(&name);
            stage_and_commit(
                &root,
                &rel,
                &format!("RepoHub: remove Claude Code agent {name}"),
                |dir| {
                    let dir = dir.to_path_buf();
                    let del_name = del_name.clone();
                    async move { claude_fs::delete_agent(&dir, &del_name).await }
                },
            )
            .await?;
            Ok(Json(DeleteAgentResponse {
                ok: true,
                scope: body.scope,
                repo_id: body.repo_id,
                name,
                branch: Some(STAGING_BRANCH.to_string()),
            }))
        }
    }
}

// ---------------------------------------------------------------------------
// Per-repo staging helper.
//
// Per-repo agents must land on `repohub-staging` for review and NEVER be left
// live in the working tree, and a single agent edit must NEVER sweep unrelated
// dirty files into the commit. So we mirror the safe pattern in
// claude_config.rs: capture the current branch, switch to/create staging, write
// only the agent file, `git add` ONLY that path (never `add -A`), commit, then
// restore the original branch — even on error.
// ---------------------------------------------------------------------------

/// Relative path of a per-repo agent file, e.g. `.claude/agents/foo.md`.
fn agent_rel_path(name: &str) -> String {
    format!(".claude/agents/{name}.md")
}

/// Output of a spawned `git` command: success flag + trimmed streams.
struct CmdOut {
    ok: bool,
    stdout: String,
    stderr: String,
}

/// Run `git -C <cwd> <args>`, capturing both streams. Arguments are passed
/// separately — never through a shell.
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

/// Run a filesystem mutation against `<root>/.claude/agents` on the staging
/// branch and commit ONLY `rel` there, restoring the original branch afterward.
///
/// `op` performs the write/delete against the agents directory; `rel` is the
/// repo-relative path of the affected agent file to stage. The commit is a
/// best-effort no-op when nothing changed (e.g. deleting an absent agent).
async fn stage_and_commit<F, Fut>(
    root: &FsPath,
    rel: &str,
    message: &str,
    op: F,
) -> ApiResult<()>
where
    F: FnOnce(&FsPath) -> Fut,
    Fut: std::future::Future<Output = ApiResult<()>>,
{
    let lp = root.to_string_lossy().to_string();

    // Remember the branch we are on so we can restore the working tree exactly.
    let original = git(&lp, &["rev-parse", "--abbrev-ref", "HEAD"]).await;
    let original_branch = if original.ok && !original.stdout.is_empty() {
        Some(original.stdout.clone())
    } else {
        None
    };

    let result = stage_and_commit_inner(&lp, root, rel, message, op).await;

    // Best-effort restore of the working tree to the original branch.
    if let Some(branch) = &original_branch {
        let _ = git(&lp, &["checkout", branch]).await;
    }

    result
}

async fn stage_and_commit_inner<F, Fut>(
    lp: &str,
    root: &FsPath,
    rel: &str,
    message: &str,
    op: F,
) -> ApiResult<()>
where
    F: FnOnce(&FsPath) -> Fut,
    Fut: std::future::Future<Output = ApiResult<()>>,
{
    let exists = git(
        lp,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{STAGING_BRANCH}"),
        ],
    )
    .await;

    // Prefer `checkout <name>` when staging exists (append to staged work) over
    // `checkout -B` (which would reset it onto the current HEAD).
    let checkout = if exists.ok {
        git(lp, &["checkout", STAGING_BRANCH]).await
    } else {
        git(lp, &["checkout", "-B", STAGING_BRANCH]).await
    };
    if !checkout.ok {
        let detail = if checkout.stderr.is_empty() {
            checkout.stdout
        } else {
            checkout.stderr
        };
        return Err(AppError::msg(format!(
            "failed to switch to staging branch: {detail}"
        )));
    }

    let dir = root.join(".claude").join("agents");
    op(&dir).await?;

    // Stage ONLY the affected agent file, never `add -A`.
    let add = git(lp, &["add", "--", rel]).await;
    if !add.ok {
        let detail = if add.stderr.is_empty() {
            add.stdout
        } else {
            add.stderr
        };
        return Err(AppError::msg(format!("failed to stage agent: {detail}")));
    }

    // `git commit` exits non-zero when there is nothing to commit; treat that
    // as a successful no-op.
    let _ = git(lp, &["commit", "-m", message]).await;
    Ok(())
}
