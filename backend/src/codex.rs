//! ChatGPT Codex credential lifecycle (P19).
//!
//! Codex is a first-class, full-access automation principal — but a guarded one.
//! It authenticates with a long-lived bearer token (`Authorization: Bearer
//! rhcx_…`) and is subject to the destructive-action confirmation flow in
//! [`crate::auth_mw::require_confirmation`]. This module owns the credential
//! lifecycle and the owner-facing control surface:
//!
//! - `codex_enabled` (global setting, default **false**) — the master switch. A
//!   bearer is only honored while Codex is enabled.
//! - `codex_destructive_confirm` (global setting, default **true**) — whether
//!   destructive calls require the double-confirm token.
//!
//! Security model (mirrors [`crate::auth_session`]): only the HMAC hash of a
//! token is ever stored, the raw token is shown to the owner exactly **once** at
//! issuance, and issuing a new token revokes any prior active one. The kill
//! switch disables Codex, revokes every credential, and revokes every
//! Codex-origin browser session in one idempotent sweep.
//!
//! Every route here is OWNER-ONLY: a [`Principal::Codex`] caller is rejected with
//! 403 so a compromised Codex token can never grant itself more access, flip the
//! confirmation requirement, or mint fresh credentials.
//!
//! Patterns follow the sibling feature modules (`evals_gcloud`, `connections`):
//! `pub fn router() -> Router<AppState>`, handlers taking `State<AppState>` and
//! returning `ApiResult<Json<..>>`, runtime sqlx only (`query` / `query_as`,
//! never the compile-time macros), and the NULL-aware global-settings upsert
//! copied locally so this module touches only its own file.

use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::FromRow;

use crate::auth_mw::Principal;
use crate::auth_session;
use crate::error::{ApiResult, AppError};
use crate::state::AppState;

/// Scope used for application-wide (non-repo) settings.
const GLOBAL_SCOPE: &str = "global";

/// Prefix on every issued Codex bearer token, so the token is recognizable in
/// logs/config and distinct from session cookies.
pub const CODEX_TOKEN_PREFIX: &str = "rhcx_";

/// Global setting: master enable switch for the Codex principal. Default false.
const KEY_CODEX_ENABLED: &str = "codex_enabled";
/// Global setting: whether destructive Codex calls require confirmation. Default
/// true. (Consumed by [`crate::auth_mw::require_confirmation`]; surfaced/edited
/// here so the owner has one place to manage Codex.)
const KEY_CODEX_CONFIRM: &str = "codex_destructive_confirm";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/codex", get(get_codex).put(put_codex))
        .route("/api/codex/token", post(post_token))
        .route("/api/codex/kill", post(post_kill))
        .route("/api/codex/credentials", get(list_credentials))
        .route("/api/codex/credentials/:id/revoke", post(revoke_credential))
}

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn broadcast(state: &AppState, event: Value) {
    let _ = state.status_tx.send(event.to_string());
}

/// Reject [`Principal::Codex`] from owner-only routes. A Codex bearer must never
/// be able to manage its own credentials or flip its own guardrails.
fn require_owner(principal: &Principal) -> ApiResult<()> {
    match principal {
        Principal::Codex => Err(AppError::Forbidden(
            "codex credentials are owner-only".into(),
        )),
        Principal::Local | Principal::User(_) => Ok(()),
    }
}

// ---------------------------------------------------------------------------
// Settings (global enable / confirm flags)
// ---------------------------------------------------------------------------

/// Whether the Codex principal is enabled (`codex_enabled`, default **false**).
pub async fn codex_enabled(state: &AppState) -> bool {
    read_global_bool(state, KEY_CODEX_ENABLED, false).await
}

/// Whether destructive Codex calls require confirmation
/// (`codex_destructive_confirm`, default **true**).
pub async fn destructive_confirm_required(state: &AppState) -> bool {
    read_global_bool(state, KEY_CODEX_CONFIRM, true).await
}

// ---------------------------------------------------------------------------
// Credential rows
// ---------------------------------------------------------------------------

/// One row of `codex_credentials`. The `token_hash` is never serialized to the
/// owner UI — credential responses use [`CredentialMeta`] instead.
#[derive(Debug, Clone, FromRow)]
pub struct CredentialRow {
    pub id: i64,
    pub token_hash: String,
    pub label: Option<String>,
    pub created_at: String,
    pub last_used_at: Option<String>,
    pub revoked: i64,
    pub revoked_at: Option<String>,
}

