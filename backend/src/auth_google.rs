//! P18 — Google OAuth 2.0 / OpenID Connect sign-in.
//!
//! Lets a remote (Tailscale) human prove who they are with their Google account
//! and, if allowlisted, mint a RepoHub session cookie. The whole module degrades
//! gracefully: with no client id/secret configured, [`load_oauth_config`] returns
//! `None`, `GET /api/auth/status` reports `configured: false`, and the login
//! endpoint answers `503`. Loopback users never need any of this — they are
//! `Principal::Local` and the gate lets them through unauthenticated.
//!
//! OIDC stack (per the Cargo.toml note, the *lighter* option): we drive the
//! authorization-code + PKCE handshake ourselves and verify Google's RS256 ID
//! token with `jsonwebtoken` against Google's JWKS (fetched + cached via
//! `reqwest`). `oauth2`'s `PkceCodeChallenge`/`CsrfToken` primitives generate the
//! PKCE verifier and CSRF state; the code-for-token exchange is a plain form POST
//! to Google's token endpoint (no full OIDC client crate needed).
//!
//! Security:
//! - The client secret is read from Google Secret Manager first, then a settings
//!   key, and is NEVER logged (no token, code, or secret is ever traced).
//! - Between the redirect to Google and the callback we stash the CSRF token and
//!   PKCE verifier in a short-lived (10 min), HMAC-signed, HttpOnly cookie — the
//!   server keeps no per-login state. The callback recomputes the HMAC to detect
//!   tampering and matches the returned `state` against the cookie's CSRF token.
//! - The allowlist (`auth_allowed_emails`) gates session creation; a verified but
//!   non-allowlisted email gets `403` and no cookie.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use base64::Engine as _;
use oauth2::{CsrfToken, PkceCodeChallenge};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::auth_mw::{self, classify_request, Principal};
use crate::auth_session::{self, AuthKeys};
use crate::error::{ApiResult, AppError};
use crate::state::AppState;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const GLOBAL_SCOPE: &str = "global";

/// Settings keys for the Google OAuth config block (all global, NULL repo_id).
const KEY_CLIENT_ID: &str = "auth_google_client_id";
const KEY_CLIENT_SECRET: &str = "auth_google_client_secret";
const KEY_REDIRECT_URI: &str = "auth_google_redirect_uri";
const KEY_ALLOWED_EMAILS: &str = "auth_allowed_emails";
const KEY_LOOPBACK_ALLOWED: &str = "auth_loopback_allowed";
const KEY_REMOTE_REQUIRED: &str = "auth_remote_required";

/// Google Secret Manager name checked before the settings-stored secret.
const SECRET_NAME: &str = "repohub-oauth-client-secret";

const GOOGLE_AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const GOOGLE_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const GOOGLE_JWKS_URL: &str = "https://www.googleapis.com/oauth2/v3/certs";
const GOOGLE_ISSUERS: [&str; 2] = ["https://accounts.google.com", "accounts.google.com"];

/// Cookie that carries the signed CSRF + PKCE state across the redirect.
const OAUTH_STATE_COOKIE: &str = "repohub_oauth";
/// How long an in-flight login may take before the state cookie is rejected.
const OAUTH_STATE_TTL_SECS: i64 = 600;
/// New sessions live for 30 days.
const SESSION_TTL_SECS: i64 = 60 * 60 * 24 * 30;
/// JWKS cache lifetime.
const JWKS_TTL: Duration = Duration::from_secs(3600);

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/auth/status", get(auth_status))
        .route("/api/auth/google/login", get(google_login))
        .route("/api/auth/google/callback", get(google_callback))
        .route("/api/auth/logout", post(logout))
        .route("/api/auth/sessions", get(list_sessions))
        .route("/api/auth/sessions/:id/revoke", post(revoke_session))
        .route("/api/auth/config", put(put_config))
}

// ---------------------------------------------------------------------------
// Config + allowlist
// ---------------------------------------------------------------------------

/// Resolved Google OAuth configuration. Present only when the client id and a
/// secret are both available.
#[derive(Debug, Clone)]
pub struct OAuthConfig {
    pub client_id: String,
    pub client_secret: String,
    pub redirect_uri: String,
}

