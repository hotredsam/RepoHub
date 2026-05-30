//! Session signing keys, opaque session tokens, and the `sessions` table.
//!
//! P18/P19 foundation (library only — no router; the auth feature agent mounts
//! the `/api/auth/*` routes that call into here).
//!
//! Security model:
//! - We never store a raw session token. A token is a CSPRNG string handed to the
//!   browser in an httpOnly cookie; the database holds only its HMAC-SHA256 hash
//!   keyed by a per-install signing key. A leaked DB therefore cannot mint
//!   cookies that pass [`lookup_session`].
//! - The signing key is generated once and persisted (base64) in the `settings`
//!   table under the global key `auth_signing_key`, so sessions survive restarts.

use axum::http::HeaderMap;
use base64::Engine as _;
use hmac::{Hmac, Mac};
use rand::RngCore;
use serde::Serialize;
use sha2::Sha256;
use sqlx::{FromRow, SqlitePool};

type HmacSha256 = Hmac<Sha256>;

/// Name of the browser session cookie.
pub const SESSION_COOKIE: &str = "repohub_session";

/// Global settings key under which the base64 signing key is persisted.
const SIGNING_KEY_SETTING: &str = "auth_signing_key";
const GLOBAL_SCOPE: &str = "global";

/// Per-install secret used to HMAC session tokens (and confirmation tokens in
/// [`crate::auth_mw`]). Wrapped in `Arc` inside [`crate::state::AppState`].
#[derive(Clone)]
pub struct AuthKeys {
    pub signing_key: [u8; 32],
}

impl std::fmt::Debug for AuthKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print key material.
        f.debug_struct("AuthKeys").field("signing_key", &"[redacted]").finish()
    }
}

impl AuthKeys {
    /// HMAC-SHA256 over `data`, returned as a raw 32-byte tag.
    pub fn mac(&self, data: &[u8]) -> [u8; 32] {
        let mut mac = HmacSha256::new_from_slice(&self.signing_key)
            .expect("HMAC accepts any key length");
        mac.update(data);
        mac.finalize().into_bytes().into()
    }
}

/// Load the persisted signing key, generating and storing one on first run.
///
/// The value is stored base64 (standard alphabet). A malformed/short stored key
/// is replaced with a fresh one (which invalidates any existing sessions, the
/// safe default).
pub async fn load_or_create_signing_key(db: &SqlitePool) -> anyhow::Result<AuthKeys> {
    if let Some(existing) = read_global_setting(db, SIGNING_KEY_SETTING).await? {
        if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(existing.trim()) {
            if bytes.len() == 32 {
                let mut key = [0u8; 32];
                key.copy_from_slice(&bytes);
                return Ok(AuthKeys { signing_key: key });
            }
        }
        tracing::warn!("auth_signing_key malformed; regenerating");
    }

    let mut key = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut key);
    let encoded = base64::engine::general_purpose::STANDARD.encode(key);
    upsert_global_setting(db, SIGNING_KEY_SETTING, &encoded).await?;
    Ok(AuthKeys { signing_key: key })
}

/// Generate a fresh opaque session token (URL-safe base64, 256 bits of entropy).
pub fn new_token() -> String {
    let mut raw = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut raw);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw)
}

/// HMAC-SHA256 of a raw token, lowercase hex. This is what we persist/compare.
pub fn hash_token(raw: &str, keys: &AuthKeys) -> String {
    hex::encode(keys.mac(raw.as_bytes()))
}

/// One row of the `sessions` table.
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct SessionRow {
    pub id: i64,
    pub token_hash: String,
    pub email: String,
    pub origin: Option<String>,
    pub ua: Option<String>,
    pub ip: Option<String>,
    pub created_at: String,
    pub last_seen_at: Option<String>,
    pub expires_at: Option<String>,
    pub revoked: i64,
}

/// Insert a new session for `email`, storing only the token hash. Returns the
/// row id.
#[allow(clippy::too_many_arguments)]
pub async fn create_session(
    db: &SqlitePool,
    keys: &AuthKeys,
    raw_token: &str,
    email: &str,
    origin: Option<&str>,
    ua: Option<&str>,
    ip: Option<&str>,
    expires_at: Option<&str>,
) -> anyhow::Result<i64> {
    let now = chrono::Utc::now().to_rfc3339();
    let hash = hash_token(raw_token, keys);
    let res = sqlx::query(
        "INSERT INTO sessions \
         (token_hash, email, origin, ua, ip, created_at, last_seen_at, expires_at, revoked) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, ?7, 0)",
    )
    .bind(&hash)
    .bind(email)
    .bind(origin)
    .bind(ua)
    .bind(ip)
    .bind(&now)
    .bind(expires_at)
    .execute(db)
    .await?;
    Ok(res.last_insert_rowid())
}

