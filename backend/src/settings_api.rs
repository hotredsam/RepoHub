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

use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::error::{ApiResult, AppError};
use crate::models::Setting;
use crate::state::AppState;

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