/// Load the Google OAuth config, or `None` when not configured.
///
/// The client secret comes from Google Secret Manager (`repohub-oauth-client-secret`)
/// first; if that is unavailable (gcloud not configured, secret absent) we fall
/// back to the `auth_google_client_secret` settings key. The client id and
/// redirect uri come from settings. Never logs the secret.
pub async fn load_oauth_config(state: &AppState) -> Option<OAuthConfig> {
    let client_id = read_global_setting(state, KEY_CLIENT_ID).await?;

    // Secret Manager first (degrades gracefully to None on any error), then the
    // settings fallback. We do not log either path's failure detail.
    let client_secret = match crate::gcloud::secret_get(&state.db, SECRET_NAME).await {
        Ok(s) if !s.trim().is_empty() => Some(s),
        _ => read_global_setting(state, KEY_CLIENT_SECRET).await,
    }?;

    // Redirect uri: prefer the configured value; otherwise derive a sensible
    // tailnet default so a fresh install only needs id + secret.
    let redirect_uri = read_global_setting(state, KEY_REDIRECT_URI)
        .await
        .unwrap_or_else(default_redirect_uri);

    Some(OAuthConfig {
        client_id,
        client_secret,
        redirect_uri,
    })
}

/// Default callback URL behind Tailscale serve (https on this Mac's tailnet name).
fn default_redirect_uri() -> String {
    "https://samuels-mac-mini.tail97ef37.ts.net/api/auth/google/callback".to_string()
}

/// The raw `auth_allowed_emails` setting value (may be empty / absent).
pub async fn allowed_emails(state: &AppState) -> Vec<String> {
    match read_global_setting(state, KEY_ALLOWED_EMAILS).await {
        Some(raw) => raw
            .split([',', ' ', '\n', '\t', ';'])
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        None => Vec::new(),
    }
}

/// Whether `email` is allowlisted (case-insensitive). An empty allowlist denies.
pub async fn email_allowed(state: &AppState, email: &str) -> bool {
    let want = email.trim().to_ascii_lowercase();
    if want.is_empty() {
        return false;
    }
    allowed_emails(state)
        .await
        .iter()
        .any(|allowed| allowed.to_ascii_lowercase() == want)
}

// ---------------------------------------------------------------------------
// GET /api/auth/status
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct AuthStatus {
    /// Whether Google OAuth is configured (client id + secret resolvable).
    configured: bool,
    /// Whether this request looks remote (non-loopback / Tailscale / proxied).
    remote: bool,
    /// Whether unauthenticated loopback access is allowed (default true).
    loopback_allowed: bool,
    /// Whether remote callers must authenticate (default true).
    remote_required: bool,
    /// Whether the caller is currently authenticated as a user.
    authenticated: bool,
    /// The authenticated email, when known.
    email: Option<String>,
    /// The configured allowlist (so the Settings UI can render/edit it).
    allowed_emails: Vec<String>,
    /// The URL the frontend should send the browser to in order to log in.
    login_url: String,
}

/// GET /api/auth/status — describe auth configuration + the caller's state.
///
/// Always 200, even when nothing is configured, so the SPA can decide whether to
/// show a Login button. The `gate` allowlists `/api/auth/*`, so this is reachable
/// pre-authentication; we re-derive the principal from the session cookie /
/// Tailscale header ourselves rather than relying on a stamped extension.
async fn auth_status(State(state): State<AppState>, headers: HeaderMap) -> Json<AuthStatus> {
    let origin = classify_request(&headers, None);
    let configured = load_oauth_config(&state).await.is_some();

    // Resolve the current principal from the same signals the gate uses.
    let (authenticated, email) = current_user(&state, &headers).await;

    Json(AuthStatus {
        configured,
        remote: origin.remote,
        loopback_allowed: auth_mw::loopback_allowed(&state).await,
        remote_required: auth_mw::remote_auth_required(&state).await,
        authenticated,
        email,
        allowed_emails: allowed_emails(&state).await,
        login_url: "/api/auth/google/login".to_string(),
    })
}

/// Best-effort "who is calling": a live session cookie, else an allowlisted
/// Tailscale identity. Returns `(authenticated, email)`.
async fn current_user(state: &AppState, headers: &HeaderMap) -> (bool, Option<String>) {
    if let Some(raw) = auth_session::read_cookie(headers, auth_session::SESSION_COOKIE) {
        if let Ok(Some(row)) = auth_session::lookup_session(&state.db, &state.auth, &raw).await {
            return (true, Some(row.email));
        }
    }
    let origin = classify_request(headers, None);
    if let Some(login) = origin.tailscale_login {
        if email_allowed(state, &login).await {
            return (true, Some(login));
        }
    }
    (false, None)
}

// ---------------------------------------------------------------------------
// GET /api/auth/google/login
// ---------------------------------------------------------------------------