/// Credential metadata safe to return to the owner (never the hash or raw token).
#[derive(Debug, Clone, Serialize)]
pub struct CredentialMeta {
    pub id: i64,
    pub label: Option<String>,
    pub created_at: String,
    pub last_used_at: Option<String>,
    pub revoked: bool,
    pub revoked_at: Option<String>,
}

impl From<CredentialRow> for CredentialMeta {
    fn from(r: CredentialRow) -> Self {
        CredentialMeta {
            id: r.id,
            label: r.label,
            created_at: r.created_at,
            last_used_at: r.last_used_at,
            revoked: r.revoked != 0,
            revoked_at: r.revoked_at,
        }
    }
}

/// A freshly issued token. The raw value is present exactly once (here) and never
/// persisted or returned again.
#[derive(Debug, Clone, Serialize)]
pub struct IssuedToken {
    pub id: i64,
    /// The raw bearer token, including the `rhcx_` prefix. Shown ONCE.
    pub token: String,
    pub label: Option<String>,
    pub created_at: String,
}

// ---------------------------------------------------------------------------
// Lifecycle (library API — also used by the gate/integrate step)
// ---------------------------------------------------------------------------

/// Issue a new Codex bearer token: revoke any prior active credential, store only
/// the HMAC hash, and return the raw token ONCE.
///
/// The raw token is `rhcx_` + a CSPRNG opaque string; we store
/// `hash_token(raw, keys)` so a leaked DB cannot reconstruct the bearer.
pub async fn issue_token(state: &AppState, label: Option<&str>) -> ApiResult<IssuedToken> {
    let label = label.map(str::trim).filter(|s| !s.is_empty());
    let raw = format!("{CODEX_TOKEN_PREFIX}{}", auth_session::new_token());
    let hash = auth_session::hash_token(&raw, &state.auth);
    let created_at = now_iso();

    let mut tx = state.db.begin().await?;

    // Revoke any prior active credential — only one live Codex token at a time.
    sqlx::query("UPDATE codex_credentials SET revoked = 1, revoked_at = ?1 WHERE revoked = 0")
        .bind(&created_at)
        .execute(&mut *tx)
        .await?;

    let res = sqlx::query(
        "INSERT INTO codex_credentials (token_hash, label, created_at, last_used_at, revoked, revoked_at) \
         VALUES (?1, ?2, ?3, NULL, 0, NULL)",
    )
    .bind(&hash)
    .bind(label)
    .bind(&created_at)
    .execute(&mut *tx)
    .await?;
    let id = res.last_insert_rowid();

    tx.commit().await?;

    Ok(IssuedToken {
        id,
        token: raw,
        label: label.map(|s| s.to_string()),
        created_at,
    })
}

/// Validate a Codex bearer token. Returns the credential id on a live match
/// (non-revoked AND `codex_enabled`), bumping `last_used_at`.
///
/// This is the authoritative implementation the gate should delegate to (the
/// foundation note in `auth_mw` flags its inline copy for replacement).
pub async fn validate_bearer(state: &AppState, bearer: &str) -> Option<i64> {
    let bearer = bearer.trim();
    if bearer.is_empty() {
        return None;
    }
    // A genuine Codex token always carries our prefix; reject anything else
    // cheaply before touching the DB (and before honoring the enable switch).
    if !bearer.starts_with(CODEX_TOKEN_PREFIX) {
        return None;
    }
    if !codex_enabled(state).await {
        return None;
    }

    let hash = auth_session::hash_token(bearer, &state.auth);
    let row: Option<(i64,)> = sqlx::query_as::<_, (i64,)>(
        "SELECT id FROM codex_credentials WHERE token_hash = ?1 AND revoked = 0",
    )
    .bind(&hash)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten();

    match row {
        Some((id,)) => {
            let now = now_iso();
            let _ = sqlx::query("UPDATE codex_credentials SET last_used_at = ?1 WHERE id = ?2")
                .bind(&now)
                .bind(id)
                .execute(&state.db)
                .await;
            Some(id)
        }
        None => None,
    }
}

