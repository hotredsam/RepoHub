//! Authentication middleware: request-origin classification, the [`gate`] layer,
//! the [`Principal`] extractor, and the destructive-action confirmation flow.
//!
//! P18/P19 foundation (library only — the `gate` layer is wired by the Integrate
//! step, not here). Three principals exist:
//! - [`Principal::Local`]   — an unauthenticated loopback caller (today's default).
//! - [`Principal::User`]    — a human authenticated via Tailscale identity header
//!                            or a valid session cookie. Trusted; exempt from the
//!                            Codex confirmation flow.
//! - [`Principal::Codex`]   — a ChatGPT Codex bearer token. Full access but every
//!                            destructive call is double-confirmed unless disabled.
//!
//! NOTE for the Codex feature agent: bearer validation currently lives inline in
//! [`validate_codex_bearer`] (querying `codex_credentials` directly). When the
//! `codex` module lands it should expose `crate::codex::validate_bearer(&AppState,
//! &str) -> Option<i64>` and this file should delegate to it.

use std::net::{IpAddr, SocketAddr};

use axum::extract::{ConnectInfo, FromRequestParts, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, Request};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use serde_json::json;

use crate::auth_session::{self, AuthKeys};
use crate::error::{AppError, ApiResult};
use crate::state::AppState;

/// Header Tailscale serve injects with the authenticated tailnet user's email.
const TS_USER_LOGIN: &str = "Tailscale-User-Login";

// Settings keys (all NULL-aware global; defaults chosen for graceful degradation).
const KEY_LOOPBACK_ALLOWED: &str = "auth_loopback_allowed"; // default true
const KEY_REMOTE_AUTH_REQUIRED: &str = "auth_remote_required"; // default true
const KEY_CODEX_CONFIRM: &str = "codex_destructive_confirm"; // default true
const GLOBAL_SCOPE: &str = "global";

/// Who is making a request, as determined by [`gate`] and stamped into request
/// extensions for the [`FromRequestParts`] extractor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Principal {
    /// Unauthenticated loopback caller (default local-only behavior).
    Local,
    /// Authenticated human; the inner string is their email/login.
    User(String),
    /// Authenticated ChatGPT Codex bearer credential.
    Codex,
}

/// Classification of where a request physically came from.
#[derive(Debug, Clone)]
pub struct RequestOrigin {
    /// True when the request did not originate from loopback (or carries remote
    /// proxy / Tailscale identity headers).
    pub remote: bool,
    /// The `Tailscale-User-Login` email, if Tailscale serve authenticated it.
    /// UNVERIFIED on its own — the gate corroborates it via Tailscale WhoIs
    /// before trusting it.
    pub tailscale_login: Option<String>,
    /// True when an upstream proxy reported the original scheme as https.
    pub forwarded_proto_https: bool,
    /// The original tailnet client IP, parsed from `Tailscale-Client-IP` (set by
    /// serve) or the first non-loopback `X-Forwarded-For` hop. Used to verify the
    /// asserted identity header via Tailscale WhoIs.
    pub forwarded_client_ip: Option<String>,
}

/// Inspect proxy/identity headers and the peer address to decide whether a
/// request is local (loopback, no remote markers) or remote.
pub fn classify_request(headers: &HeaderMap, peer: Option<SocketAddr>) -> RequestOrigin {
    let tailscale_login = headers
        .get(TS_USER_LOGIN)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let forwarded_proto_https = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.split(',').next().unwrap_or(s).trim().eq_ignore_ascii_case("https"))
        .unwrap_or(false)
        || headers
            .get("forwarded")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_ascii_lowercase().contains("proto=https"))
            .unwrap_or(false);

    // Any non-loopback forwarded client address means remote.
    let forwarded_remote = xff_has_non_loopback(headers) || forwarded_has_non_loopback(headers);

    // The peer address (when ConnectInfo is available). A non-loopback peer is
    // remote; absence is treated as local (CLI/tests connect on loopback).
    let peer_remote = peer.map(|p| !is_loopback_ip(p.ip())).unwrap_or(false);

    let remote = forwarded_remote || peer_remote || tailscale_login.is_some();

    let forwarded_client_ip = forwarded_client_ip(headers);

    RequestOrigin {
        remote,
        tailscale_login,
        forwarded_proto_https,
        forwarded_client_ip,
    }
}

