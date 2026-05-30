//! P19 audit trail: the `GET /api/audit` query endpoint and the [`record`]
//! middleware that writes one `audit_log` row per request.
//!
//! Wiring (done by the Integrate step, not here):
//! - [`router`] is merged into the app like every other feature router.
//! - [`record`] is an axum layer applied *after* [`crate::auth_mw::gate`], so the
//!   [`Principal`] the gate stamped into request extensions is visible here.
//!
//! Cost model: only [`Principal::Codex`] requests pay for body buffering +
//! redaction (Codex is the powerful, externally-driven principal we most want a
//! durable record of). Local/User requests are logged cheaply with no body read,
//! preserving today's loopback performance.

use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::Request;
use axum::middleware::Next;
use axum::response::Response;
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::auth_mw::Principal;
use crate::error::{ApiResult, AppError};
use crate::state::AppState;

/// Reject [`Principal::Codex`] from owner-only routes. The audit trail contains
/// every user's email/IP/UA and request paths — a Codex bearer must never read it.
fn require_owner(principal: &Principal) -> ApiResult<()> {
    match principal {
        Principal::Codex => Err(AppError::Forbidden(
            "the audit log is owner-only".into(),
        )),
        Principal::Local | Principal::User(_) => Ok(()),
    }
}

/// Maximum number of request-body bytes buffered/excerpted for a Codex call.
const BODY_CAP: usize = 4096;

/// Keys whose values are scrubbed from any captured body excerpt. Matched
/// case-insensitively against JSON keys (and `key=value` form fields).
const REDACT_KEYS: &[&str] = &["client_secret", "token", "password", "id_token"];

/// Query-string parameter names whose values are bearer-equivalent secrets and
/// must never be persisted. The OAuth `code` is exchangeable for tokens within
/// its window; `state` is CSRF-relevant; the rest are obvious credentials.
const REDACT_QUERY_KEYS: &[&str] = &[
    "code",
    "state",
    "id_token",
    "access_token",
    "refresh_token",
    "token",
    "client_secret",
];

const REDACTED: &str = "[REDACTED]";

pub fn router() -> Router<AppState> {
    Router::new().route("/api/audit", get(list_audit))
}

// ---------------------------------------------------------------------------
// GET /api/audit
// ---------------------------------------------------------------------------

/// One row of the audit trail, as returned by `GET /api/audit`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct AuditEntry {
    pub id: i64,
    pub ts: String,
    pub actor: Option<String>,
    pub actor_kind: Option<String>,
    pub credential_id: Option<i64>,
    pub method: Option<String>,
    pub path: Option<String>,
    pub query: Option<String>,
    pub status: Option<i64>,
    pub summary: Option<String>,
    pub body_excerpt: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AuditQuery {
    /// Filter by actor (exact match, e.g. an email or `codex`).
    pub actor: Option<String>,
    /// Filter by HTTP method (case-insensitive).
    pub method: Option<String>,
    /// Filter by request path prefix.
    pub path: Option<String>,
    /// Cap the number of rows returned (1..=1000, default 200).
    pub limit: Option<i64>,
    /// Only rows with `ts >= since` (RFC3339; lexicographic compare on UTC stamps).
    pub since: Option<String>,
}

/// Return audit rows newest-first, applying any supplied filters. OWNER-ONLY.
async fn list_audit(
    State(state): State<AppState>,
    principal: Principal,
    Query(q): Query<AuditQuery>,
) -> ApiResult<Json<Vec<AuditEntry>>> {
    require_owner(&principal)?;

    let limit = q.limit.unwrap_or(200).clamp(1, 1000);

    // Build the WHERE clause dynamically; bind positionally to stay on runtime
    // sqlx (no query! macros, per conventions).
    let mut sql = String::from(
        "SELECT id, ts, actor, actor_kind, credential_id, method, path, query, \
                status, summary, body_excerpt \
         FROM audit_log WHERE 1 = 1",
    );

    let actor = q.actor.as_ref().map(|s| s.trim()).filter(|s| !s.is_empty());
    let method = q
        .method
        .as_ref()
        .map(|s| s.trim().to_ascii_uppercase())
        .filter(|s| !s.is_empty());
    let path_prefix = q.path.as_ref().map(|s| s.trim()).filter(|s| !s.is_empty());
    let since = q.since.as_ref().map(|s| s.trim()).filter(|s| !s.is_empty());

    let mut idx = 0;
    if actor.is_some() {
        idx += 1;
        sql.push_str(&format!(" AND actor = ?{idx}"));
    }
    if method.is_some() {
        idx += 1;
        sql.push_str(&format!(" AND UPPER(IFNULL(method,'')) = ?{idx}"));
    }
    let path_like = path_prefix.map(|p| format!("{p}%"));
    if path_like.is_some() {
        idx += 1;
        sql.push_str(&format!(" AND path LIKE ?{idx}"));
    }
    if since.is_some() {
        idx += 1;
        sql.push_str(&format!(" AND ts >= ?{idx}"));
    }
    idx += 1;
    sql.push_str(&format!(" ORDER BY ts DESC, id DESC LIMIT ?{idx}"));

    let mut query = sqlx::query_as::<_, AuditEntry>(&sql);
    if let Some(a) = actor {
        query = query.bind(a.to_string());
    }
    if let Some(m) = method {
        query = query.bind(m);
    }
    if let Some(p) = path_like {
        query = query.bind(p);
    }
    if let Some(s) = since {
        query = query.bind(s.to_string());
    }
    query = query.bind(limit);

    let rows = query.fetch_all(&state.db).await?;
    Ok(Json(rows))
}

