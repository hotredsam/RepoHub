//! P16 — Claude Code runtime configuration API.
//!
//! Two layers:
//! - **GLOBAL** (`scope='global'`): applied LIVE to the user's real
//!   `~/.claude/settings.json` via [`claude_fs`]. Every write snapshots the
//!   `~/.claude` git safety-net first and MERGES the patch (never blind
//!   overwrite). Init/commit happen LAZILY — only when the user makes a change.
//! - **PER-REPO** override (`<repo>/.claude/settings.json`): an optional layer
//!   that inherits the global default when unset. Per-repo runtime overrides do
//!   NOT land live in the working tree — they are committed to the per-repo
//!   `repohub-staging` branch for review via the Merge tab.
//!
//! This module is intentionally separate from `settings_api` (which owns
//! `/api/settings`, `/api/preferences`, `/api/settings/suggest`). It does not
//! duplicate or modify that surface.
//!
//! IMPORTANT: per the RepoHub decisions we never set a default `model` — it is
//! left unset so Claude Code's own default is inherited.
//!
//! HARD RULES honoured here: git runs via `tokio::process` with arguments passed
//! separately (never shell-interpolated); secrets are never logged.

use std::path::PathBuf;

use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::process::Command;

use crate::claude_fs::{self, HistoryEntry};
use crate::error::{ApiResult, AppError};
use crate::models::Repo;
use crate::state::AppState;

/// Per-repo branch where override changes land for review (never live).
const STAGING_BRANCH: &str = "repohub-staging";

/// Relative path of a repo's per-repo Claude settings file.
const REPO_SETTINGS_REL: &str = ".claude/settings.json";

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/config/global",
            get(get_global_config).put(put_global_config),
        )
        .route("/api/config/history", get(get_history))
        .route("/api/config/revert", post(post_revert))
        .route(
            "/api/config/repo/:id",
            get(get_repo_config).put(put_repo_config),
        )
}

// ---------------------------------------------------------------------------
// Normalized view of well-known Claude Code knobs.
// ---------------------------------------------------------------------------

/// A normalized, UI-friendly projection of the well-known knobs in a
/// `settings.json`. Every field is best-effort: keys that are absent or shaped
/// unexpectedly simply read as empty/None so the UI can show inherit-vs-override
/// cleanly. The raw settings object is always returned alongside this view.
#[derive(Debug, Default, Serialize)]
pub struct NormalizedConfig {
    /// Selected model, if explicitly set. (We never write a default here.)
    pub model: Option<String>,
    /// `systemPrompt` string, if present.
    pub system_prompt: Option<String>,
    /// `appendSystemPrompt` / `systemPrompt.append` text, if present.
    pub append_system_prompt: Option<String>,
    /// Enabled skills (names) — best-effort across a few shapes.
    pub skills: Vec<String>,
    /// `permissions.allow`.
    pub permissions_allow: Vec<String>,
    /// `permissions.deny`.
    pub permissions_deny: Vec<String>,
    /// `permissions.ask`.
    pub permissions_ask: Vec<String>,
    /// Allowed tools (`tools.allow` / `allowedTools`).
    pub tools_allow: Vec<String>,
    /// Denied tools (`tools.deny` / `disallowedTools`).
    pub tools_deny: Vec<String>,
    /// `loops` block, passed through untouched (free-form).
    pub loops: Option<Value>,
    /// `workflows` block, passed through untouched (free-form).
    pub workflows: Option<Value>,
}

/// Pull a string field from an object by key.
fn get_str(obj: &Value, key: &str) -> Option<String> {
    obj.get(key)
        .and_then(Value::as_str)
        .map(|s| s.to_string())
}

/// Coerce a JSON value into a `Vec<String>`:
/// - array of strings -> those strings
/// - array of objects -> their `name`/`id` field if present
/// - object -> keys whose value is truthy (e.g. `{ "skill": true }`)
/// - single string -> a one-element vec
fn as_string_list(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::Array(arr)) => arr
            .iter()
            .filter_map(|item| match item {
                Value::String(s) => Some(s.clone()),
                Value::Object(_) => get_str(item, "name").or_else(|| get_str(item, "id")),
                _ => None,
            })
            .collect(),
        Some(Value::Object(map)) => map
            .iter()
            .filter_map(|(k, val)| match val {
                Value::Bool(true) => Some(k.clone()),
                // Treat any non-false, non-null value as "enabled".
                Value::Bool(false) | Value::Null => None,
                _ => Some(k.clone()),
            })
            .collect(),
        Some(Value::String(s)) => vec![s.clone()],
        _ => Vec::new(),
    }
}