/// The original tailnet client IP for WhoIs corroboration. Prefers the
/// `Tailscale-Client-IP` header serve injects alongside the identity headers;
/// falls back to the first non-loopback `X-Forwarded-For` hop.
fn forwarded_client_ip(headers: &HeaderMap) -> Option<String> {
    if let Some(ip) = headers
        .get("tailscale-client-ip")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        if ip.parse::<IpAddr>().is_ok() {
            return Some(ip.to_string());
        }
    }
    let raw = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok())?;
    raw.split(',')
        .map(|h| h.trim())
        .find(|h| match h.parse::<IpAddr>() {
            Ok(ip) => !is_loopback_ip(ip),
            Err(_) => false,
        })
        .map(|s| s.to_string())
}

fn is_loopback_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback(),
        IpAddr::V6(v6) => v6.is_loopback(),
    }
}

/// True when `X-Forwarded-For` lists any non-loopback client address.
fn xff_has_non_loopback(headers: &HeaderMap) -> bool {
    let Some(raw) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()) else {
        return false;
    };
    raw.split(',').any(|hop| {
        let hop = hop.trim();
        match hop.parse::<IpAddr>() {
            Ok(ip) => !is_loopback_ip(ip),
            // Unparseable token (e.g. obfuscated identifier) — treat as remote.
            Err(_) => !hop.is_empty(),
        }
    })
}

/// True when an RFC 7239 `Forwarded` header carries a non-loopback `for=` value.
fn forwarded_has_non_loopback(headers: &HeaderMap) -> bool {
    let Some(raw) = headers.get("forwarded").and_then(|v| v.to_str().ok()) else {
        return false;
    };
    let lowered = raw.to_ascii_lowercase();
    for part in lowered.split([';', ',']) {
        let part = part.trim();
        if let Some(rest) = part.strip_prefix("for=") {
            let val = rest.trim_matches('"').trim_start_matches('[');
            let host = val.split(']').next().unwrap_or(val);
            let host = host.rsplit_once(':').map(|(h, _)| h).unwrap_or(host);
            match host.parse::<IpAddr>() {
                Ok(ip) if is_loopback_ip(ip) => {}
                Ok(_) => return true,
                Err(_) if host == "localhost" || host.is_empty() || host == "unknown" => {}
                Err(_) => return true,
            }
        }
    }
    false
}

/// Whether `path` is a pre-authentication endpoint that MUST be reachable before
/// any principal exists. Deliberately a tight allowlist: the login handshake and
/// the unauthenticated status probe only. Everything else under `/api/auth/*`
/// (e.g. session enumeration/revocation, config edits) is privileged and is
/// gated like the rest of the app — the handlers enforce their own admin checks.
fn is_open_path(path: &str) -> bool {
    matches!(
        path,
        "/api/health"
            | "/api/auth/status"
            | "/api/auth/google/login"
            | "/api/auth/google/callback"
            | "/api/auth/logout"
    )
}

