//! Tailscale serve control (P18).
//!
//! Exposes the loopback RepoHub server to the user's tailnet over HTTPS via
//! `tailscale serve`. Tailscale terminates TLS with a tailnet cert and reverse
//! proxies to `http://127.0.0.1:<port>`, so the app keeps binding loopback only
//! while becoming reachable (authenticated, tailnet-private) from other devices.
//!
//! HARD RULE: this module NEVER invokes `tailscale funnel`. Funnel publishes a
//! service to the PUBLIC internet, which would defeat the local-first / private
//! security posture. We only ever drive `tailscale serve` (tailnet-private). If
//! a funnel is detected in the status JSON we surface it as a WARNING via
//! `funnel_enabled` so the UI can prompt the user to turn it off — we do not act
//! on it.
//!
//! GRACEFUL DEGRADATION: when the Tailscale CLI is absent (not on PATH and not
//! at the macOS app bundle path) every probe reports `installed: false` and the
//! status call still succeeds. `status()` NEVER errors.

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::process::Command;

use crate::auth_mw::Principal;
use crate::error::{ApiResult, AppError};
use crate::state::AppState;

/// macOS Tailscale.app bundles the CLI here even when it is not symlinked onto
/// `PATH`. We fall back to this absolute path before declaring it uninstalled.
const MACOS_APP_CLI: &str = "/Applications/Tailscale.app/Contents/MacOS/Tailscale";

/// Reject [`Principal::Codex`] from serve enable/disable: a bearer must never be
/// able to toggle the host's tailnet HTTPS exposure.
fn require_owner(principal: &Principal) -> ApiResult<()> {
    match principal {
        Principal::Codex => Err(AppError::Forbidden(
            "tailscale serve control is owner-only".into(),
        )),
        Principal::Local | Principal::User(_) => Ok(()),
    }
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/tailscale", get(get_status))
        .route("/api/tailscale/serve/enable", post(enable))
        .route("/api/tailscale/serve/disable", post(disable))
}

/// Best-effort snapshot of the local Tailscale state, surfaced to the UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TsStatus {
    /// Whether the Tailscale CLI is discoverable (PATH or macOS app bundle).
    pub installed: bool,
    /// Whether this node is logged in to a tailnet (BackendState == "Running").
    pub logged_in: bool,
    /// Whether a `tailscale serve` mapping is currently active.
    pub serve_enabled: bool,
    /// WARNING: whether a public Funnel is active. We never enable this; if true
    /// the UI should prompt the user to disable it (the service is PUBLIC).
    pub funnel_enabled: bool,
    /// The published HTTPS URL (e.g. `https://samuels-mac-mini.tail97ef37.ts.net`).
    pub published_url: Option<String>,
    /// This node's MagicDNS name (`Self.DNSName`, trailing dot trimmed).
    pub dns_name: Option<String>,
    /// The tailnet (MagicDNSSuffix), e.g. `tail97ef37.ts.net`.
    pub tailnet: Option<String>,
}

impl TsStatus {
    /// All-false / unknown status used when the CLI is missing.
    fn not_installed() -> Self {
        TsStatus {
            installed: false,
            logged_in: false,
            serve_enabled: false,
            funnel_enabled: false,
            published_url: None,
            dns_name: None,
            tailnet: None,
        }
    }
}

/// Resolve the Tailscale CLI: prefer `PATH`, fall back to the macOS app bundle.
/// Returns `None` when no usable binary is found.
async fn tailscale_bin() -> Option<String> {
    // `which tailscale` on PATH.
    let on_path = Command::new("which")
        .arg("tailscale")
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false);
    if on_path {
        return Some("tailscale".to_string());
    }
    // macOS app bundle path.
    if tokio::fs::metadata(MACOS_APP_CLI).await.is_ok() {
        return Some(MACOS_APP_CLI.to_string());
    }
    None
}