/// GET /api/auth/google/login — redirect the browser to Google's consent screen
/// with PKCE + CSRF state. 503 when OAuth is not configured.
async fn google_login(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let Some(cfg) = load_oauth_config(&state).await else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "Google OAuth is not configured" })),
        )
            .into_response();
    };

    let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
    let csrf = CsrfToken::new_random();

    // Pack {csrf|verifier|issued_ts} into a signed, short-lived cookie. We keep no
    // server-side login state; the callback validates the HMAC and the CSRF match.
    let payload = make_state_cookie(&state.auth, csrf.secret(), verifier.secret());

    let secure = classify_request(&headers, None).forwarded_proto_https;
    let set_cookie = make_state_set_cookie(&payload, secure);

    let auth_url = build_authorize_url(&cfg, csrf.secret(), challenge.as_str());

    // Redirect + Set-Cookie together.
    let mut resp = Redirect::to(&auth_url).into_response();
    if let Ok(v) = header::HeaderValue::from_str(&set_cookie) {
        resp.headers_mut().append(header::SET_COOKIE, v);
    }
    resp
}

/// Build the Google authorization URL (offline-free; we only need the ID token).
fn build_authorize_url(cfg: &OAuthConfig, csrf: &str, code_challenge: &str) -> String {
    let mut url = url_with_query(GOOGLE_AUTH_URL);
    url.push_pair("client_id", &cfg.client_id);
    url.push_pair("redirect_uri", &cfg.redirect_uri);
    url.push_pair("response_type", "code");
    url.push_pair("scope", "openid email profile");
    url.push_pair("state", csrf);
    url.push_pair("code_challenge", code_challenge);
    url.push_pair("code_challenge_method", "S256");
    url.push_pair("prompt", "select_account");
    url.finish()
}

// ---------------------------------------------------------------------------
// GET /api/auth/google/callback
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

/// GET /api/auth/google/callback — exchange the code, verify the ID token,
/// allowlist-gate, and (on success) mint a session cookie + 302 to `/`.
async fn google_callback(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<CallbackQuery>,
) -> Response {
    if let Some(err) = q.error.as_deref() {
        // User denied consent or Google reported an error. Don't echo arbitrary
        // provider strings into anything sensitive; just surface a 400.
        tracing::warn!(reason = %sanitize(err), "google oauth callback returned error");
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "google sign-in was cancelled or failed" })),
        )
            .into_response();
    }

    let (Some(code), Some(returned_state)) = (q.code.as_deref(), q.state.as_deref()) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "missing code or state" })),
        )
            .into_response();
    };

    let Some(cfg) = load_oauth_config(&state).await else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "Google OAuth is not configured" })),
        )
            .into_response();
    };

    // Recover + verify the CSRF/PKCE state cookie.
    let Some(raw_state) = auth_session::read_cookie(&headers, OAUTH_STATE_COOKIE) else {
        return unauthorized("missing oauth state cookie");
    };
    let Some((csrf, verifier)) = parse_state_cookie(&state.auth, &raw_state) else {
        return unauthorized("invalid or expired oauth state");
    };
    if !constant_time_eq(csrf.as_bytes(), returned_state.as_bytes()) {
        return unauthorized("oauth state mismatch");
    }

    // Exchange the authorization code for tokens (PKCE verifier proves it's us).
    let id_token = match exchange_code(&cfg, code, &verifier).await {
        Ok(tok) => tok,
        Err(e) => {
            tracing::warn!(error = %e, "oauth code exchange failed");
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": "failed to exchange authorization code" })),
            )
                .into_response();
        }
    };

    // Verify the ID token (sig/iss/aud/exp/email_verified) -> trusted email.
    let email = match verify_id_token(&id_token, &cfg.client_id).await {
        Ok(email) => email,
        Err(e) => {
            tracing::warn!(error = %e, "id token verification failed");
            return unauthorized("could not verify Google identity");
        }
    };

    // Allowlist gate.
    if !email_allowed(&state, &email).await {
        tracing::warn!(email = %sanitize(&email), "login denied: email not allowlisted");
        let mut resp = (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "this Google account is not allowed" })),
        )
            .into_response();
        clear_state_cookie(&mut resp);
        return resp;
    }

    // Mint the session.
    let raw_token = auth_session::new_token();
    let now = chrono::Utc::now();
    let expires_at = (now + chrono::Duration::seconds(SESSION_TTL_SECS)).to_rfc3339();
    let ua = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok());
    if let Err(e) = auth_session::create_session(
        &state.db,
        &state.auth,
        &raw_token,
        &email,
        Some("google"),
        ua,
        None,
        Some(&expires_at),
    )
    .await
    {
        tracing::error!(error = %e, "failed to persist session");
        return AppError::msg("failed to create session").into_response();
    }

    let secure = classify_request(&headers, None).forwarded_proto_https;
    let set_cookie = auth_session::session_set_cookie_for(secure, &raw_token, SESSION_TTL_SECS);

    let mut resp = Redirect::to("/").into_response();
    append_set_cookie(&mut resp, &set_cookie);
    clear_state_cookie(&mut resp);
    resp
}