/// The axum auth gate. Wraps the whole app (wired by Integrate).
///
/// Policy:
/// - A tight set of pre-auth endpoints ([`is_open_path`]) is ALWAYS allowed so
///   the login handshake + status probe are reachable. NOTE: this is NOT all of
///   `/api/auth/*`; privileged auth routes (sessions list/revoke, config) are
///   gated and enforce their own admin checks.
/// - A local loopback caller, when `auth_loopback_allowed` (default true), passes
///   through as [`Principal::Local`] — preserving today's unauthenticated UX.
/// - Otherwise authentication is required:
///     * `Authorization: Bearer <codex>` validated → [`Principal::Codex`].
///     * `Tailscale-User-Login` (allowlisted) or a valid session cookie →
///       [`Principal::User`].
///     * Failing that: 401 JSON for `/api/*` and `/ws/*`; SPA paths pass through
///       (no principal) so the React app can render its Login screen.
///
/// Codex is forbidden outright from the interactive WebSocket RCE/agent surfaces
/// (`/ws/terminal`, `/ws/claude`): a bearer principal must never reach a raw PTY
/// or an interactive Claude session.
///
/// Any request the gate itself REJECTS (401/403) is recorded best-effort in the
/// audit log here, because the inner `record` layer never runs for short-circuited
/// responses (security-interesting probes — stale/guessed bearer replay,
/// post-revocation attempts — would otherwise leave no trace).
pub async fn gate(
    State(state): State<AppState>,
    mut req: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let path = req.uri().path().to_string();

    // Always-open pre-auth endpoints (tight allowlist).
    if is_open_path(&path) {
        return next.run(req).await;
    }

    let headers = req.headers().clone();
    let method = req.method().as_str().to_string();
    let query = req.uri().query().map(|s| s.to_string());
    let had_bearer = bearer_token(&headers).is_some();
    let peer = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ci| ci.0);
    let origin = classify_request(&headers, peer);

    // Local loopback passthrough (today's default).
    let local_ok = !origin.remote && loopback_allowed(&state).await;
    if local_ok {
        req.extensions_mut().insert(Principal::Local);
        return next.run(req).await;
    }

    // If remote auth has been explicitly disabled, treat everyone as Local. This
    // is the escape hatch for trusted-network deployments.
    if origin.remote && !remote_auth_required(&state).await {
        req.extensions_mut().insert(Principal::Local);
        return next.run(req).await;
    }

    // ---- authentication required ----

    // 1) Codex bearer. Delegated to the authoritative `codex` module (which also
    //    enforces the Codex enable switch + token-prefix check).
    if let Some(token) = bearer_token(&headers) {
        if crate::codex::validate_bearer(&state, &token).await.is_some() {
            // Codex must NEVER reach an interactive shell / agent socket. These
            // are direct RCE; deny before upgrade, recording the attempt.
            if is_codex_forbidden_ws(&path) {
                gate_audit(
                    &state, Some("codex"), "codex", &method, &path,
                    query.as_deref(), 403,
                )
                .await;
                return AppError::Forbidden(
                    "Codex is not permitted on interactive shell/agent endpoints".into(),
                )
                .into_response();
            }
            req.extensions_mut().insert(Principal::Codex);
            return next.run(req).await;
        }
    }

    // 2) Tailscale identity — only when corroborated by Tailscale WhoIs.
    //
    // The `Tailscale-User-Login` header is client-controllable (serve proxies
    // from loopback, so any local process can forge it). We trust it ONLY when a
    // Tailscale WhoIs of the forwarded client IP returns the SAME login AND that
    // login is allowlisted. A forged header with no matching tailnet peer fails.
    if let Some(login) = &origin.tailscale_login {
        if is_email_allowlisted(&state, login).await
            && tailscale_identity_verified(&origin, login).await
        {
            req.extensions_mut().insert(Principal::User(login.clone()));
            return next.run(req).await;
        }
    }
    if let Some(raw) = auth_session::read_cookie(&headers, auth_session::SESSION_COOKIE) {
        if let Ok(Some(row)) = auth_session::lookup_session(&state.db, &state.auth, &raw).await {
            req.extensions_mut().insert(Principal::User(row.email));
            return next.run(req).await;
        }
    }

    // ---- unauthenticated ----
    if path.starts_with("/api/") || path.starts_with("/ws/") {
        // Record the rejection (the inner `record` layer never runs for this
        // short-circuit). Mark a presented-but-invalid bearer as a Codex-style
        // auth attempt so post-revocation / brute-force probing is visible.
        let kind = if had_bearer { "codex" } else { "denied" };
        let actor = if had_bearer { Some("codex") } else { None };
        gate_audit(&state, actor, kind, &method, &path, query.as_deref(), 401).await;
        return AppError::Unauthorized("authentication required".into()).into_response();
    }
    // SPA route: let the frontend render its Login screen (no principal stamped).
    next.run(req).await
}

/// WebSocket paths a Codex bearer must never reach (raw PTY / interactive agent).
fn is_codex_forbidden_ws(path: &str) -> bool {
    matches!(path, "/ws/terminal" | "/ws/claude")
}

/// Best-effort audit row for a request the gate REJECTED before the inner audit
/// layer could run. Query strings are redacted (the callback `code`/`state` etc.
/// must never be persisted). Never panics; a logging failure is swallowed.
async fn gate_audit(
    state: &AppState,
    actor: Option<&str>,
    actor_kind: &str,
    method: &str,
    path: &str,
    query: Option<&str>,
    status: i64,
) {
    let redacted = query.map(crate::audit::redact_query);
    crate::audit::write_entry(
        &state.db,
        actor,
        actor_kind,
        method,
        path,
        redacted.as_deref(),
        status,
        None,
    )
    .await;
}