/// KILL SWITCH. Disable Codex (`codex_enabled = false`), revoke every credential,
/// revoke every `origin = 'codex'` browser session, and broadcast a status event.
/// Idempotent: safe to call when nothing is active.
pub async fn kill(state: &AppState) -> ApiResult<()> {
    let now = now_iso();

    // 1) Master switch off.
    upsert_global_setting(state, KEY_CODEX_ENABLED, "false").await?;

    // 2) Revoke all credentials.
    sqlx::query("UPDATE codex_credentials SET revoked = 1, revoked_at = ?1 WHERE revoked = 0")
        .bind(&now)
        .execute(&state.db)
        .await?;

    // 3) Revoke all Codex-origin sessions (defense in depth).
    sqlx::query("UPDATE sessions SET revoked = 1 WHERE origin = 'codex' AND revoked = 0")
        .execute(&state.db)
        .await?;

    // 4) Tell the UI.
    broadcast(state, json!({ "event": "codex_killed", "ts": now }));

    Ok(())
}

// ---------------------------------------------------------------------------
// Routes (owner-only)
// ---------------------------------------------------------------------------

/// The fixed set of API surfaces a Codex bearer may exercise. Returned by
/// `GET /api/codex` so the owner can see exactly what Codex can reach. This is
/// documentation, not enforcement (the gate authorizes Codex for the full app);
/// it never includes raw tokens.
fn api_surface() -> Vec<Value> {
    vec![
        json!({ "method": "GET",  "path": "/api/repos",            "summary": "List tracked repositories" }),
        json!({ "method": "POST", "path": "/api/repos",            "summary": "Track / clone a repository", "destructive": true }),
        json!({ "method": "POST", "path": "/api/repos/:id/pull",   "summary": "Pull a repository", "destructive": true }),
        json!({ "method": "GET",  "path": "/api/repos/:id/status", "summary": "Repository status" }),
        json!({ "method": "POST", "path": "/api/connections/integrate", "summary": "Integrate a feature across repos", "destructive": true }),
        json!({ "method": "POST", "path": "/api/merge",            "summary": "Open / merge a staging PR", "destructive": true }),
        json!({ "method": "GET",  "path": "/api/tickets",          "summary": "List tickets" }),
        json!({ "method": "POST", "path": "/api/tickets",          "summary": "Create a ticket", "destructive": true }),
        json!({ "method": "GET",  "path": "/api/claude/chat",      "summary": "Claude chat / runner" }),
        json!({ "method": "GET",  "path": "/api/settings",         "summary": "Read layered settings" }),
    ]
}

/// GET /api/codex — config + status + active-credential metadata + the documented
/// API surface. Never returns a raw token or any token hash.
async fn get_codex(
    State(state): State<AppState>,
    principal: Principal,
) -> ApiResult<Json<Value>> {
    require_owner(&principal)?;

    let enabled = codex_enabled(&state).await;
    let confirm = destructive_confirm_required(&state).await;

    // The single active (non-revoked) credential's metadata, if any.
    let active: Option<CredentialRow> = sqlx::query_as::<_, CredentialRow>(
        "SELECT id, token_hash, label, created_at, last_used_at, revoked, revoked_at \
         FROM codex_credentials WHERE revoked = 0 ORDER BY id DESC LIMIT 1",
    )
    .fetch_optional(&state.db)
    .await?;

    let active_meta = active.map(CredentialMeta::from);

    let total: (i64,) = sqlx::query_as::<_, (i64,)>(
        "SELECT COUNT(*) FROM codex_credentials WHERE revoked = 0",
    )
    .fetch_one(&state.db)
    .await?;

    Ok(Json(json!({
        "config": {
            "enabled": enabled,
            "destructive_confirm": confirm,
            "token_prefix": CODEX_TOKEN_PREFIX,
        },
        "status": {
            "enabled": enabled,
            "active_credentials": total.0,
            "has_active_credential": active_meta.is_some(),
        },
        "active_credential": active_meta,
        "api_surface": api_surface(),
    })))
}

#[derive(Debug, Deserialize)]
pub struct PutCodexBody {
    /// Master enable switch. `None` leaves it untouched.
    #[serde(default)]
    pub enabled: Option<bool>,
    /// Whether destructive calls require confirmation. `None` leaves it untouched.
    #[serde(default)]
    pub destructive_confirm: Option<bool>,
}