/// POST a code-for-token exchange to Google and return the raw `id_token`.
async fn exchange_code(
    cfg: &OAuthConfig,
    code: &str,
    verifier: &str,
) -> anyhow::Result<String> {
    let client = http_client()?;
    let params = [
        ("grant_type", "authorization_code"),
        ("code", code),
        ("client_id", cfg.client_id.as_str()),
        ("client_secret", cfg.client_secret.as_str()),
        ("redirect_uri", cfg.redirect_uri.as_str()),
        ("code_verifier", verifier),
    ];
    let resp = client
        .post(GOOGLE_TOKEN_URL)
        .form(&params)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("token request failed: {e}"))?;

    if !resp.status().is_success() {
        // The body may contain provider error codes but not our secrets; still,
        // we never log it verbatim.
        return Err(anyhow::anyhow!(
            "token endpoint returned status {}",
            resp.status()
        ));
    }

    let body: TokenResponse = resp
        .json()
        .await
        .map_err(|e| anyhow::anyhow!("invalid token response: {e}"))?;

    body.id_token
        .ok_or_else(|| anyhow::anyhow!("token response missing id_token"))
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    id_token: Option<String>,
}

// ---------------------------------------------------------------------------
// ID token verification (RS256 against Google's JWKS)
// ---------------------------------------------------------------------------

/// Claims we read out of Google's ID token.
#[derive(Debug, Deserialize)]
struct IdClaims {
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    email_verified: Option<BoolOrString>,
}

/// Google sometimes serializes `email_verified` as a JSON bool and sometimes as
/// the string `"true"`. Accept both.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum BoolOrString {
    Bool(bool),
    Str(String),
}

impl BoolOrString {
    fn is_true(&self) -> bool {
        match self {
            BoolOrString::Bool(b) => *b,
            BoolOrString::Str(s) => s.eq_ignore_ascii_case("true"),
        }
    }
}

/// Verify a Google ID token's signature (RS256 via JWKS), issuer, audience, and
/// expiry; require a verified email. Returns the email on success.
pub async fn verify_id_token(id_token: &str, client_id: &str) -> anyhow::Result<String> {
    use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};

    let header = decode_header(id_token).map_err(|e| anyhow::anyhow!("bad jwt header: {e}"))?;
    let kid = header
        .kid
        .ok_or_else(|| anyhow::anyhow!("id token missing kid"))?;

    let jwk = jwks_lookup(&kid).await?;
    let key = DecodingKey::from_rsa_components(&jwk.n, &jwk.e)
        .map_err(|e| anyhow::anyhow!("bad jwk: {e}"))?;

    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_audience(&[client_id]);
    validation.set_issuer(&GOOGLE_ISSUERS);
    validation.set_required_spec_claims(&["exp", "iss", "aud"]);
    // `validate_exp` defaults to true.

    let data = decode::<IdClaims>(id_token, &key, &validation)
        .map_err(|e| anyhow::anyhow!("id token invalid: {e}"))?;

    let claims = data.claims;
    let verified = claims
        .email_verified
        .as_ref()
        .map(BoolOrString::is_true)
        .unwrap_or(false);
    if !verified {
        return Err(anyhow::anyhow!("email not verified by Google"));
    }
    let email = claims
        .email
        .filter(|e| !e.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("id token has no email"))?;

    Ok(email.trim().to_string())
}

/// One RSA JWK (the subset we need to build a verifying key).
#[derive(Debug, Clone, Deserialize)]
struct Jwk {
    kid: String,
    n: String,
    e: String,
}

#[derive(Debug, Deserialize)]
struct Jwks {
    keys: Vec<Jwk>,
}

/// Process-wide JWKS cache: `(fetched_at, keys)`.
static JWKS_CACHE: std::sync::OnceLock<Arc<Mutex<Option<(Instant, Vec<Jwk>)>>>> =
    std::sync::OnceLock::new();

