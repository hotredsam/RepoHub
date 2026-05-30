//! Settings & preferences API.
//!
//! Endpoints (per the RepoHub contract):
//! - `GET  /api/settings?scope=&repo_id=`  — list settings rows, optionally filtered.
//! - `PUT  /api/settings`                  — upsert a single setting.
//! - `GET  /api/preferences`               — well-known global preference keys.
//! - `PUT  /api/preferences`               — upsert well-known global preference keys.
//!
//! Settings are stored in the `settings` table keyed by `(scope, repo_id, key)`.
//! Preferences are simply a curated set of settings rows with `scope = 'global'`
//! and `repo_id IS NULL`. We expose them through a flat, typed shape so the
//! frontend Settings view does not have to know the underlying key names.

use std::path::{Path as FsPath, PathBuf};

use axum::extract::{Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{ApiResult, AppError};
use crate::models::{Prompt, Repo, Setting};
use crate::state::AppState;
use crate::{claude_runner, gitops};

/// Branch where all Claude-authored changes land (never `main` directly).
const STAGING_BRANCH: &str = "repohub-staging";

/// Cap on how many entries the shallow file listing will gather.
const MAX_LISTING_ENTRIES: usize = 200;

/// How many recent prompts to feed into the suggestion context.
const RECENT_PROMPTS_LIMIT: i64 = 20;

/// Scope used for application-wide preferences.
const GLOBAL_SCOPE: &str = "global";

/// Well-known preference keys and their defaults.
const PREF_LANGUAGES_KEY: &str = "pref_languages";
const PREF_CLOUD_KEY: &str = "pref_cloud";
const DEFAULT_PREF_LANGUAGES: &str = "Rust";
const DEFAULT_PREF_CLOUD: &str = "Google Cloud";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/settings", get(list_settings).put(upsert_setting))
        .route("/api/preferences", get(get_preferences).put(put_preferences))
        .route("/api/settings/suggest", post(suggest_settings))
        .route("/api/settings/apply-suggestion", post(apply_suggestion))
}

// ---------------------------------------------------------------------------
// GET /api/settings?scope=&repo_id=
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct SettingsQuery {
    pub scope: Option<String>,
    pub repo_id: Option<i64>,
}

async fn list_settings(
    State(state): State<AppState>,
    Query(q): Query<SettingsQuery>,
) -> ApiResult<Json<Vec<Setting>>> {
    // Build the query dynamically depending on which filters were supplied.
    // We use runtime sqlx (no compile-time macros) and bind every parameter.
    let rows: Vec<Setting> = match (q.scope.as_deref(), q.repo_id) {
        (Some(scope), Some(repo_id)) => {
            sqlx::query_as::<_, Setting>(
                "SELECT scope, repo_id, key, value FROM settings \
                 WHERE scope = ?1 AND repo_id = ?2 ORDER BY key",
            )
            .bind(scope)
            .bind(repo_id)
            .fetch_all(&state.db)
            .await?
        }
        (Some(scope), None) => {
            sqlx::query_as::<_, Setting>(
                "SELECT scope, repo_id, key, value FROM settings \
                 WHERE scope = ?1 ORDER BY key",
            )
            .bind(scope)
            .fetch_all(&state.db)
            .await?
        }
        (None, Some(repo_id)) => {
            sqlx::query_as::<_, Setting>(
                "SELECT scope, repo_id, key, value FROM settings \
                 WHERE repo_id = ?1 ORDER BY scope, key",
            )
            .bind(repo_id)
            .fetch_all(&state.db)
            .await?
        }
        (None, None) => {
            sqlx::query_as::<_, Setting>(
                "SELECT scope, repo_id, key, value FROM settings \
                 ORDER BY scope, key",
            )
            .fetch_all(&state.db)
            .await?
        }
    };

    Ok(Json(rows))
}

// ---------------------------------------------------------------------------
// PUT /api/settings  — upsert one setting row.
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct UpsertSettingBody {
    /// Defaults to `"global"` when omitted.
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub repo_id: Option<i64>,
    pub key: String,
    pub value: String,
}