/// PUT /api/codex — update the enable / confirmation flags, then return the same
/// shape as `GET /api/codex`.
async fn put_codex(
    State(state): State<AppState>,
    principal: Principal,
    Json(body): Json<PutCodexBody>,
) -> ApiResult<Json<Value>> {
    require_owner(&principal)?;

    if let Some(enabled) = body.enabled {
        upsert_global_setting(&state, KEY_CODEX_ENABLED, bool_str(enabled)).await?;
        broadcast(
            &state,
            json!({ "event": "codex_config", "enabled": enabled }),
        );
    }
    if let Some(confirm) = body.destructive_confirm {
        upsert_global_setting(&state, KEY_CODEX_CONFIRM, bool_str(confirm)).await?;
    }

    get_codex(State(state), principal).await
}

#[derive(Debug, Deserialize)]
pub struct TokenBody {
    #[serde(default)]
    pub label: Option<String>,
}

/// POST /api/codex/token — mint a new bearer token (revoking any prior active
/// one). The raw token is returned ONCE, with a warning the owner cannot recover
/// it later.
async fn post_token(
    State(state): State<AppState>,
    principal: Principal,
    Json(body): Json<TokenBody>,
) -> ApiResult<Json<Value>> {
    require_owner(&principal)?;

    let issued = issue_token(&state, body.label.as_deref()).await?;
    broadcast(
        &state,
        json!({ "event": "codex_token_issued", "id": issued.id }),
    );

    Ok(Json(json!({
        "token": issued.token,
        "credential": {
            "id": issued.id,
            "label": issued.label,
            "created_at": issued.created_at,
        },
        "warning": "Copy this token now — it is shown only once and cannot be recovered. \
                    Issuing it revoked any previous Codex token.",
    })))
}

/// POST /api/codex/kill — the kill switch. Disable Codex, revoke all credentials
/// and Codex sessions. Idempotent.
async fn post_kill(
    State(state): State<AppState>,
    principal: Principal,
) -> ApiResult<Json<Value>> {
    require_owner(&principal)?;
    kill(&state).await?;
    Ok(Json(json!({ "killed": true, "enabled": false })))
}

/// GET /api/codex/credentials — all credential metadata (never the hash/token),
/// newest first.
async fn list_credentials(
    State(state): State<AppState>,
    principal: Principal,
) -> ApiResult<Json<Vec<CredentialMeta>>> {
    require_owner(&principal)?;

    let rows: Vec<CredentialRow> = sqlx::query_as::<_, CredentialRow>(
        "SELECT id, token_hash, label, created_at, last_used_at, revoked, revoked_at \
         FROM codex_credentials ORDER BY id DESC",
    )
    .fetch_all(&state.db)
    .await?;

    Ok(Json(rows.into_iter().map(CredentialMeta::from).collect()))
}

/// POST /api/codex/credentials/:id/revoke — soft-revoke a single credential.
async fn revoke_credential(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    require_owner(&principal)?;

    let now = now_iso();
    let res = sqlx::query(
        "UPDATE codex_credentials SET revoked = 1, revoked_at = ?1 WHERE id = ?2 AND revoked = 0",
    )
    .bind(&now)
    .bind(id)
    .execute(&state.db)
    .await?;

    let revoked = res.rows_affected() > 0;
    if revoked {
        broadcast(
            &state,
            json!({ "event": "codex_token_revoked", "id": id }),
        );
    }
    Ok(Json(json!({ "id": id, "revoked": revoked })))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn bool_str(b: bool) -> &'static str {
    if b {
        "true"
    } else {
        "false"
    }
}

async fn read_global_bool(state: &AppState, key: &str, default: bool) -> bool {
    match read_global_setting(state, key).await {
        Some(v) => matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        None => default,
    }
}

async fn read_global_setting(state: &AppState, key: &str) -> Option<String> {
    sqlx::query_as::<_, (String,)>(
        "SELECT value FROM settings WHERE scope = ?1 AND repo_id IS NULL AND key = ?2",
    )
    .bind(GLOBAL_SCOPE)
    .bind(key)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten()
    .map(|(s,)| s)
}

/// NULL-aware upsert of a single `scope='global'`, `repo_id IS NULL` setting.
/// Copied from `evals_gcloud::upsert_global_setting` per the conventions so this
/// module stays self-contained.
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
        sqlx::query("INSERT INTO settings (scope, repo_id, key, value) VALUES (?1, NULL, ?2, ?3)")
            .bind(GLOBAL_SCOPE)
            .bind(key)
            .bind(value)
            .execute(&mut *tx)
            .await?;
    }

    tx.commit().await?;
    Ok(())
}