fn jwks_cache() -> &'static Arc<Mutex<Option<(Instant, Vec<Jwk>)>>> {
    JWKS_CACHE.get_or_init(|| Arc::new(Mutex::new(None)))
}

/// Look up a JWK by `kid`, using the cache and refreshing once on a miss (keys
/// rotate, so a cache miss may simply mean we need to refetch).
async fn jwks_lookup(kid: &str) -> anyhow::Result<Jwk> {
    let cache = jwks_cache();

    // Fast path: fresh cache hit.
    {
        let guard = cache.lock().await;
        if let Some((fetched, keys)) = guard.as_ref() {
            if fetched.elapsed() < JWKS_TTL {
                if let Some(j) = keys.iter().find(|k| k.kid == kid) {
                    return Ok(j.clone());
                }
            }
        }
    }

    // Slow path: refetch and retry.
    let keys = fetch_jwks().await?;
    {
        let mut guard = cache.lock().await;
        *guard = Some((Instant::now(), keys.clone()));
    }
    keys.into_iter()
        .find(|k| k.kid == kid)
        .ok_or_else(|| anyhow::anyhow!("no matching JWKS key for kid"))
}

async fn fetch_jwks() -> anyhow::Result<Vec<Jwk>> {
    let client = http_client()?;
    let resp = client
        .get(GOOGLE_JWKS_URL)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("jwks request failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(anyhow::anyhow!("jwks endpoint status {}", resp.status()));
    }
    let jwks: Jwks = resp
        .json()
        .await
        .map_err(|e| anyhow::anyhow!("invalid jwks response: {e}"))?;
    Ok(jwks.keys)
}

/// Shared reqwest client (rustls, short timeouts). Built per call — cheap enough
/// for the auth path and avoids a global the gate would need to thread through.
fn http_client() -> anyhow::Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| anyhow::anyhow!("http client build failed: {e}"))
}

// ---------------------------------------------------------------------------
// POST /api/auth/logout
// ---------------------------------------------------------------------------

/// POST /api/auth/logout — revoke the caller's session (if any) and clear the
/// cookie. Idempotent; always 200.
async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(raw) = auth_session::read_cookie(&headers, auth_session::SESSION_COOKIE) {
        if let Ok(Some(row)) = auth_session::lookup_session(&state.db, &state.auth, &raw).await {
            let _ = auth_session::revoke_session(&state.db, row.id).await;
        }
    }
    let mut resp = Json(json!({ "ok": true })).into_response();
    append_set_cookie(&mut resp, &auth_session::session_clear_cookie());
    resp
}

// ---------------------------------------------------------------------------
// GET /api/auth/sessions  +  POST /api/auth/sessions/:id/revoke
// ---------------------------------------------------------------------------

/// One session as surfaced to the UI. We deliberately OMIT `token_hash`.
#[derive(Debug, Serialize)]
struct SessionView {
    id: i64,
    email: String,
    origin: Option<String>,
    ua: Option<String>,
    ip: Option<String>,
    created_at: String,
    last_seen_at: Option<String>,
    expires_at: Option<String>,
    revoked: bool,
    /// True when this row is the caller's current session.
    current: bool,
}

/// GET /api/auth/sessions — list all sessions (newest first), flagging the
/// caller's own. ADMIN-ONLY: an authenticated user or loopback admin; Codex is
/// rejected. Without this check a remote, unauthenticated caller could enumerate
/// every user's email/IP/UA. We still avoid leaking the token hash.
async fn list_sessions(
    State(state): State<AppState>,
    principal: Principal,
    headers: HeaderMap,
) -> ApiResult<Json<Vec<SessionView>>> {
    require_admin(&state, &principal, &headers).await?;

    let current_id = match auth_session::read_cookie(&headers, auth_session::SESSION_COOKIE) {
        Some(raw) => auth_session::lookup_session(&state.db, &state.auth, &raw)
            .await
            .ok()
            .flatten()
            .map(|r| r.id),
        None => None,
    };

    let rows = auth_session::list_sessions(&state.db, None)
        .await
        .map_err(AppError::Anyhow)?;

    let views = rows
        .into_iter()
        .map(|r| SessionView {
            current: Some(r.id) == current_id,
            id: r.id,
            email: r.email,
            origin: r.origin,
            ua: r.ua,
            ip: r.ip,
            created_at: r.created_at,
            last_seen_at: r.last_seen_at,
            expires_at: r.expires_at,
            revoked: r.revoked != 0,
        })
        .collect();

    Ok(Json(views))
}

