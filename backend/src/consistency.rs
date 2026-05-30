//! Consistency / styling engine (P8).
//!
//! Lets the user pick a "golden" repo whose conventions become the template for
//! a set of target repos. Conventions are grouped into fixed *bundles*:
//!
//! - `format` — code-formatting + lint config (prettier, rustfmt, eslint,
//!   `.editorconfig`). Applied **mechanically** by copying the files that exist
//!   in the golden repo into each target (overwrite).
//! - `meta`   — repo scaffolding (README, LICENSE, `.gitignore`, `.github/*`).
//!   Also applied mechanically by file copy.
//! - `tokens` — design tokens (css/ts). Applied by asking the local `claude`
//!   CLI to align the target to the golden repo.
//! - `claude` — `CLAUDE.md` / `AGENTS.md` agent guidance. Also applied via the
//!   `claude` CLI so prose is merged intelligently rather than blindly clobbered.
//!
//! All changes land on the per-repo `repohub-staging` branch and are committed
//! there; nothing touches `main`. Git/`gh`/`claude` are invoked via separate
//! process args (never through a shell).

use axum::extract::State;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::{Path, PathBuf};

use crate::claude_runner;
use crate::error::{ApiResult, AppError};
use crate::gitops;
use crate::models::Repo;
use crate::state::AppState;

/// Scope used for application-wide settings (matches `settings_api`).
const GLOBAL_SCOPE: &str = "global";
/// Setting key under which the golden repo id is stored.
const GOLDEN_KEY: &str = "golden_repo_id";
/// Branch all consistency changes land on (never `main`).
const STAGING_BRANCH: &str = "repohub-staging";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/consistency/config", get(get_config))
        .route("/api/consistency/golden", put(put_golden))
        .route("/api/consistency/apply", post(apply))
}

// ---------------------------------------------------------------------------
// Bundle catalog
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct Bundle {
    pub key: &'static str,
    pub label: &'static str,
    pub description: &'static str,
}

/// The fixed set of bundles surfaced to the UI.
fn bundles() -> Vec<Bundle> {
    vec![
        Bundle {
            key: "format",
            label: "Formatting & Lint",
            description: "prettier, rustfmt, .editorconfig, eslint config",
        },
        Bundle {
            key: "meta",
            label: "Repo Meta",
            description: "README scaffold, LICENSE, .gitignore, .github templates",
        },
        Bundle {
            key: "tokens",
            label: "Design Tokens",
            description: "design tokens (css / ts) aligned to the golden repo",
        },
        Bundle {
            key: "claude",
            label: "Claude Guidance",
            description: "CLAUDE.md / AGENTS.md aligned to the golden repo",
        },
    ]
}

/// Relative paths/globs copied for the `format` bundle when present in golden.
const FORMAT_FILES: &[&str] = &[
    ".editorconfig",
    ".prettierrc",
    ".prettierrc.json",
    ".prettierrc.js",
    ".prettierrc.cjs",
    ".prettierrc.yaml",
    ".prettierrc.yml",
    ".prettierignore",
    "prettier.config.js",
    "prettier.config.cjs",
    "rustfmt.toml",
    ".rustfmt.toml",
    ".eslintrc",
    ".eslintrc.json",
    ".eslintrc.js",
    ".eslintrc.cjs",
    ".eslintrc.yaml",
    ".eslintrc.yml",
    "eslint.config.js",
    "eslint.config.mjs",
    "eslint.config.cjs",
    ".eslintignore",
];

/// Relative paths copied for the `meta` bundle when present in golden. Files
/// here are individual entries; `.github` is handled as a recursive directory
/// copy separately.
const META_FILES: &[&str] = &[
    "README.md",
    "LICENSE",
    "LICENSE.md",
    "LICENSE.txt",
    ".gitignore",
    ".gitattributes",
];

/// Directories copied recursively for the `meta` bundle when present in golden.
const META_DIRS: &[&str] = &[".github"];

// ---------------------------------------------------------------------------
// GET /api/consistency/config
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct ConfigResponse {
    golden_repo_id: Option<i64>,
    bundles: Vec<Bundle>,
}

async fn get_config(State(state): State<AppState>) -> ApiResult<Json<ConfigResponse>> {
    let golden_repo_id = read_golden(&state).await?;
    Ok(Json(ConfigResponse {
        golden_repo_id,
        bundles: bundles(),
    }))
}

/// Read the configured golden repo id, if any.
async fn read_golden(state: &AppState) -> ApiResult<Option<i64>> {
    let row: Option<(String,)> = sqlx::query_as::<_, (String,)>(
        "SELECT value FROM settings \
         WHERE scope = ?1 AND repo_id IS NULL AND key = ?2",
    )
    .bind(GLOBAL_SCOPE)
    .bind(GOLDEN_KEY)
    .fetch_optional(&state.db)
    .await?;
    Ok(row.and_then(|r| r.0.parse::<i64>().ok()))
}

// ---------------------------------------------------------------------------
// PUT /api/consistency/golden
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct GoldenBody {
    pub repo_id: i64,
}