// ---------------------------------------------------------------------------
// record — the auditing middleware (applied AFTER the gate)
// ---------------------------------------------------------------------------

/// axum layer that records one `audit_log` row per request.
///
/// Must run *after* [`crate::auth_mw::gate`] so the gate-stamped [`Principal`]
/// is present in request extensions. For [`Principal::Codex`] the request body
/// (capped at [`BODY_CAP`]) is buffered, excerpted with secrets redacted, and
/// then handed back to the downstream handler intact. Local/User requests are
/// logged without touching the body.
pub async fn record(
    State(state): State<AppState>,
    req: Request<Body>,
    next: Next,
) -> Response {
    let principal = req
        .extensions()
        .get::<Principal>()
        .cloned()
        .unwrap_or(Principal::Local);

    let method = req.method().as_str().to_string();
    let path = req.uri().path().to_string();
    // Redact sensitive query parameters (e.g. the OAuth callback `code`/`state`)
    // before they are ever persisted.
    let query = req.uri().query().map(redact_query);

    let (actor, actor_kind) = match &principal {
        Principal::Local => (None, "local".to_string()),
        Principal::User(email) => (Some(email.clone()), "user".to_string()),
        Principal::Codex => (Some("codex".to_string()), "codex".to_string()),
    };

    // Only Codex pays for body buffering + redaction — and never on the auth
    // handshake paths, whose bodies/queries can carry codes/secrets.
    let is_auth_path = path.starts_with("/api/auth/");
    let (req, body_excerpt) = if matches!(principal, Principal::Codex) && !is_auth_path {
        buffer_and_excerpt(req).await
    } else {
        (req, None)
    };

    let response = next.run(req).await;
    let status = response.status().as_u16() as i64;

    write_entry(
        &state.db,
        actor.as_deref(),
        &actor_kind,
        &method,
        &path,
        query.as_deref(),
        status,
        body_excerpt.as_deref(),
    )
    .await;

    response
}

/// Buffer the request body (capped) into an excerpt with secrets redacted, then
/// reconstruct the request so downstream handlers can still read the body.
///
/// On any failure (oversized body, read error) we keep the original request
/// flowing untouched and skip the excerpt rather than break the request.
async fn buffer_and_excerpt(req: Request<Body>) -> (Request<Body>, Option<String>) {
    let (parts, body) = req.into_parts();
    match axum::body::to_bytes(body, BODY_CAP).await {
        Ok(bytes) => {
            let excerpt = excerpt_body(&bytes);
            let req = Request::from_parts(parts, Body::from(bytes));
            (req, excerpt)
        }
        Err(_) => {
            // Body exceeded the cap or could not be read: do not buffer it (that
            // would consume it), let the original (now-empty) body proceed. We
            // record a marker instead of the content.
            let req = Request::from_parts(parts, Body::empty());
            (req, Some("[body unavailable: too large or unreadable]".to_string()))
        }
    }
}

/// Produce a redacted, bounded excerpt of a request body. Tries JSON first
/// (recursively redacting sensitive keys); falls back to a redacted plain-text
/// snippet for non-JSON bodies.
fn excerpt_body(bytes: &[u8]) -> Option<String> {
    if bytes.is_empty() {
        return None;
    }
    let text = String::from_utf8_lossy(bytes);

    if let Ok(mut val) = serde_json::from_str::<serde_json::Value>(&text) {
        redact_json(&mut val);
        let rendered = val.to_string();
        return Some(truncate(&rendered, BODY_CAP));
    }

    // Non-JSON (e.g. form-encoded or plain): scrub obvious `key=value` secrets.
    Some(truncate(&redact_form(&text), BODY_CAP))
}

/// Recursively replace the values of sensitive keys in a JSON value.
fn redact_json(val: &mut serde_json::Value) {
    match val {
        serde_json::Value::Object(map) => {
            for (k, v) in map.iter_mut() {
                if is_redact_key(k) {
                    *v = serde_json::Value::String(REDACTED.to_string());
                } else {
                    redact_json(v);
                }
            }
        }
        serde_json::Value::Array(items) => {
            for item in items.iter_mut() {
                redact_json(item);
            }
        }
        _ => {}
    }
}

/// Redact `key=value` pairs (form-encoded / query-like bodies) for sensitive
/// keys, leaving structure otherwise intact.
fn redact_form(text: &str) -> String {
    text.split('&')
        .map(|pair| match pair.split_once('=') {
            Some((k, _)) if is_redact_key(k.trim()) => format!("{k}={REDACTED}"),
            _ => pair.to_string(),
        })
        .collect::<Vec<_>>()
        .join("&")
}