/// POST /api/auth/sessions/:id/revoke — revoke one session by id.
///
/// AUTHORIZED callers only. A full admin (an authenticated [`Principal::User`] or
/// the loopback admin) may revoke ANY session. A non-admin authenticated caller
/// may revoke ONLY their own current session. Codex and unauthenticated remote
/// callers are rejected — otherwise anyone reaching the port could force-logout
/// the admin by iterating ids.
async fn revoke_session(
    State(state): State<AppState>,
    principal: Principal,
    headers: HeaderMap,
    axum::extract::Path(id): axum::extract::Path<i64>,
) -> ApiResult<Json<Value>> {
    // Codex is never allowed to touch sessions.
    if matches!(principal, Principal::Codex) {
        return Err(AppError::Forbidden("Codex may not manage sessions".into()));
    }

    // Full admins may revoke any id; otherwise the caller may only revoke their
    // own current session.
    if require_admin(&state, &principal, &headers).await.is_err() {
        let own_id = own_session_id(&state, &headers).await;
        if own_id != Some(id) {
            return Err(AppError::Forbidden(
                "you may only revoke your own session".into(),
            ));
        }
    }

    let revoked = auth_session::revoke_session(&state.db, id)
        .await
        .map_err(AppError::Anyhow)?;
    if !revoked {
        return Err(AppError::msg(format!("session {id} not found")));
    }
    Ok(Json(json!({ "revoked": id })))
}

/// The id of the caller's own live session (from the session cookie), if any.
async fn own_session_id(state: &AppState, headers: &HeaderMap) -> Option<i64> {
    let raw = auth_session::read_cookie(headers, auth_session::SESSION_COOKIE)?;
    auth_session::lookup_session(&state.db, &state.auth, &raw)
        .await
        .ok()
        .flatten()
        .map(|r| r.id)
}

// ---------------------------------------------------------------------------
// PUT /api/auth/config
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ConfigBody {
    /// Replace the allowlist. Accepts comma/space/`;`-separated input; stored
    /// normalized (comma-separated, trimmed).
    #[serde(default)]
    allowed_emails: Option<String>,
    /// Toggle unauthenticated loopback access.
    #[serde(default)]
    loopback_allowed: Option<bool>,
    /// Toggle the remote-auth requirement.
    #[serde(default)]
    remote_required: Option<bool>,
}

/// PUT /api/auth/config — update the allowlist + loopback/remote toggles.
///
/// This is a privileged operation. We require the caller to be a real user, OR a
/// loopback admin when loopback access is allowed (today's local-first default).
/// Codex is not permitted to widen its own access.
async fn put_config(
    State(state): State<AppState>,
    principal: Principal,
    headers: HeaderMap,
    Json(body): Json<ConfigBody>,
) -> ApiResult<Json<AuthStatusConfig>> {
    require_admin(&state, &principal, &headers).await?;

    if let Some(raw) = body.allowed_emails.as_deref() {
        let normalized = raw
            .split([',', ' ', '\n', '\t', ';'])
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(",");
        upsert_global_setting(&state, KEY_ALLOWED_EMAILS, &normalized).await?;
    }
    if let Some(v) = body.loopback_allowed {
        upsert_global_setting(&state, KEY_LOOPBACK_ALLOWED, bool_str(v)).await?;
    }
    if let Some(v) = body.remote_required {
        upsert_global_setting(&state, KEY_REMOTE_REQUIRED, bool_str(v)).await?;
    }

    Ok(Json(AuthStatusConfig {
        allowed_emails: allowed_emails(&state).await,
        loopback_allowed: auth_mw::loopback_allowed(&state).await,
        remote_required: auth_mw::remote_auth_required(&state).await,
    }))
}

#[derive(Debug, Serialize)]
struct AuthStatusConfig {
    allowed_emails: Vec<String>,
    loopback_allowed: bool,
    remote_required: bool,
}

/// Authorize a config mutation: an authenticated user is fine; a loopback caller
/// is fine while loopback access is allowed (local-first admin). Anything else is
/// forbidden — notably Codex must not edit the allowlist.
async fn require_admin(
    state: &AppState,
    principal: &Principal,
    headers: &HeaderMap,
) -> ApiResult<()> {
    match principal {
        Principal::User(_) => Ok(()),
        Principal::Local => {
            // `gate` stamps `Local` for loopback passthrough. If the gate did not
            // run (e.g. routed directly), re-derive: a non-remote caller with
            // loopback allowed is the local admin.
            let origin = classify_request(headers, None);
            if !origin.remote && auth_mw::loopback_allowed(state).await {
                Ok(())
            } else {
                // A real user could still be present via session cookie even if
                // the extractor defaulted to Local.
                let (authed, _) = current_user(state, headers).await;
                if authed {
                    Ok(())
                } else {
                    Err(AppError::Forbidden("admin access required".into()))
                }
            }
        }
        Principal::Codex => Err(AppError::Forbidden(
            "Codex may not change auth configuration".into(),
        )),
    }
}