/// First non-empty list among several candidate dotted paths in `obj`.
///
/// Each candidate is a `.`-separated path (e.g. `"permissions.allow"`). None of
/// the well-known keys contain a literal dot, so splitting on `.` is safe.
fn first_list(obj: &Value, candidates: &[&str]) -> Vec<String> {
    for path in candidates {
        let mut cur = obj;
        let mut ok = true;
        for seg in path.split('.') {
            match cur.get(seg) {
                Some(next) => cur = next,
                None => {
                    ok = false;
                    break;
                }
            }
        }
        if ok {
            let list = as_string_list(Some(cur));
            if !list.is_empty() {
                return list;
            }
        }
    }
    Vec::new()
}

/// Build the normalized view from a raw settings object. Accepts a few common
/// shapes for each knob so it stays robust to how the file was authored.
fn normalize(settings: &Value) -> NormalizedConfig {
    let mut out = NormalizedConfig {
        model: get_str(settings, "model"),
        ..Default::default()
    };

    // systemPrompt may be a plain string, or an object { value/text, append }.
    match settings.get("systemPrompt") {
        Some(Value::String(s)) => out.system_prompt = Some(s.clone()),
        Some(obj @ Value::Object(_)) => {
            out.system_prompt = get_str(obj, "value").or_else(|| get_str(obj, "text"));
            out.append_system_prompt = get_str(obj, "append");
        }
        _ => {}
    }
    // Standalone append key (camelCase) also supported.
    if out.append_system_prompt.is_none() {
        out.append_system_prompt = get_str(settings, "appendSystemPrompt");
    }

    out.skills = first_list(settings, &["skills.enabled", "skills"]);

    out.permissions_allow = first_list(settings, &["permissions.allow"]);
    out.permissions_deny = first_list(settings, &["permissions.deny"]);
    out.permissions_ask = first_list(settings, &["permissions.ask"]);

    out.tools_allow = first_list(settings, &["tools.allow", "allowedTools"]);
    out.tools_deny = first_list(settings, &["tools.deny", "disallowedTools"]);

    out.loops = settings.get("loops").cloned();
    out.workflows = settings.get("workflows").cloned();

    out
}

// ---------------------------------------------------------------------------
// GET /api/config/global
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct GlobalConfigResponse {
    /// Raw `~/.claude/settings.json` (or `{}` if absent).
    pub settings: Value,
    /// Normalized well-known knobs derived from `settings`.
    pub normalized: NormalizedConfig,
    /// Effective values. At the GLOBAL layer there is nothing above to inherit
    /// from, so the effective view equals the normalized view; it is surfaced
    /// explicitly so the per-repo layer (which inherits global) can use the same
    /// shape on the frontend.
    pub effective: NormalizedConfig,
}

async fn get_global_config(State(_state): State<AppState>) -> ApiResult<Json<GlobalConfigResponse>> {
    let settings = claude_fs::read_settings().await?;
    let normalized = normalize(&settings);
    let effective = normalize(&settings);
    Ok(Json(GlobalConfigResponse {
        settings,
        normalized,
        effective,
    }))
}

// ---------------------------------------------------------------------------
// PUT /api/config/global  — merge a patch LIVE into ~/.claude/settings.json.
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct GlobalPatchBody {
    /// A partial settings object deep-merged into the live file.
    pub patch: Value,
}

async fn put_global_config(
    State(_state): State<AppState>,
    Json(body): Json<GlobalPatchBody>,
) -> ApiResult<Json<GlobalConfigResponse>> {
    if !body.patch.is_object() {
        return Err(AppError::msg("patch must be a JSON object"));
    }

    // write_settings_merge snapshots the ~/.claude git safety-net first, then
    // deep-merges the patch into the existing file (never a blind overwrite).
    claude_fs::write_settings_merge(&body.patch).await?;

    let settings = claude_fs::read_settings().await?;
    let normalized = normalize(&settings);
    let effective = normalize(&settings);
    Ok(Json(GlobalConfigResponse {
        settings,
        normalized,
        effective,
    }))
}