/// Look up a live (non-revoked, non-expired) session by its raw cookie value,
/// bumping `last_seen_at`. Returns `None` when absent/revoked/expired.
pub async fn lookup_session(
    db: &SqlitePool,
    keys: &AuthKeys,
    raw_token: &str,
) -> anyhow::Result<Option<SessionRow>> {
    let hash = hash_token(raw_token, keys);
    let row: Option<SessionRow> = sqlx::query_as::<_, SessionRow>(
        "SELECT id, token_hash, email, origin, ua, ip, created_at, last_seen_at, \
                expires_at, revoked \
         FROM sessions WHERE token_hash = ?1 AND revoked = 0",
    )
    .bind(&hash)
    .fetch_optional(db)
    .await?;

    let Some(row) = row else { return Ok(None) };

    // Expiry check (lexicographic RFC3339 comparison is valid for UTC `Z` stamps).
    if let Some(exp) = &row.expires_at {
        let now = chrono::Utc::now().to_rfc3339();
        if exp.as_str() <= now.as_str() {
            return Ok(None);
        }
    }

    let now = chrono::Utc::now().to_rfc3339();
    let _ = sqlx::query("UPDATE sessions SET last_seen_at = ?1 WHERE id = ?2")
        .bind(&now)
        .bind(row.id)
        .execute(db)
        .await;

    Ok(Some(row))
}

/// Revoke a session by id (soft delete). Returns whether a row was affected.
pub async fn revoke_session(db: &SqlitePool, id: i64) -> anyhow::Result<bool> {
    let res = sqlx::query("UPDATE sessions SET revoked = 1 WHERE id = ?1")
        .bind(id)
        .execute(db)
        .await?;
    Ok(res.rows_affected() > 0)
}

/// List all sessions for `email` (or every session when `email` is `None`),
/// newest first.
pub async fn list_sessions(
    db: &SqlitePool,
    email: Option<&str>,
) -> anyhow::Result<Vec<SessionRow>> {
    let rows = match email {
        Some(e) => {
            sqlx::query_as::<_, SessionRow>(
                "SELECT id, token_hash, email, origin, ua, ip, created_at, last_seen_at, \
                        expires_at, revoked \
                 FROM sessions WHERE email = ?1 ORDER BY created_at DESC",
            )
            .bind(e)
            .fetch_all(db)
            .await?
        }
        None => {
            sqlx::query_as::<_, SessionRow>(
                "SELECT id, token_hash, email, origin, ua, ip, created_at, last_seen_at, \
                        expires_at, revoked \
                 FROM sessions ORDER BY created_at DESC",
            )
            .fetch_all(db)
            .await?
        }
    };
    Ok(rows)
}

/// Build a `Set-Cookie` header value that installs the session cookie.
///
/// `Secure` is only set when `secure` is true (i.e. when served over HTTPS, as
/// it is behind Tailscale serve); on plain loopback http we must omit it or the
/// browser drops the cookie. Always `HttpOnly`, `SameSite=Lax`, `Path=/`.
pub fn session_set_cookie_for(secure: bool, raw_token: &str, max_age_secs: i64) -> String {
    let mut c = format!(
        "{SESSION_COOKIE}={raw_token}; HttpOnly; SameSite=Lax; Path=/; Max-Age={max_age_secs}"
    );
    if secure {
        c.push_str("; Secure");
    }
    c
}

/// Build a `Set-Cookie` header value that clears the session cookie.
pub fn session_clear_cookie() -> String {
    format!("{SESSION_COOKIE}=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0")
}

/// Read a single cookie value by `name` from the request `Cookie` header.
pub fn read_cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    let raw = headers.get(axum::http::header::COOKIE)?.to_str().ok()?;
    for part in raw.split(';') {
        let part = part.trim();
        if let Some((k, v)) = part.split_once('=') {
            if k.trim() == name {
                return Some(v.trim().to_string());
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Settings helpers (NULL-aware upsert copied from evals_gcloud per conventions).
// ---------------------------------------------------------------------------

async fn read_global_setting(db: &SqlitePool, key: &str) -> anyhow::Result<Option<String>> {
    let v: Option<(String,)> = sqlx::query_as::<_, (String,)>(
        "SELECT value FROM settings WHERE scope = ?1 AND repo_id IS NULL AND key = ?2",
    )
    .bind(GLOBAL_SCOPE)
    .bind(key)
    .fetch_optional(db)
    .await?;
    Ok(v.map(|(s,)| s))
}

async fn upsert_global_setting(db: &SqlitePool, key: &str, value: &str) -> anyhow::Result<()> {
    let mut tx = db.begin().await?;
    let updated = sqlx::query(
        "UPDATE settings SET value = ?2 WHERE scope = ?1 AND repo_id IS NULL AND key = ?3",
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