// ---------------------------------------------------------------------------
// State-cookie signing (CSRF + PKCE verifier)
// ---------------------------------------------------------------------------

/// Build the signed oauth state cookie payload (the cookie *value*, not the full
/// Set-Cookie line). Layout:
/// `base64url(issued_ts).base64url(csrf).base64url(verifier).base64url(hmac)`.
fn make_state_cookie(keys: &AuthKeys, csrf: &str, verifier: &str) -> String {
    let issued = chrono::Utc::now().timestamp();
    let ts_b64 = b64(issued.to_string().as_bytes());
    let csrf_b64 = b64(csrf.as_bytes());
    let ver_b64 = b64(verifier.as_bytes());
    let signed_payload = format!("oauth|{issued}|{csrf}|{verifier}");
    let sig = b64(&keys.mac(signed_payload.as_bytes()));
    format!("{ts_b64}.{csrf_b64}.{ver_b64}.{sig}")
}

/// Verify + parse the oauth state cookie. Returns `(csrf, verifier)` when the
/// HMAC checks out and the cookie is within [`OAUTH_STATE_TTL_SECS`].
fn parse_state_cookie(keys: &AuthKeys, raw: &str) -> Option<(String, String)> {
    let mut parts = raw.split('.');
    let ts_b64 = parts.next()?;
    let csrf_b64 = parts.next()?;
    let ver_b64 = parts.next()?;
    let sig_b64 = parts.next()?;
    if parts.next().is_some() {
        return None;
    }

    let issued: i64 = String::from_utf8(unb64(ts_b64)?).ok()?.parse().ok()?;
    let csrf = String::from_utf8(unb64(csrf_b64)?).ok()?;
    let verifier = String::from_utf8(unb64(ver_b64)?).ok()?;
    let got_sig = unb64(sig_b64)?;

    let now = chrono::Utc::now().timestamp();
    if issued > now || now - issued > OAUTH_STATE_TTL_SECS {
        return None;
    }

    let signed_payload = format!("oauth|{issued}|{csrf}|{verifier}");
    let expected = keys.mac(signed_payload.as_bytes());
    if !constant_time_eq(&got_sig, &expected) {
        return None;
    }

    Some((csrf, verifier))
}

/// Build the full `Set-Cookie` line installing the oauth state cookie.
fn make_state_set_cookie(payload: &str, secure: bool) -> String {
    let mut c = format!(
        "{OAUTH_STATE_COOKIE}={payload}; HttpOnly; SameSite=Lax; Path=/; Max-Age={OAUTH_STATE_TTL_SECS}"
    );
    if secure {
        c.push_str("; Secure");
    }
    c
}

/// Append a `Set-Cookie` clearing the oauth state cookie to `resp`.
fn clear_state_cookie(resp: &mut Response) {
    let cleared = format!("{OAUTH_STATE_COOKIE}=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0");
    if let Ok(v) = header::HeaderValue::from_str(&cleared) {
        resp.headers_mut().append(header::SET_COOKIE, v);
    }
}

/// Append a complete `Set-Cookie` line to `resp` (no-op if it isn't a valid
/// header value, which it always is for our generated cookies).
fn append_set_cookie(resp: &mut Response, set_cookie: &str) {
    if let Ok(v) = header::HeaderValue::from_str(set_cookie) {
        resp.headers_mut().append(header::SET_COOKIE, v);
    }
}

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

fn unauthorized(msg: &str) -> Response {
    AppError::Unauthorized(msg.to_string()).into_response()
}

fn bool_str(v: bool) -> &'static str {
    if v {
        "true"
    } else {
        "false"
    }
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn unb64(s: &str) -> Option<Vec<u8>> {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(s).ok()
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

/// Strip control characters / newlines from provider-supplied strings before
/// they ever touch a log line.
fn sanitize(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control())
        .take(200)
        .collect()
}

// --- minimal query-string builder (avoids pulling `url` for one call) ---

struct QueryBuilder {
    base: String,
    pairs: Vec<(String, String)>,
}