/// Extract the gate-assigned [`Principal`] from request extensions. Defaults to
/// [`Principal::Local`] if the gate did not run (e.g. tests, not-yet-wired).
#[async_trait::async_trait]
impl FromRequestParts<AppState> for Principal {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        _state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        Ok(parts
            .extensions
            .get::<Principal>()
            .cloned()
            .unwrap_or(Principal::Local))
    }
}

/// Whether unauthenticated loopback access is allowed (`auth_loopback_allowed`,
/// default `true`).
pub async fn loopback_allowed(state: &AppState) -> bool {
    read_global_bool(state, KEY_LOOPBACK_ALLOWED, true).await
}

/// Whether remote callers must authenticate (`auth_remote_required`, default
/// `true`).
pub async fn remote_auth_required(state: &AppState) -> bool {
    read_global_bool(state, KEY_REMOTE_AUTH_REQUIRED, true).await
}

/// Enforce the destructive-action confirmation flow for Codex callers.
///
/// - [`Principal::User`] and [`Principal::Local`] are exempt (returns `Ok`).
/// - [`Principal::Codex`], when `codex_destructive_confirm` (default true) is on,
///   must present a `supplied` confirmation token that is a valid HMAC signature
///   over `method|path|body_hash` and not older than 120s. If absent/invalid, a
///   409 [`AppError::Conflict`] is returned carrying a freshly minted
///   `confirm_token` the caller should echo back.
pub async fn require_confirmation(
    state: &AppState,
    principal: &Principal,
    method: &str,
    path: &str,
    body_hash: &str,
    supplied: Option<&str>,
) -> ApiResult<()> {
    match principal {
        Principal::User(_) | Principal::Local => return Ok(()),
        Principal::Codex => {}
    }

    if !read_global_bool(state, KEY_CODEX_CONFIRM, true).await {
        return Ok(());
    }

    let now = chrono::Utc::now().timestamp();
    if let Some(tok) = supplied {
        if verify_confirm_token(&state.auth, tok, method, path, body_hash, now) {
            return Ok(());
        }
    }

    let token = mint_confirm_token(&state.auth, method, path, body_hash, now);
    Err(AppError::Conflict(json!({
        "error": "confirmation required",
        "confirm_token": token,
        "expires_in": 120,
    })))
}

/// SHA-256 (lowercase hex) used to bind a confirmation token to an exact request
/// body. The Codex destructive-confirm flow hashes the body so a single
/// confirmation can only authorize the payload it was issued for.
pub fn body_hash(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    hex::encode(h.finalize())
}

/// Header a client may use to echo a confirmation token for endpoints that have
/// no (or no convenient) JSON body to carry it (e.g. `POST /api/repos/:id/pull`).
pub const CONFIRM_HEADER: &str = "x-confirm-token";