// ---------------------------------------------------------------------------
// GET /api/config/history  &  POST /api/config/revert
// ---------------------------------------------------------------------------

async fn get_history(State(_state): State<AppState>) -> ApiResult<Json<Vec<HistoryEntry>>> {
    Ok(Json(claude_fs::history().await))
}

#[derive(Debug, Deserialize)]
pub struct RevertBody {
    pub hash: String,
}

#[derive(Debug, Serialize)]
pub struct RevertResponse {
    pub ok: bool,
    /// The settings as they stand after the revert.
    pub settings: Value,
    pub normalized: NormalizedConfig,
}

async fn post_revert(
    State(_state): State<AppState>,
    Json(body): Json<RevertBody>,
) -> ApiResult<Json<RevertResponse>> {
    let hash = body.hash.trim();
    if hash.is_empty() {
        return Err(AppError::msg("revert hash must not be empty"));
    }
    // The hash is passed through to git as a separate argument by claude_fs;
    // reject obvious option-injection just in case it is ever logged/echoed.
    if hash.starts_with('-') {
        return Err(AppError::msg("invalid revert hash"));
    }

    claude_fs::revert(hash).await?;

    let settings = claude_fs::read_settings().await?;
    let normalized = normalize(&settings);
    Ok(Json(RevertResponse {
        ok: true,
        settings,
        normalized,
    }))
}

// ---------------------------------------------------------------------------
// Per-repo overrides — committed to the repohub-staging branch (NOT live).
// ---------------------------------------------------------------------------

/// Output of a spawned `git` command: success flag + trimmed streams. Mirrors
/// the convention used in `merge.rs` so the per-repo flow reads consistently.
struct CmdOut {
    ok: bool,
    stdout: String,
    stderr: String,
}

/// Run `git -C <cwd> <args>`, capturing both streams. Arguments are passed
/// separately — never through a shell. A spawn failure folds into a non-ok
/// result carrying the error in `stderr`.
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

/// Fetch a repo row by id, erroring if it is missing.
async fn fetch_repo(state: &AppState, id: i64) -> ApiResult<Repo> {
    sqlx::query_as::<_, Repo>("SELECT * FROM repos WHERE id = ?1")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::msg(format!("repo {id} not found")))
}

/// Resolve a cloned repo's on-disk path, erroring if it is not local.
fn repo_local_path(repo: &Repo) -> ApiResult<PathBuf> {
    let lp = repo
        .local_path
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::msg("repo is not cloned locally"))?;
    let path = PathBuf::from(lp);
    if !path.join(".git").exists() {
        return Err(AppError::msg("repo is not cloned locally"));
    }
    Ok(path)
}

#[derive(Debug, Serialize)]
pub struct RepoConfigResponse {
    pub repo_id: i64,
    /// The repo's per-repo override settings as they stand on the staging
    /// branch (or `{}` if none exist yet). This is the OVERRIDE layer only —
    /// it inherits the global default for anything left unset.
    pub settings: Value,
    /// Whether a per-repo override file currently exists on the staging branch.
    pub exists: bool,
    /// Normalized view of the override layer.
    pub normalized: NormalizedConfig,
    /// Branch the override lives on (for review via the Merge tab).
    pub branch: String,
}

/// Read the per-repo `.claude/settings.json` AS IT STANDS ON THE STAGING BRANCH
/// without disturbing the working tree, via `git show <branch>:<path>`. Returns
/// `{}` when the branch or file is absent.
async fn read_repo_override(lp: &str) -> ApiResult<Value> {
    // If the staging branch does not exist yet there is no override.
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
        return Ok(json!({}));
    }

    let spec = format!("{STAGING_BRANCH}:{REPO_SETTINGS_REL}");
    let show = git(lp, &["show", &spec]).await;
    if !show.ok {
        // Branch exists but the file does not — no override yet.
        return Ok(json!({}));
    }
    if show.stdout.trim().is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(&show.stdout)
        .map_err(|e| AppError::msg(format!("failed to parse {REPO_SETTINGS_REL}: {e}")))
}