fn url_with_query(base: &str) -> QueryBuilder {
    QueryBuilder {
        base: base.to_string(),
        pairs: Vec::new(),
    }
}

impl QueryBuilder {
    fn push_pair(&mut self, k: &str, v: &str) {
        self.pairs.push((k.to_string(), v.to_string()));
    }

    fn finish(self) -> String {
        let mut out = self.base;
        let mut first = true;
        for (k, v) in self.pairs {
            out.push(if first { '?' } else { '&' });
            first = false;
            out.push_str(&urlencode(&k));
            out.push('=');
            out.push_str(&urlencode(&v));
        }
        out
    }
}

/// Percent-encode per application/x-www-form-urlencoded (RFC 3986 unreserved set
/// kept literal; everything else hex-escaped). Spaces become `%20` (not `+`),
/// which is valid in a query string.
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                out.push('%');
                out.push_str(&format!("{b:02X}"));
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Settings helpers (NULL-aware; self-contained per conventions)
// ---------------------------------------------------------------------------

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
    .filter(|s| !s.trim().is_empty())
}

/// NULL-aware upsert of one global setting (copied from `evals_gcloud` per the
/// conventions; SQLite treats NULL `repo_id` as distinct, so update-then-insert).
async fn upsert_global_setting(state: &AppState, key: &str, value: &str) -> ApiResult<()> {
    let mut tx = state.db.begin().await?;

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

#[cfg(test)]
mod tests {
    use super::*;

    fn keys() -> AuthKeys {
        AuthKeys {
            signing_key: [9u8; 32],
        }
    }

    #[test]
    fn state_cookie_roundtrips_and_binds() {
        let k = keys();
        // PKCE verifiers are 43..=128 chars; use a realistic one.
        let verifier = "a".repeat(50);
        let csrf = "csrf-token-value-123";
        let cookie = make_state_cookie(&k, csrf, &verifier);
        // Cookie payload is the value portion (no name= prefix here).
        let (got_csrf, got_ver) = parse_state_cookie(&k, &cookie).unwrap();
        assert_eq!(got_csrf, csrf);
        assert_eq!(got_ver, verifier);
    }

    #[test]
    fn state_cookie_rejects_tampering() {
        let k = keys();
        let cookie = make_state_cookie(&k, "csrf", &"v".repeat(50));
        // Flip a character in the signature segment.
        let mut parts: Vec<&str> = cookie.split('.').collect();
        let mutated_sig = format!("{}A", &parts[3][..parts[3].len().saturating_sub(1)]);
        parts[3] = &mutated_sig;
        let tampered = parts.join(".");
        assert!(parse_state_cookie(&k, &tampered).is_none());
    }

    #[test]
    fn state_cookie_rejects_wrong_key() {
        let k1 = keys();
        let k2 = AuthKeys {
            signing_key: [3u8; 32],
        };
        let cookie = make_state_cookie(&k1, "csrf", &"v".repeat(50));
        assert!(parse_state_cookie(&k2, &cookie).is_none());
    }

    #[test]
    fn urlencode_escapes_reserved() {
        assert_eq!(urlencode("openid email profile"), "openid%20email%20profile");
        assert_eq!(urlencode("a+b/c=d"), "a%2Bb%2Fc%3Dd");
        assert_eq!(urlencode("safe-_.~"), "safe-_.~");
    }

    #[test]
    fn bool_or_string_parses_both() {
        assert!(BoolOrString::Bool(true).is_true());
        assert!(!BoolOrString::Bool(false).is_true());
        assert!(BoolOrString::Str("true".into()).is_true());
        assert!(BoolOrString::Str("TRUE".into()).is_true());
        assert!(!BoolOrString::Str("false".into()).is_true());
    }

    #[test]
    fn authorize_url_has_required_params() {
        let cfg = OAuthConfig {
            client_id: "cid.apps.googleusercontent.com".into(),
            client_secret: "secret".into(),
            redirect_uri: "https://host/api/auth/google/callback".into(),
        };
        let url = build_authorize_url(&cfg, "csrf123", "challengeABC");
        assert!(url.starts_with(GOOGLE_AUTH_URL));
        assert!(url.contains("client_id=cid.apps.googleusercontent.com"));
        assert!(url.contains("code_challenge=challengeABC"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("state=csrf123"));
        assert!(url.contains("scope=openid%20email%20profile"));
        // The redirect_uri is percent-encoded.
        assert!(url.contains("redirect_uri=https%3A%2F%2Fhost%2Fapi%2Fauth%2Fgoogle%2Fcallback"));
    }
}