fn is_redact_key(key: &str) -> bool {
    let key = key.trim().to_ascii_lowercase();
    REDACT_KEYS.iter().any(|k| key == *k)
}

/// Redact sensitive parameters from a raw query string before it is persisted to
/// `audit_log.query`. Preserves structure (`a=1&code=...&b=2`) while masking the
/// values of [`REDACT_QUERY_KEYS`] (OAuth `code`/`state`, tokens, secrets). Used
/// by both the [`record`] layer and the gate's rejection-audit path.
pub fn redact_query(raw: &str) -> String {
    raw.split('&')
        .map(|pair| match pair.split_once('=') {
            Some((k, _)) if is_redact_query_key(k) => format!("{k}={REDACTED}"),
            _ => pair.to_string(),
        })
        .collect::<Vec<_>>()
        .join("&")
}

fn is_redact_query_key(key: &str) -> bool {
    let key = key.trim().to_ascii_lowercase();
    REDACT_QUERY_KEYS.iter().any(|k| key == *k)
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    // Truncate on a char boundary at or below `max`.
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &s[..end])
}

/// Insert one row into `audit_log`. Best-effort: a logging failure never breaks
/// the request that is being recorded.
#[allow(clippy::too_many_arguments)]
pub async fn write_entry(
    db: &SqlitePool,
    actor: Option<&str>,
    actor_kind: &str,
    method: &str,
    path: &str,
    query: Option<&str>,
    status: i64,
    body_excerpt: Option<&str>,
) {
    let ts = chrono::Utc::now().to_rfc3339();
    let summary = format!("{method} {path} -> {status}");
    let res = sqlx::query(
        "INSERT INTO audit_log \
            (ts, actor, actor_kind, credential_id, method, path, query, status, summary, body_excerpt) \
         VALUES (?1, ?2, ?3, NULL, ?4, ?5, ?6, ?7, ?8, ?9)",
    )
    .bind(&ts)
    .bind(actor)
    .bind(actor_kind)
    .bind(method)
    .bind(path)
    .bind(query)
    .bind(status)
    .bind(&summary)
    .bind(body_excerpt)
    .execute(db)
    .await;
    if let Err(e) = res {
        tracing::warn!(error = %e, "failed to write audit_log row");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn redacts_sensitive_json_keys_recursively() {
        let mut v = json!({
            "client_secret": "abc",
            "nested": { "token": "xyz", "keep": "ok" },
            "list": [{ "password": "p" }, { "id_token": "t" }],
            "safe": "value"
        });
        redact_json(&mut v);
        assert_eq!(v["client_secret"], json!(REDACTED));
        assert_eq!(v["nested"]["token"], json!(REDACTED));
        assert_eq!(v["nested"]["keep"], json!("ok"));
        assert_eq!(v["list"][0]["password"], json!(REDACTED));
        assert_eq!(v["list"][1]["id_token"], json!(REDACTED));
        assert_eq!(v["safe"], json!("value"));
    }

    #[test]
    fn excerpt_redacts_and_is_some_for_json() {
        let body = br#"{"token":"deadbeef","keep":1}"#;
        let out = excerpt_body(body).expect("json body yields excerpt");
        assert!(out.contains(REDACTED));
        assert!(!out.contains("deadbeef"));
        assert!(out.contains("keep"));
    }

    #[test]
    fn excerpt_redacts_form_bodies() {
        let body = b"grant_type=authorization_code&client_secret=shhh&code=keepme";
        let out = excerpt_body(body).expect("form body yields excerpt");
        assert!(out.contains("client_secret=[REDACTED]"));
        assert!(!out.contains("shhh"));
        assert!(out.contains("code=keepme"));
    }

    #[test]
    fn empty_body_has_no_excerpt() {
        assert!(excerpt_body(b"").is_none());
    }

    #[test]
    fn truncate_respects_char_boundaries() {
        let s = "héllo wörld this is a long string";
        let t = truncate(s, 6);
        assert!(t.ends_with("..."));
        // Must be valid UTF-8 (no panic) and shorter than the original.
        assert!(t.len() <= s.len() + 3);
    }

    #[test]
    fn redact_key_match_is_case_insensitive() {
        assert!(is_redact_key("Client_Secret"));
        assert!(is_redact_key("TOKEN"));
        assert!(!is_redact_key("tokenish"));
        assert!(!is_redact_key("access"));
    }

    #[test]
    fn redact_query_masks_oauth_code_and_state() {
        let q = "code=4/abc.SECRET&state=csrf123&scope=openid";
        let out = redact_query(q);
        assert!(out.contains("code=[REDACTED]"));
        assert!(out.contains("state=[REDACTED]"));
        assert!(!out.contains("SECRET"));
        assert!(!out.contains("csrf123"));
        // Non-sensitive params are preserved verbatim.
        assert!(out.contains("scope=openid"));
    }

    #[test]
    fn redact_query_leaves_plain_params() {
        assert_eq!(redact_query("limit=50&actor=codex"), "limit=50&actor=codex");
    }
}