/// Read the confirmation token a client echoed in the [`CONFIRM_HEADER`].
pub fn confirm_token_header(headers: &HeaderMap) -> Option<String> {
    headers
        .get(CONFIRM_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Run [`require_confirmation`] for an endpoint identified solely by `method` and
/// `path` (no request body to bind). The supplied token typically comes from the
/// [`CONFIRM_HEADER`]. The body hash is a constant empty marker so the challenge
/// and retry bind identically.
pub async fn require_confirmation_pathonly(
    state: &AppState,
    principal: &Principal,
    method: &str,
    path: &str,
    supplied: Option<&str>,
) -> ApiResult<()> {
    require_confirmation(state, principal, method, path, "-", supplied).await
}

/// Run [`require_confirmation`] against a JSON request body whose own
/// `confirm_token` field is EXCLUDED from the bound hash.
///
/// This is the canonical way to guard a destructive Codex endpoint. The body MUST
/// be canonicalized identically on the challenge (token = None) and the retry
/// (token = Some) — otherwise the hash differs and the flow loops forever. We
/// achieve that here by serializing the body with its confirmation field forced to
/// `null` via the caller-supplied `strip` closure, so the hash is identical on
/// both passes regardless of whether the client echoed a token.
///
/// `supplied` is the token the client echoed back (typically the body's own
/// `confirm_token` field), and `canonical_json` is the body serialized with that
/// field cleared.
pub async fn require_confirmation_json(
    state: &AppState,
    principal: &Principal,
    method: &str,
    path: &str,
    canonical_json: &[u8],
    supplied: Option<&str>,
) -> ApiResult<()> {
    let bh = body_hash(canonical_json);
    require_confirmation(state, principal, method, path, &bh, supplied).await
}

const CONFIRM_TTL_SECS: i64 = 120;

/// Mint a confirmation token: `base64url(issued_ts).base64url(hmac)` binding the
/// issuance time to `method|path|body_hash`.
fn mint_confirm_token(
    keys: &AuthKeys,
    method: &str,
    path: &str,
    body_hash: &str,
    issued_ts: i64,
) -> String {
    let payload = confirm_payload(method, path, body_hash, issued_ts);
    let sig = keys.mac(payload.as_bytes());
    let ts_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(issued_ts.to_string());
    let sig_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sig);
    format!("{ts_b64}.{sig_b64}")
}

/// Verify a confirmation token against `method|path|body_hash`, rejecting tokens
/// whose issuance is in the future or older than [`CONFIRM_TTL_SECS`].
fn verify_confirm_token(
    keys: &AuthKeys,
    token: &str,
    method: &str,
    path: &str,
    body_hash: &str,
    now: i64,
) -> bool {
    let Some((ts_b64, sig_b64)) = token.split_once('.') else {
        return false;
    };
    let Ok(ts_bytes) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(ts_b64) else {
        return false;
    };
    let Ok(issued_ts) = String::from_utf8_lossy(&ts_bytes).parse::<i64>() else {
        return false;
    };
    if issued_ts > now || now - issued_ts > CONFIRM_TTL_SECS {
        return false;
    }
    let Ok(got_sig) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(sig_b64) else {
        return false;
    };
    let expected = keys.mac(confirm_payload(method, path, body_hash, issued_ts).as_bytes());
    constant_time_eq(&got_sig, &expected)
}

fn confirm_payload(method: &str, path: &str, body_hash: &str, issued_ts: i64) -> String {
    format!("confirm|{issued_ts}|{method}|{path}|{body_hash}")
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

// ---------------------------------------------------------------------------
// Auth helpers
// ---------------------------------------------------------------------------

/// Extract the value of an `Authorization: Bearer <token>` header.
fn bearer_token(headers: &HeaderMap) -> Option<String> {
    let raw = headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?;
    let token = raw.strip_prefix("Bearer ").or_else(|| raw.strip_prefix("bearer "))?;
    let token = token.trim();
    if token.is_empty() {
        None
    } else {
        Some(token.to_string())
    }
}

/// Corroborate an asserted `Tailscale-User-Login` against Tailscale's LocalAPI.
///
/// Returns true only when WhoIs of the forwarded client IP resolves to the SAME
/// login (case-insensitive). If we cannot determine the client IP, or WhoIs is
/// unavailable / returns a different login, the header is NOT trusted. This is the
/// difference between "the header says X" and "the connection provably came from
/// tailnet user X" — without it, any loopback process could impersonate any
/// allowlisted user by setting one header.
async fn tailscale_identity_verified(origin: &RequestOrigin, asserted_login: &str) -> bool {
    let Some(client_ip) = origin.forwarded_client_ip.as_deref() else {
        return false;
    };
    match crate::tailscale::whois_login(client_ip).await {
        Some(verified) => verified.eq_ignore_ascii_case(asserted_login.trim()),
        None => false,
    }
}

/// Whether `email` is on the auth allowlist (`auth_allowed_emails`, comma- or
/// whitespace-separated, case-insensitive). Empty allowlist denies (closed by
/// default once remote auth is in effect).
async fn is_email_allowlisted(state: &AppState, email: &str) -> bool {
    let Some(raw) = read_global_setting(state, "auth_allowed_emails").await else {
        return false;
    };
    let want = email.trim().to_ascii_lowercase();
    raw.split([',', ' ', '\n', '\t', ';'])
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .any(|allowed| allowed == want)
}

async fn read_global_bool(state: &AppState, key: &str, default: bool) -> bool {
    match read_global_setting(state, key).await {
        Some(v) => matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"),
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, SocketAddrV4};

    fn keys() -> AuthKeys {
        AuthKeys { signing_key: [7u8; 32] }
    }

    #[test]
    fn loopback_peer_is_local() {
        let headers = HeaderMap::new();
        let peer = SocketAddr::from(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 51000));
        let o = classify_request(&headers, Some(peer));
        assert!(!o.remote);
        assert!(o.tailscale_login.is_none());
    }

    #[test]
    fn non_loopback_peer_is_remote() {
        let headers = HeaderMap::new();
        let peer = SocketAddr::from(SocketAddrV4::new(Ipv4Addr::new(100, 64, 0, 5), 51000));
        let o = classify_request(&headers, Some(peer));
        assert!(o.remote);
    }

    #[test]
    fn tailscale_header_marks_remote_and_login() {
        let mut headers = HeaderMap::new();
        headers.insert(TS_USER_LOGIN, "sam@example.com".parse().unwrap());
        let o = classify_request(&headers, None);
        assert!(o.remote);
        assert_eq!(o.tailscale_login.as_deref(), Some("sam@example.com"));
    }

    #[test]
    fn xff_non_loopback_is_remote() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "100.64.0.7, 127.0.0.1".parse().unwrap());
        let o = classify_request(&headers, None);
        assert!(o.remote);
    }

    #[test]
    fn forwarded_proto_https_detected() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-proto", "https".parse().unwrap());
        let o = classify_request(&headers, None);
        assert!(o.forwarded_proto_https);
    }

    #[test]
    fn confirm_token_roundtrips_and_binds() {
        let k = keys();
        let now = 1_000_000i64;
        let tok = mint_confirm_token(&k, "POST", "/api/x", "abc", now);
        // Valid within TTL, same binding.
        assert!(verify_confirm_token(&k, &tok, "POST", "/api/x", "abc", now + 10));
        // Wrong path / method / body fails.
        assert!(!verify_confirm_token(&k, &tok, "POST", "/api/y", "abc", now + 10));
        assert!(!verify_confirm_token(&k, &tok, "GET", "/api/x", "abc", now + 10));
        assert!(!verify_confirm_token(&k, &tok, "POST", "/api/x", "zzz", now + 10));
        // Expired fails.
        assert!(!verify_confirm_token(&k, &tok, "POST", "/api/x", "abc", now + 121));
        // Garbage fails.
        assert!(!verify_confirm_token(&k, "not-a-token", "POST", "/api/x", "abc", now));
    }

    /// Regression for the destructive-confirm round trip: the body hash MUST be
    /// computed with `confirm_token` excluded so the challenge (token=None) and
    /// the retry (token=Some) bind to the SAME hash. If a handler hashed the body
    /// *including* the token, the retry would re-challenge forever.
    #[test]
    fn confirm_body_hash_excludes_token_so_retry_validates() {
        use serde::Serialize;
        #[derive(Serialize)]
        struct Body {
            ids: Vec<i64>,
            delete_local: bool,
            // Excluded from the canonical hash by being forced to None.
            confirm_token: Option<String>,
        }

        let k = keys();
        let now = 2_000_000i64;

        // Canonical body used for hashing on BOTH passes: token forced to None.
        let canonical = Body {
            ids: vec![1, 2],
            delete_local: true,
            confirm_token: None,
        };
        let bh = body_hash(&serde_json::to_vec(&canonical).unwrap());

        // Challenge issues a token bound to bh.
        let tok = mint_confirm_token(&k, "DELETE", "/api/repos", &bh, now);

        // The retry's body carries the echoed token, but we hash the canonical
        // form (token cleared) — so the hash matches and verification succeeds.
        let retry = Body {
            ids: vec![1, 2],
            delete_local: true,
            confirm_token: Some(tok.clone()),
        };
        let mut retry_canonical = retry;
        retry_canonical.confirm_token = None;
        let bh_retry = body_hash(&serde_json::to_vec(&retry_canonical).unwrap());
        assert_eq!(bh, bh_retry, "canonical hash must ignore confirm_token");
        assert!(verify_confirm_token(&k, &tok, "DELETE", "/api/repos", &bh_retry, now + 5));
    }
}