async fn put_golden(
    State(state): State<AppState>,
    Json(body): Json<GoldenBody>,
) -> ApiResult<Json<serde_json::Value>> {
    // Validate the repo exists before persisting.
    fetch_repo(&state, body.repo_id).await?;
    upsert_global_setting(&state, GOLDEN_KEY, &body.repo_id.to_string()).await?;
    Ok(Json(json!({ "golden_repo_id": body.repo_id })))
}

/// Upsert a global (`repo_id IS NULL`) setting, mirroring `settings_api`.
async fn upsert_global_setting(state: &AppState, key: &str, value: &str) -> ApiResult<()> {
    let mut tx = state.db.begin().await?;
    let updated = sqlx::query(
        "UPDATE settings SET value = ?3 \
         WHERE scope = ?1 AND repo_id IS NULL AND key = ?2",
    )
    .bind(GLOBAL_SCOPE)
    .bind(key)
    .bind(value)
    .execute(&mut *tx)
    .await?;

    if updated.rows_affected() == 0 {
        sqlx::query(
            "INSERT INTO settings (scope, repo_id, key, value) \
             VALUES (?1, NULL, ?2, ?3)",
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
// POST /api/consistency/apply
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct ApplyBody {
    pub golden_id: i64,
    pub target_ids: Vec<i64>,
    pub bundle_keys: Vec<String>,
}

#[derive(Debug, Serialize)]
struct TargetResult {
    repo_id: i64,
    ok: bool,
    committed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct ApplyResponse {
    summary: String,
    results: Vec<TargetResult>,
}

async fn apply(
    State(state): State<AppState>,
    Json(body): Json<ApplyBody>,
) -> ApiResult<Json<ApplyResponse>> {
    let known: Vec<&'static str> = bundles().into_iter().map(|b| b.key).collect();
    let selected: Vec<String> = body
        .bundle_keys
        .iter()
        .filter(|k| known.contains(&k.as_str()))
        .cloned()
        .collect();
    if selected.is_empty() {
        return Err(AppError::msg("no valid bundle_keys selected"));
    }

    // The golden repo must be cloned so we have its files on disk.
    let golden = fetch_repo(&state, body.golden_id).await?;
    let golden_path = require_cloned(&golden)?;

    let mut results = Vec::with_capacity(body.target_ids.len());

    for target_id in &body.target_ids {
        if *target_id == body.golden_id {
            // Applying the golden onto itself is a no-op; skip cleanly.
            results.push(TargetResult {
                repo_id: *target_id,
                ok: true,
                committed: false,
                error: Some("skipped: target is the golden repo".to_string()),
            });
            continue;
        }
        let res = apply_to_target(&state, *target_id, &golden_path, &selected).await;
        results.push(match res {
            Ok(committed) => TargetResult {
                repo_id: *target_id,
                ok: true,
                committed,
                error: None,
            },
            Err(e) => TargetResult {
                repo_id: *target_id,
                ok: false,
                committed: false,
                error: Some(e.to_string()),
            },
        });
    }

    let ok_count = results.iter().filter(|r| r.ok).count();
    let committed_count = results.iter().filter(|r| r.committed).count();
    let summary = format!(
        "Applied [{}] from golden '{}' to {} target(s): {} ok, {} committed.",
        selected.join(", "),
        golden.full_name,
        results.len(),
        ok_count,
        committed_count,
    );

    broadcast(
        &state,
        json!({
            "event": "consistency_applied",
            "golden_id": body.golden_id,
            "targets": results.len(),
            "ok": ok_count,
            "committed": committed_count,
        }),
    );

    Ok(Json(ApplyResponse { summary, results }))
}

/// Apply the selected bundles to a single target. Returns whether a commit was
/// created on the staging branch.
async fn apply_to_target(
    state: &AppState,
    target_id: i64,
    golden_path: &Path,
    bundle_keys: &[String],
) -> ApiResult<bool> {
    let target = fetch_repo(state, target_id).await?;
    let target_path = require_cloned(&target)?;

    // Everything lands on the per-repo staging branch — never main.
    gitops::ensure_staging_branch(&target_path, STAGING_BRANCH)
        .await
        .map_err(AppError::Anyhow)?;

    let mut applied: Vec<&str> = Vec::new();

    for key in bundle_keys {
        match key.as_str() {
            "format" => {
                copy_files(golden_path, &target_path, FORMAT_FILES).await?;
                applied.push("format");
            }
            "meta" => {
                copy_files(golden_path, &target_path, META_FILES).await?;
                for dir in META_DIRS {
                    copy_dir_recursive(&golden_path.join(dir), &target_path.join(dir)).await?;
                }
                applied.push("meta");
            }
            "tokens" => {
                run_align_claude(&target_path, golden_path, AlignKind::Tokens).await?;
                applied.push("tokens");
            }
            "claude" => {
                run_align_claude(&target_path, golden_path, AlignKind::Claude).await?;
                applied.push("claude");
            }
            _ => {}
        }
    }

    let message = format!(
        "RepoHub consistency: align {} to golden repo",
        applied.join(", ")
    );
    let committed = gitops::commit_all(&target_path, &message)
        .await
        .map_err(AppError::Anyhow)?;

    Ok(committed)
}

/// Which Claude-driven alignment to run for a bundle.
#[derive(Debug, Clone, Copy)]
enum AlignKind {
    Tokens,
    Claude,
}

const TOKENS_PROMPT_PREFIX: &str =
    "Align this repository's design tokens to match the golden reference repository located at";
const TOKENS_PROMPT_BODY: &str = "Update this repository's design tokens (e.g. CSS custom \
properties in tokens.css / theme files, and any TypeScript token modules such as tokens.ts) so \
they match the conventions, naming, and values used by the golden reference repository. Only \
change token definitions and closely related styling config; do not rewrite application logic. \
Preserve files that have no token-related content.";

const CLAUDE_PROMPT_PREFIX: &str =
    "Align this repository's CLAUDE.md and AGENTS.md to match the golden reference repository located at";
const CLAUDE_PROMPT_BODY: &str = "Update (or create) this repository's CLAUDE.md and AGENTS.md so \
their structure, section headings, tone, and shared conventions match the golden reference \
repository, while keeping any details that are genuinely specific to THIS repository (its name, \
purpose, commands, and paths). Do not invent facts about this repo; merge the golden's \
conventions over this repo's accurate specifics.";

/// Invoke the local `claude` CLI in the target dir, instructing it to align to
/// the golden repo at `golden_path`. Token usage is discarded here (the run is
/// best-effort mechanical alignment, not a logged chat).
async fn run_align_claude(
    target_path: &Path,
    golden_path: &Path,
    kind: AlignKind,
) -> ApiResult<()> {
    let (prefix, body) = match kind {
        AlignKind::Tokens => (TOKENS_PROMPT_PREFIX, TOKENS_PROMPT_BODY),
        AlignKind::Claude => (CLAUDE_PROMPT_PREFIX, CLAUDE_PROMPT_BODY),
    };
    let prompt = format!("{} {}.\n\n{}", prefix, golden_path.display(), body);
    claude_runner::run_collect(target_path, &prompt)
        .await
        .map_err(AppError::Anyhow)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Filesystem helpers (mechanical copy for `format` / `meta`)
// ---------------------------------------------------------------------------

/// Copy each of `rel_paths` from `golden` to `target` (overwrite), but only the
/// ones that exist as regular files in the golden repo.
async fn copy_files(golden: &Path, target: &Path, rel_paths: &[&str]) -> ApiResult<()> {
    for rel in rel_paths {
        let src = golden.join(rel);
        if tokio::fs::metadata(&src).await.map(|m| m.is_file()).unwrap_or(false) {
            let dst = target.join(rel);
            if let Some(parent) = dst.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            tokio::fs::copy(&src, &dst).await?;
        }
    }
    Ok(())
}

/// Recursively copy a directory `src` into `dst` (overwriting files). No-op if
/// `src` does not exist. Uses an explicit stack to avoid async recursion.
async fn copy_dir_recursive(src: &Path, dst: &Path) -> ApiResult<()> {
    if !tokio::fs::metadata(src).await.map(|m| m.is_dir()).unwrap_or(false) {
        return Ok(());
    }
    let mut stack: Vec<(PathBuf, PathBuf)> = vec![(src.to_path_buf(), dst.to_path_buf())];
    while let Some((from, to)) = stack.pop() {
        tokio::fs::create_dir_all(&to).await?;
        let mut entries = tokio::fs::read_dir(&from).await?;
        while let Some(entry) = entries.next_entry().await? {
            let ft = entry.file_type().await?;
            let from_child = entry.path();
            let to_child = to.join(entry.file_name());
            if ft.is_dir() {
                stack.push((from_child, to_child));
            } else if ft.is_file() {
                if let Some(parent) = to_child.parent() {
                    tokio::fs::create_dir_all(parent).await?;
                }
                tokio::fs::copy(&from_child, &to_child).await?;
            }
            // Symlinks and other special files are skipped intentionally.
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

async fn fetch_repo(state: &AppState, id: i64) -> ApiResult<Repo> {
    sqlx::query_as::<_, Repo>("SELECT * FROM repos WHERE id = ?1")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::msg(format!("repo {id} not found")))
}

/// Return the cloned local path of a repo, erroring if it is not cloned.
fn require_cloned(repo: &Repo) -> ApiResult<PathBuf> {
    let path = repo
        .local_path
        .as_ref()
        .map(PathBuf::from)
        .filter(|p| p.join(".git").exists())
        .ok_or_else(|| {
            AppError::msg(format!(
                "repo '{}' (id {}) is not cloned locally",
                repo.full_name, repo.id
            ))
        })?;
    Ok(path)
}

fn broadcast(state: &AppState, event: serde_json::Value) {
    let _ = state.status_tx.send(event.to_string());
}