/// Run the Tailscale CLI with separate args (never shell-interpolated) and
/// return stdout as a string. `Err` only on spawn failure or non-zero exit.
async fn run_ts(bin: &str, args: &[&str]) -> ApiResult<String> {
    let out = Command::new(bin)
        .args(args)
        .output()
        .await
        .map_err(|e| AppError::msg(format!("failed to run tailscale: {e}")))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(AppError::msg(format!(
            "tailscale {} failed: {}",
            args.first().copied().unwrap_or(""),
            stderr.trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Run a CLI command and parse its stdout as JSON, tolerating any failure by
/// returning `None`. Used by `status()` so probing never errors.
async fn run_ts_json(bin: &str, args: &[&str]) -> Option<serde_json::Value> {
    let out = Command::new(bin).args(args).output().await.ok()?;
    if !out.status.success() {
        return None;
    }
    serde_json::from_slice(&out.stdout).ok()
}

/// Probe the local Tailscale state via `tailscale serve status --json` and
/// `tailscale status --json`. NEVER errors — returns a best-effort snapshot,
/// reporting `installed: false` when the CLI is absent.
///
/// The `_db` handle is unused (status is derived entirely from the CLI) but kept
/// in the signature to match the sibling status probes (e.g. `gcloud::status`).
pub async fn status(_db: &sqlx::SqlitePool) -> TsStatus {
    probe().await
}

/// Shared probe used by `status` and by the enable/disable handlers so the UI
/// gets a fresh snapshot immediately. NEVER errors.
async fn probe() -> TsStatus {
    let Some(bin) = tailscale_bin().await else {
        return TsStatus::not_installed();
    };

    let mut st = TsStatus::not_installed();
    st.installed = true;

    // `tailscale status --json` → login state + this node's DNS name / tailnet.
    if let Some(v) = run_ts_json(&bin, &["status", "--json"]).await {
        // BackendState == "Running" means logged in and connected.
        st.logged_in = v
            .get("BackendState")
            .and_then(|b| b.as_str())
            .map(|s| s.eq_ignore_ascii_case("Running"))
            .unwrap_or(false);

        // Self.DNSName is the MagicDNS name with a trailing dot, e.g.
        // "samuels-mac-mini.tail97ef37.ts.net."
        if let Some(dns) = v
            .get("Self")
            .and_then(|s| s.get("DNSName"))
            .and_then(|d| d.as_str())
        {
            let trimmed = dns.trim_end_matches('.');
            if !trimmed.is_empty() {
                st.dns_name = Some(trimmed.to_string());
            }
        }

        // MagicDNSSuffix is the tailnet domain, e.g. "tail97ef37.ts.net".
        if let Some(suffix) = v
            .get("MagicDNSSuffix")
            .and_then(|m| m.as_str())
            .filter(|s| !s.is_empty())
        {
            st.tailnet = Some(suffix.to_string());
        }
    }

    // `tailscale serve status --json` → serve/funnel mappings.
    if let Some(v) = run_ts_json(&bin, &["serve", "status", "--json"]).await {
        st.serve_enabled = serve_active(&v);
        // WARNING surface only — we never invoke funnel ourselves.
        st.funnel_enabled = funnel_active(&v);
    }

    // Derive the published HTTPS URL from the node's DNS name when serve is on.
    if st.serve_enabled {
        if let Some(dns) = &st.dns_name {
            st.published_url = Some(format!("https://{dns}"));
        }
    }

    st
}

/// Whether the serve-status JSON describes an active HTTPS serve mapping.
///
/// The `serve status --json` shape is a `ServeConfig`: TLS web handlers live
/// under `TCP` (ports) and `Web` (host:port → handlers). A non-empty `Web` (or
/// `TCP`) map means serve is active. We treat any populated handler set as
/// "serve enabled" without over-fitting to one schema version.
fn serve_active(v: &serde_json::Value) -> bool {
    let non_empty_obj = |key: &str| {
        v.get(key)
            .and_then(|m| m.as_object())
            .map(|m| !m.is_empty())
            .unwrap_or(false)
    };
    non_empty_obj("Web") || non_empty_obj("TCP")
}

/// Whether the serve-status JSON reports ANY active Funnel. The `ServeConfig`
/// carries `AllowFunnel: { "host:port": true }`; any `true` value means a public
/// Funnel is live. This is a WARNING signal only — we never enable Funnel.
fn funnel_active(v: &serde_json::Value) -> bool {
    v.get("AllowFunnel")
        .and_then(|m| m.as_object())
        .map(|m| m.values().any(|val| val.as_bool().unwrap_or(false)))
        .unwrap_or(false)
}

/// Corroborate a tailnet identity asserted via the `Tailscale-User-Login` header
/// against the Tailscale LocalAPI. Returns the verified login email for the given
/// client IP, or `None` if the CLI is unavailable, the address is not a current
/// tailnet peer, or the output cannot be parsed.
///
/// SECURITY: the identity header alone is client-controllable (the app binds
/// loopback and serve proxies from loopback, so any local process can forge it).
/// The gate must only honor the header when `whois(client_ip)` returns the SAME
/// login — proving the connection genuinely originated from that tailnet user.
pub async fn whois_login(client_ip: &str) -> Option<String> {
    let client_ip = client_ip.trim();
    if client_ip.is_empty() {
        return None;
    }
    // Reject anything that is not a bare IP literal before handing it to the CLI
    // (args are passed separately, but a strict check avoids surprising parsing).
    if client_ip.parse::<std::net::IpAddr>().is_err() {
        return None;
    }

    let bin = tailscale_bin().await?;
    let v = run_ts_json(&bin, &["whois", "--json", client_ip]).await?;

    // `tailscale whois --json` returns a WhoIsResponse: { UserProfile: { LoginName, .. }, .. }.
    let login = v
        .get("UserProfile")
        .and_then(|p| p.get("LoginName"))
        .and_then(|l| l.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())?;
    Some(login)
}

async fn get_status(State(state): State<AppState>) -> ApiResult<Json<TsStatus>> {
    Ok(Json(status(&state.db).await))
}

/// Enable a tailnet-private HTTPS serve mapping that reverse-proxies the
/// loopback RepoHub server. Runs:
///   `tailscale serve --bg https / http://127.0.0.1:<port>`
///
/// NOTE: this is `serve`, never `funnel` — the service stays private to the
/// tailnet (authenticated devices only).
pub async fn enable_serve(port: u16) -> ApiResult<TsStatus> {
    let bin = tailscale_bin()
        .await
        .ok_or_else(|| AppError::msg("Tailscale CLI not found (install Tailscale)"))?;

    let target = format!("http://127.0.0.1:{port}");
    // Args passed SEPARATELY — never shell-interpolated.
    run_ts(&bin, &["serve", "--bg", "https", "/", &target]).await?;

    // Return a fresh snapshot so the UI immediately reflects the new mapping.
    Ok(probe().await)
}

/// Disable all serve mappings for this node: `tailscale serve reset`.
pub async fn disable_serve() -> ApiResult<TsStatus> {
    let bin = tailscale_bin()
        .await
        .ok_or_else(|| AppError::msg("Tailscale CLI not found (install Tailscale)"))?;

    run_ts(&bin, &["serve", "reset"]).await?;
    Ok(probe().await)
}

#[derive(Debug, Serialize)]
struct EnableResult {
    status: TsStatus,
    /// Surfaced to the UI when a public Funnel was detected (we never enable it).
    funnel_warning: Option<String>,
}

async fn enable(
    State(state): State<AppState>,
    principal: Principal,
) -> ApiResult<Json<EnableResult>> {
    require_owner(&principal)?;
    let st = enable_serve(state.cfg.port).await?;
    let funnel_warning = if st.funnel_enabled {
        Some(
            "A public Tailscale Funnel is active for this node. RepoHub never enables \
             Funnel; the service is exposed to the PUBLIC internet — disable it with \
             `tailscale funnel reset`."
                .to_string(),
        )
    } else {
        None
    };
    Ok(Json(EnableResult {
        status: st,
        funnel_warning,
    }))
}

async fn disable(
    State(_state): State<AppState>,
    principal: Principal,
) -> ApiResult<Json<serde_json::Value>> {
    require_owner(&principal)?;
    let st = disable_serve().await?;
    Ok(Json(json!({ "status": st })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_installed_is_all_false() {
        let st = TsStatus::not_installed();
        assert!(!st.installed);
        assert!(!st.logged_in);
        assert!(!st.serve_enabled);
        assert!(!st.funnel_enabled);
        assert!(st.published_url.is_none());
        assert!(st.dns_name.is_none());
        assert!(st.tailnet.is_none());
    }

    #[test]
    fn serve_active_detects_web_handlers() {
        let v = json!({
            "TCP": { "443": { "HTTPS": true } },
            "Web": {
                "samuels-mac-mini.tail97ef37.ts.net:443": {
                    "Handlers": { "/": { "Proxy": "http://127.0.0.1:8787" } }
                }
            }
        });
        assert!(serve_active(&v));
    }

    #[test]
    fn serve_inactive_on_empty_config() {
        assert!(!serve_active(&json!({})));
        assert!(!serve_active(&json!({ "Web": {}, "TCP": {} })));
    }

    #[test]
    fn funnel_active_detects_allow_funnel_true() {
        let v = json!({ "AllowFunnel": { "samuels-mac-mini.tail97ef37.ts.net:443": true } });
        assert!(funnel_active(&v));
    }

    #[test]
    fn funnel_inactive_when_absent_or_false() {
        assert!(!funnel_active(&json!({})));
        assert!(!funnel_active(
            &json!({ "AllowFunnel": { "host:443": false } })
        ));
    }
}