async fn upsert_setting(
    State(state): State<AppState>,
    Json(body): Json<UpsertSettingBody>,
) -> ApiResult<Json<Setting>> {
    let key = body.key.trim();
    if key.is_empty() {
        return Err(AppError::msg("setting key must not be empty"));
    }
    let scope = body
        .scope
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(GLOBAL_SCOPE)
        .to_string();

    upsert_setting_row(&state, &scope, body.repo_id, key, &body.value).await?;

    Ok(Json(Setting {
        scope,
        repo_id: body.repo_id,
        key: key.to_string(),
        value: body.value,
    }))
}

/// Upsert a single `(scope, repo_id, key)` setting.
///
/// SQLite treats `NULL` values as distinct in unique constraints, so a plain
/// `ON CONFLICT(scope, repo_id, key)` upsert does not reliably match rows where
/// `repo_id IS NULL`. To stay correct for both the `repo_id = N` and the
/// `repo_id IS NULL` cases we do an explicit update-then-insert in a
/// transaction rather than relying on the unique index.
async fn upsert_setting_row(
    state: &AppState,
    scope: &str,
    repo_id: Option<i64>,
    key: &str,
    value: &str,
) -> ApiResult<()> {
    let mut tx = state.db.begin().await?;

    // UPDATE matching the NULL-aware semantics we want. `?2 IS NULL` lets the
    // same statement handle both global (repo_id NULL) and per-repo rows.
    let updated = sqlx::query(
        "UPDATE settings SET value = ?4 \
         WHERE scope = ?1 \
           AND ((?2 IS NULL AND repo_id IS NULL) OR repo_id = ?2) \
           AND key = ?3",
    )
    .bind(scope)
    .bind(repo_id)
    .bind(key)
    .bind(value)
    .execute(&mut *tx)
    .await?;

    if updated.rows_affected() == 0 {
        sqlx::query(
            "INSERT INTO settings (scope, repo_id, key, value) \
             VALUES (?1, ?2, ?3, ?4)",
        )
        .bind(scope)
        .bind(repo_id)
        .bind(key)
        .bind(value)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok(())
}

/// Read a single global setting value, returning `None` if absent.
async fn get_global_setting(state: &AppState, key: &str) -> ApiResult<Option<String>> {
    let row: Option<(String,)> = sqlx::query_as::<_, (String,)>(
        "SELECT value FROM settings \
         WHERE scope = ?1 AND repo_id IS NULL AND key = ?2",
    )
    .bind(GLOBAL_SCOPE)
    .bind(key)
    .fetch_optional(&state.db)
    .await?;
    Ok(row.map(|r| r.0))
}

// ---------------------------------------------------------------------------
// GET/PUT /api/preferences  — curated global preferences.
// ---------------------------------------------------------------------------

/// Preferences are exposed under their well-known setting key names so the
/// frontend can address them directly (`pref_languages`, `pref_cloud`).
#[derive(Debug, Serialize, Deserialize)]
pub struct Preferences {
    /// Preferred default language(s) for new work, e.g. "Rust".
    pub pref_languages: String,
    /// Preferred cloud provider, e.g. "Google Cloud".
    pub pref_cloud: String,
}

async fn get_preferences(State(state): State<AppState>) -> ApiResult<Json<Preferences>> {
    let pref_languages = get_global_setting(&state, PREF_LANGUAGES_KEY)
        .await?
        .unwrap_or_else(|| DEFAULT_PREF_LANGUAGES.to_string());
    let pref_cloud = get_global_setting(&state, PREF_CLOUD_KEY)
        .await?
        .unwrap_or_else(|| DEFAULT_PREF_CLOUD.to_string());

    Ok(Json(Preferences {
        pref_languages,
        pref_cloud,
    }))
}

#[derive(Debug, Deserialize)]
pub struct PreferencesUpdate {
    pub pref_languages: Option<String>,
    pub pref_cloud: Option<String>,
}

async fn put_preferences(
    State(state): State<AppState>,
    Json(body): Json<PreferencesUpdate>,
) -> ApiResult<Json<Preferences>> {
    if let Some(languages) = body.pref_languages.as_deref() {
        upsert_setting_row(&state, GLOBAL_SCOPE, None, PREF_LANGUAGES_KEY, languages).await?;
    }
    if let Some(cloud) = body.pref_cloud.as_deref() {
        upsert_setting_row(&state, GLOBAL_SCOPE, None, PREF_CLOUD_KEY, cloud).await?;
    }

    // Return the full, current preference set (including any unchanged defaults).
    get_preferences(State(state)).await
}

// ---------------------------------------------------------------------------
// POST /api/settings/suggest  — ask Claude to PROPOSE config (preview only).
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct SuggestBody {
    pub repo_id: i64,
}

/// One file the model proposes (or one we ask the caller to apply).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuggestedFile {
    pub path: String,
    pub content: String,
}