async fn get_repo_config(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<RepoConfigResponse>> {
    let repo = fetch_repo(&state, id).await?;
    let path = repo_local_path(&repo)?;
    let lp = path.to_string_lossy().to_string();

    let settings = read_repo_override(&lp).await?;
    let exists = settings.as_object().map(|m| !m.is_empty()).unwrap_or(false);
    let normalized = normalize(&settings);

    Ok(Json(RepoConfigResponse {
        repo_id: id,
        settings,
        exists,
        normalized,
        branch: STAGING_BRANCH.to_string(),
    }))
}

#[derive(Debug, Deserialize)]
pub struct RepoPatchBody {
    /// A partial settings object deep-merged into the repo's override file.
    pub patch: Value,
}

#[derive(Debug, Serialize)]
pub struct RepoPutResponse {
    pub repo_id: i64,
    pub ok: bool,
    pub committed: bool,
    pub branch: String,
    /// The override settings after the merge.
    pub settings: Value,
    pub normalized: NormalizedConfig,
}

async fn put_repo_config(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<RepoPatchBody>,
) -> ApiResult<Json<RepoPutResponse>> {
    if !body.patch.is_object() {
        return Err(AppError::msg("patch must be a JSON object"));
    }

    let repo = fetch_repo(&state, id).await?;
    let path = repo_local_path(&repo)?;
    let lp = path.to_string_lossy().to_string();

    // Remember the branch we are on so we can restore the working tree exactly
    // as we found it — per-repo overrides must NOT land live.
    let original = git(&lp, &["rev-parse", "--abbrev-ref", "HEAD"]).await;
    let original_branch = if original.ok && !original.stdout.is_empty() {
        Some(original.stdout.clone())
    } else {
        None
    };

    // 1) Merge the patch into the override as it stands on the staging branch.
    let mut merged = read_repo_override(&lp).await?;
    if !merged.is_object() {
        merged = json!({});
    }
    claude_fs::deep_merge(&mut merged, &body.patch);

    let pretty = serde_json::to_string_pretty(&merged)
        .map_err(|e| AppError::msg(format!("failed to serialize override: {e}")))?;

    // 2) Switch to (or create) the staging branch, write the file, commit, then
    //    restore the original branch. Wrapped so we always try to restore even
    //    on an intermediate failure.
    let committed = stage_repo_override(&lp, &pretty).await;

    // 3) Best-effort restore of the working tree to the original branch.
    if let Some(branch) = &original_branch {
        let _ = git(&lp, &["checkout", branch]).await;
    }

    let committed = committed?;

    Ok(Json(RepoPutResponse {
        repo_id: id,
        ok: true,
        committed,
        branch: STAGING_BRANCH.to_string(),
        settings: merged.clone(),
        normalized: normalize(&merged),
    }))
}

/// Check out the staging branch, write the override file, and commit it there.
/// Returns whether a commit was actually created (false when the content was
/// unchanged — `git commit` is a no-op). The caller restores the prior branch.
async fn stage_repo_override(lp: &str, contents: &str) -> ApiResult<bool> {
    // `checkout -B` creates the branch if missing or resets it onto the current
    // HEAD if it exists; matches gitops::ensure_staging_branch semantics, but we
    // prefer `checkout <name>` if the branch already exists so we APPEND to the
    // staged work rather than discarding it.
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

    let checkout = if exists.ok {
        git(lp, &["checkout", STAGING_BRANCH]).await
    } else {
        // Create the staging branch from the current HEAD.
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

    // Write the override file (create `.claude/` as needed).
    let dest = PathBuf::from(lp).join(REPO_SETTINGS_REL);
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(&dest, contents.as_bytes()).await?;

    // Stage only the override file, then commit. `git commit` exits non-zero
    // when there is nothing to commit; treat that as a successful no-op.
    let add = git(lp, &["add", REPO_SETTINGS_REL]).await;
    if !add.ok {
        let detail = if add.stderr.is_empty() {
            add.stdout
        } else {
            add.stderr
        };
        return Err(AppError::msg(format!("failed to stage override: {detail}")));
    }

    let commit = git(
        lp,
        &["commit", "-m", "RepoHub: per-repo Claude config override"],
    )
    .await;
    Ok(commit.ok)
}