#[derive(Debug, Serialize)]
pub struct SuggestResponse {
    pub claude_md: String,
    pub settings_json: String,
    pub dev_configs: Vec<SuggestedFile>,
    pub rationale: String,
}

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

/// Shallow listing of `root` to a depth of two levels, names (relative paths)
/// only, capped at `MAX_LISTING_ENTRIES`. `.git` is skipped. Best-effort: any
/// unreadable directory is silently ignored so the suggestion can still proceed.
fn shallow_listing(root: &FsPath) -> Vec<String> {
    fn walk(dir: &FsPath, base: &FsPath, depth: usize, out: &mut Vec<String>) {
        if depth == 0 || out.len() >= MAX_LISTING_ENTRIES {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            if out.len() >= MAX_LISTING_ENTRIES {
                return;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name == ".git" {
                continue;
            }
            let path = entry.path();
            let rel = path
                .strip_prefix(base)
                .unwrap_or(&path)
                .to_string_lossy()
                .to_string();
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            if is_dir {
                out.push(format!("{rel}/"));
                walk(&path, base, depth - 1, out);
            } else {
                out.push(rel);
            }
        }
    }

    let mut out = Vec::new();
    walk(root, root, 2, &mut out);
    out.truncate(MAX_LISTING_ENTRIES);
    out
}

/// Construct the suggestion prompt from gathered repo context + preferences.
fn build_suggest_prompt(
    repo: &Repo,
    listing: &[String],
    recent_prompts: &[Prompt],
    pref_languages: &str,
    pref_cloud: &str,
) -> String {
    let language = repo.language.as_deref().unwrap_or("unknown");
    let description = repo.description.as_deref().unwrap_or("(none)");
    let listing_block = if listing.is_empty() {
        "(empty or unreadable)".to_string()
    } else {
        listing.join("\n")
    };
    let prompts_block = if recent_prompts.is_empty() {
        "(no recent prompts)".to_string()
    } else {
        recent_prompts
            .iter()
            .map(|p| {
                let line = p.prompt.replace('\n', " ");
                let line: String = line.chars().take(200).collect();
                format!("- {line}")
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    format!(
        "You are helping configure the repository \"{full_name}\" for use with Claude Code.\n\
         Detected primary language: {language}.\n\
         Repository description: {description}.\n\n\
         My standing preferences (SUGGEST, do not force — only apply where they fit):\n\
         - Preferred language(s): {pref_languages}\n\
         - Preferred cloud provider: {pref_cloud}\n\n\
         Shallow file listing (top 2 levels, names only):\n{listing_block}\n\n\
         A sample of my recent prompts for this repo (for tone/intent):\n{prompts_block}\n\n\
         Propose sensible, conservative configuration for this repo:\n\
         1. A `.claude/settings.json` with reasonable permissions and (optionally) hooks.\n\
         2. A `CLAUDE.md` documenting the repo for Claude Code, reflecting the detected \
         language and my preferences where appropriate.\n\
         3. A few dev-tooling config files appropriate to the language (e.g. for Rust: \
         rustfmt.toml, .editorconfig, a CI workflow). Keep them minimal and idiomatic.\n\n\
         Respond with ONLY a single JSON object (no markdown fences, no prose outside it) \
         of the exact shape:\n\
         {{\n\
           \"claude_md\": \"<full CLAUDE.md contents>\",\n\
           \"settings_json\": \"<full .claude/settings.json contents>\",\n\
           \"dev_configs\": [{{\"path\": \"<repo-relative path>\", \"content\": \"<file contents>\"}}],\n\
           \"rationale\": \"<short explanation of your choices>\"\n\
         }}\n\
         Use repo-relative paths only; never absolute paths or paths containing `..`.",
        full_name = repo.full_name,
    )
}

/// Strip an optional ```json … ``` (or bare ```) fence around `s`.
fn strip_code_fence(s: &str) -> &str {
    let t = s.trim();
    if let Some(rest) = t.strip_prefix("```") {
        // Drop an optional language tag on the opening fence line.
        let rest = match rest.find('\n') {
            Some(nl) => &rest[nl + 1..],
            None => rest,
        };
        return rest.strip_suffix("```").map(str::trim).unwrap_or(rest);
    }
    t
}

/// Best-effort parse of the model's JSON object into the typed response.
fn parse_suggestion(raw: &str) -> SuggestResponse {
    // The model may wrap the JSON in a code fence or surrounding prose; try the
    // fenced/whole string first, then fall back to the outermost `{ … }` slice.
    let candidate = strip_code_fence(raw);
    let parsed: Option<Value> = serde_json::from_str(candidate).ok().or_else(|| {
        let start = raw.find('{')?;
        let end = raw.rfind('}')?;
        if end <= start {
            return None;
        }
        serde_json::from_str(&raw[start..=end]).ok()
    });

    match parsed {
        Some(v) => {
            let claude_md = v
                .get("claude_md")
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string();
            let settings_json = v
                .get("settings_json")
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string();
            let dev_configs = v
                .get("dev_configs")
                .and_then(|x| x.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|f| {
                            let path = f.get("path").and_then(|p| p.as_str())?.to_string();
                            let content =
                                f.get("content").and_then(|c| c.as_str()).unwrap_or_default();
                            Some(SuggestedFile {
                                path,
                                content: content.to_string(),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            let rationale = v
                .get("rationale")
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string();
            SuggestResponse {
                claude_md,
                settings_json,
                dev_configs,
                rationale,
            }
        }
        // Parsing failed: surface the raw response so the user still sees the
        // model's output, and leave the structured fields empty (best-effort).
        None => SuggestResponse {
            claude_md: String::new(),
            settings_json: String::new(),
            dev_configs: Vec::new(),
            rationale: raw.trim().to_string(),
        },
    }
}

async fn suggest_settings(
    State(state): State<AppState>,
    Json(body): Json<SuggestBody>,
) -> ApiResult<Json<SuggestResponse>> {
    let repo = fetch_repo(&state, body.repo_id).await?;
    let path = repo_local_path(&repo)?;

    // Gather context: shallow listing, recent prompts, and preferences.
    let listing = shallow_listing(&path);

    let recent_prompts = sqlx::query_as::<_, Prompt>(
        "SELECT * FROM prompts WHERE repo_id = ?1 ORDER BY created_at DESC LIMIT ?2",
    )
    .bind(body.repo_id)
    .bind(RECENT_PROMPTS_LIMIT)
    .fetch_all(&state.db)
    .await?;

    let pref_languages = get_global_setting(&state, PREF_LANGUAGES_KEY)
        .await?
        .unwrap_or_else(|| DEFAULT_PREF_LANGUAGES.to_string());
    let pref_cloud = get_global_setting(&state, PREF_CLOUD_KEY)
        .await?
        .unwrap_or_else(|| DEFAULT_PREF_CLOUD.to_string());

    let prompt = build_suggest_prompt(
        &repo,
        &listing,
        &recent_prompts,
        &pref_languages,
        &pref_cloud,
    );

    let outcome = claude_runner::run_collect(&path, &prompt)
        .await
        .map_err(AppError::Anyhow)?;

    Ok(Json(parse_suggestion(&outcome.response)))
}

// ---------------------------------------------------------------------------
// POST /api/settings/apply-suggestion  — write files onto the staging branch.
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct ApplySuggestionBody {
    pub repo_id: i64,
    pub files: Vec<SuggestedFile>,
}

#[derive(Debug, Serialize)]
pub struct ApplySuggestionResponse {
    pub ok: bool,
    pub branch: String,
}

/// Validate that a model-proposed path stays inside the repo. Rejects absolute
/// paths, any `..` traversal, and — critically — any path whose components
/// escape the repo via a symlink (e.g. `cfg/app.json` where `cfg` is a symlink
/// to `~/.ssh`). The proposed paths come from untrusted model output, so a
/// purely lexical `..` check is not enough: a symlinked intermediate directory
/// would let `create_dir_all`/`write` follow it outside the working tree.
fn safe_join(root: &FsPath, rel: &str) -> ApiResult<PathBuf> {
    let rel = rel.trim();
    if rel.is_empty() {
        return Err(AppError::msg("file path must not be empty"));
    }
    let candidate = FsPath::new(rel);
    if candidate.is_absolute() {
        return Err(AppError::msg(format!("refusing absolute path: {rel}")));
    }
    for component in candidate.components() {
        if matches!(component, std::path::Component::ParentDir) {
            return Err(AppError::msg(format!("refusing path traversal: {rel}")));
        }
    }

    let dest = root.join(candidate);

    // Reject if any existing ancestor of `dest` (below the repo root) is a
    // symlink — following it could land the write outside the working tree.
    // Walk from the repo root down through each relative component.
    let mut probe = root.to_path_buf();
    for component in candidate.components() {
        probe.push(component);
        match std::fs::symlink_metadata(&probe) {
            Ok(meta) => {
                if meta.file_type().is_symlink() {
                    return Err(AppError::msg(format!(
                        "refusing to write through symlinked path component: {rel}"
                    )));
                }
            }
            // Component does not exist yet — nothing more to check below it.
            Err(_) => break,
        }
    }

    // Belt-and-braces: canonicalize the deepest existing ancestor and assert it
    // still lives under the canonicalized repo root.
    let canon_root = std::fs::canonicalize(root)
        .map_err(|e| AppError::msg(format!("repo root is not accessible: {e}")))?;
    let mut existing = dest.as_path();
    let resolved = loop {
        match std::fs::canonicalize(existing) {
            Ok(p) => break p,
            Err(_) => match existing.parent() {
                Some(parent) => existing = parent,
                // Should never happen: root itself canonicalized above.
                None => break canon_root.clone(),
            },
        }
    };
    if !resolved.starts_with(&canon_root) {
        return Err(AppError::msg(format!(
            "refusing path that escapes the repo root: {rel}"
        )));
    }

    Ok(dest)
}

async fn apply_suggestion(
    State(state): State<AppState>,
    Json(body): Json<ApplySuggestionBody>,
) -> ApiResult<Json<ApplySuggestionResponse>> {
    if body.files.is_empty() {
        return Err(AppError::msg("no files to apply"));
    }

    let repo = fetch_repo(&state, body.repo_id).await?;
    let path = repo_local_path(&repo)?;
    if !path.join(".git").exists() {
        return Err(AppError::msg("repo is not cloned locally"));
    }

    // Claude changes land on the staging branch, never main directly.
    gitops::ensure_staging_branch(&path, STAGING_BRANCH)
        .await
        .map_err(AppError::Anyhow)?;

    for file in &body.files {
        let dest = safe_join(&path, &file.path)?;
        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&dest, file.content.as_bytes()).await?;
    }

    gitops::commit_all(&path, "RepoHub: suggested settings")
        .await
        .map_err(AppError::Anyhow)?;

    Ok(Json(ApplySuggestionResponse {
        ok: true,
        branch: STAGING_BRANCH.to_string(),
    }))
}
