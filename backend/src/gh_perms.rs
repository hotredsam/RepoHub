//! P20 — GitHub permissions analysis (READ-ONLY).
//!
//! Inspects the current `gh` auth session (account + token scopes), flags
//! over-broad scopes, and best-effort reports the viewer's role and default-branch
//! protection for each tracked repo. A second endpoint feeds the same data to the
//! local `claude` CLI for a plain-language summary + least-privilege advice.
//!
//! Nothing here CHANGES any permission: every `gh` call is a read (`auth status`
//! or `gh api ... GET`). All `gh` invocations pass arguments **separately** via
//! `tokio::process::Command` — repo slugs are never shell-interpolated — and the
//! token itself is never logged (only its declared scopes).

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;
use serde_json::{json, Value};
use tokio::process::Command;

use crate::auth_mw::Principal;
use crate::error::{ApiResult, AppError};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/permissions", get(get_permissions))
        .route("/api/permissions/analyze", post(analyze_permissions))
}

/// Reject [`Principal::Codex`] from these routes. `analyze` in particular spawns
/// the `claude` CLI on demand — an abusable compute sink we never expose to a
/// bearer principal.
fn require_owner(principal: &Principal) -> ApiResult<()> {
    match principal {
        Principal::Codex => Err(AppError::Forbidden(
            "permissions analysis is owner-only".into(),
        )),
        Principal::Local | Principal::User(_) => Ok(()),
    }
}

// ---------------------------------------------------------------------------
// Outgoing shapes
// ---------------------------------------------------------------------------

/// One flagged scope that grants more authority than this app needs.
#[derive(Debug, Clone, Serialize)]
pub struct OverBroad {
    pub scope: String,
    pub why: String,
}

/// Per-repo permission summary (best-effort; fields stay null/false on failure).
#[derive(Debug, Clone, Serialize)]
pub struct RepoPerm {
    pub full_name: String,
    pub role: Option<String>,
    pub branch_protected: bool,
}

/// Full read-only permissions report (the body of `GET /api/permissions`).
#[derive(Debug, Clone, Serialize)]
pub struct PermissionsReport {
    pub account: Option<String>,
    pub token_scopes: Vec<String>,
    pub over_broad: Vec<OverBroad>,
    pub repos: Vec<RepoPerm>,
    /// Always null here; populated only by `POST /api/permissions/analyze`.
    pub summary: Option<String>,
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// Reject malformed `owner/name` slugs before handing them to `gh`. `gh` only
/// ever sees these as separate args (never shell), but a strict check yields
/// clearer behavior and avoids surprising `gh` path parsing.
fn valid_repo(full_name: &str) -> bool {
    let parts: Vec<&str> = full_name.split('/').collect();
    parts.len() == 2
        && parts.iter().all(|p| {
            !p.is_empty()
                && p.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        })
}

// ---------------------------------------------------------------------------
// `gh auth status` parsing (account + token scopes)
// ---------------------------------------------------------------------------

/// Parsed `gh auth status` essentials. We never retain or log the token itself.
struct AuthStatus {
    account: Option<String>,
    scopes: Vec<String>,
}

/// Run `gh auth status` and parse the active account login + declared token
/// scopes. Best-effort: returns empty/None fields if `gh` is missing or the
/// output shape changes, rather than failing the whole request.
async fn auth_status() -> AuthStatus {
    let out = Command::new("gh")
        .args(["auth", "status"])
        .output()
        .await;

    let Ok(out) = out else {
        return AuthStatus {
            account: None,
            scopes: Vec::new(),
        };
    };

    // `gh auth status` writes its human-readable report to stderr on some
    // versions and stdout on others — concatenate both before parsing.
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push('\n');
    text.push_str(&String::from_utf8_lossy(&out.stderr));

    parse_auth_status(&text)
}

/// Parse the human-readable `gh auth status` text. Tolerant of leading glyphs
/// (`✓`, `-`) and surrounding whitespace.
fn parse_auth_status(text: &str) -> AuthStatus {
    let mut account: Option<String> = None;
    let mut scopes: Vec<String> = Vec::new();

    for raw in text.lines() {
        let line = raw.trim().trim_start_matches(['✓', '-', '•', '*']).trim();

        // "Logged in to github.com account <login> (keyring)"
        if account.is_none() {
            if let Some(idx) = line.find("account ") {
                let rest = &line[idx + "account ".len()..];
                let login = rest
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .trim();
                if !login.is_empty() {
                    account = Some(login.to_string());
                }
            }
        }

        // "Token scopes: 'gist', 'read:org', 'repo', 'workflow'"
        if scopes.is_empty() {
            if let Some(idx) = line.to_ascii_lowercase().find("token scopes:") {
                let rest = &line[idx + "token scopes:".len()..];
                scopes = rest
                    .split(',')
                    .map(|s| s.trim().trim_matches(['\'', '"', ' ']).to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
            }
        }
    }

    AuthStatus { account, scopes }
}

/// Flag scopes that are broader than this local-first dashboard requires.
/// READ-ONLY advice only — nothing is revoked or changed.
fn flag_over_broad(scopes: &[String]) -> Vec<OverBroad> {
    let mut flags = Vec::new();
    for s in scopes {
        let s_norm = s.trim().to_ascii_lowercase();
        match s_norm.as_str() {
            "delete_repo" => flags.push(OverBroad {
                scope: s.clone(),
                why: "Grants permission to permanently delete repositories. Only needed for the bulk-delete feature; consider removing it unless you actively use bulk delete.".into(),
            }),
            // Full `repo` (not a sub-scope like `repo:status`) is broad: it
            // includes private-repo read/write AND enables remote deletion paths.
            "repo" => flags.push(OverBroad {
                scope: s.clone(),
                why: "Full control of private repositories (read/write code, issues, and admin on repo hooks). This is the widest repo scope and, combined with delete_repo, permits bulk deletion. Prefer the narrowest sub-scopes (e.g. repo:status, public_repo) that cover your workflows.".into(),
            }),
            _ => {}
        }
    }
    flags
}

// ---------------------------------------------------------------------------
// Per-repo permission probing (best-effort `gh api ... GET`)
// ---------------------------------------------------------------------------

/// Best-effort: fetch the viewer's role/permission + default branch for one repo
/// via `gh api repos/<full> --jq`, then check default-branch protection. Any
/// failure (no access, issues disabled, missing branch) degrades to
/// `role=None, branch_protected=false` rather than erroring the whole report.
async fn probe_repo(full_name: &str) -> RepoPerm {
    let mut perm = RepoPerm {
        full_name: full_name.to_string(),
        role: None,
        branch_protected: false,
    };

    if !valid_repo(full_name) {
        return perm;
    }

    // 1) Viewer role/permission + default branch in one call.
    //    `role_name` is the highest role (admin/maintain/write/triage/read) when
    //    GitHub provides it; otherwise we derive a coarse role from the boolean
    //    `permissions` map. The jq emits a compact JSON object on stdout.
    let endpoint = format!("repos/{full_name}");
    let jq = "{role: .role_name, perms: .permissions, default_branch: .default_branch}";
    let default_branch = {
        let out = Command::new("gh")
            .args(["api", &endpoint, "--jq", jq])
            .output()
            .await;

        match out {
            Ok(out) if out.status.success() => {
                if let Ok(v) = serde_json::from_slice::<Value>(&out.stdout) {
                    perm.role = derive_role(&v);
                    v.get("default_branch")
                        .and_then(|b| b.as_str())
                        .map(|s| s.to_string())
                } else {
                    None
                }
            }
            _ => None,
        }
    };

    // 2) Default-branch protection. The protection endpoint 404s ("Branch not
    //    protected") when no rule exists — that is the common, expected case and
    //    simply means `branch_protected = false`.
    if let Some(branch) = default_branch.as_deref().filter(|b| !b.is_empty()) {
        if branch_is_protected(full_name, branch).await {
            perm.branch_protected = true;
        }
    }

    perm
}

/// Derive a coarse role string from the `gh api repos/<full>` JSON. Prefers the
/// explicit `role_name`; otherwise maps the boolean `permissions` object down to
/// the highest implied role.
fn derive_role(v: &Value) -> Option<String> {
    if let Some(role) = v.get("role").and_then(|r| r.as_str()) {
        if !role.is_empty() {
            return Some(role.to_string());
        }
    }
    let perms = v.get("perms")?;
    let has = |k: &str| perms.get(k).and_then(|b| b.as_bool()).unwrap_or(false);
    let role = if has("admin") {
        "admin"
    } else if has("maintain") {
        "maintain"
    } else if has("push") {
        "write"
    } else if has("triage") {
        "triage"
    } else if has("pull") {
        "read"
    } else {
        return None;
    };
    Some(role.to_string())
}

/// Best-effort check of whether the default branch is protected. Returns false
/// on the expected 404 (no protection) or any other failure.
async fn branch_is_protected(full_name: &str, branch: &str) -> bool {
    let endpoint = format!("repos/{full_name}/branches/{branch}/protection");
    match Command::new("gh")
        .args(["api", &endpoint])
        .output()
        .await
    {
        // A successful GET means a protection rule exists.
        Ok(out) => out.status.success(),
        Err(_) => false,
    }
}

/// Resolve the set of tracked repo `full_name`s from the DB.
async fn tracked_full_names(state: &AppState) -> ApiResult<Vec<String>> {
    let rows = sqlx::query_as::<_, (String,)>(
        "SELECT full_name FROM repos WHERE tracked = 1 ORDER BY full_name",
    )
    .fetch_all(&state.db)
    .await?;
    Ok(rows.into_iter().map(|(full_name,)| full_name).collect())
}

/// Gather the full read-only report: account + scopes + over-broad flags + the
/// best-effort per-repo permission rows. `summary` is left None here.
async fn gather_report(state: &AppState) -> ApiResult<PermissionsReport> {
    let auth = auth_status().await;
    let over_broad = flag_over_broad(&auth.scopes);

    let names = tracked_full_names(state).await?;
    let mut repos = Vec::with_capacity(names.len());
    for full_name in &names {
        repos.push(probe_repo(full_name).await);
    }

    Ok(PermissionsReport {
        account: auth.account,
        token_scopes: auth.scopes,
        over_broad,
        repos,
        summary: None,
    })
}

// ---------------------------------------------------------------------------
// GET /api/permissions
// ---------------------------------------------------------------------------

/// READ-ONLY: report the current GitHub auth account, token scopes, over-broad
/// scope flags, and per-repo role/branch-protection for tracked repos.
async fn get_permissions(
    State(state): State<AppState>,
    principal: Principal,
) -> ApiResult<Json<PermissionsReport>> {
    require_owner(&principal)?;
    let report = gather_report(&state).await?;
    Ok(Json(report))
}

// ---------------------------------------------------------------------------
// POST /api/permissions/analyze
// ---------------------------------------------------------------------------

/// Build the prompt fed to the local `claude` CLI from the gathered report.
fn build_prompt(report: &PermissionsReport) -> String {
    // Serialize the raw data so Claude reasons over the exact facts. Pretty-print
    // for readability; this contains no secrets (token value is never included).
    let raw = serde_json::to_string_pretty(report)
        .unwrap_or_else(|_| "{}".to_string());

    format!(
        "You are reviewing the GitHub access used by a local-first repo dashboard \
         called RepoHub. Below is a READ-ONLY snapshot of the current `gh` CLI \
         auth account, its OAuth token scopes, any scopes already flagged as \
         over-broad, and per-repo role / default-branch-protection facts.\n\n\
         Write a short, plain-language summary for a non-expert that:\n\
         1. States in one or two sentences what this token can currently do.\n\
         2. Lists concrete least-privilege recommendations (which scopes to drop \
         or narrow, and why), being explicit that `delete_repo` and full `repo` \
         are the riskiest.\n\
         3. Notes any repos whose default branch is NOT protected, as a gentle \
         suggestion (not a requirement).\n\
         Do NOT propose commands that change anything; this is advisory only. Keep \
         it under ~200 words and use simple bullet points.\n\n\
         DATA (JSON):\n{raw}\n"
    )
}

/// POST: gather the same read-only data, then ask the local `claude` CLI for a
/// plain-language summary + least-privilege recommendations. Degrades clearly
/// (summary = null, plus an `error` note) if `claude` is unavailable; the raw
/// data is always returned regardless.
async fn analyze_permissions(
    State(state): State<AppState>,
    principal: Principal,
) -> ApiResult<Json<Value>> {
    require_owner(&principal)?;
    let report = gather_report(&state).await?;
    let prompt = build_prompt(&report);

    // Run Claude from the data root so it has a stable, writable cwd. This is a
    // pure text task (no repo files needed), so the working dir is incidental.
    let cwd = state.cfg.root.clone();

    match crate::claude_runner::run_collect(&cwd, &prompt).await {
        Ok(outcome) if !outcome.response.trim().is_empty() => Ok(Json(json!({
            "summary": outcome.response.trim(),
            "raw": report,
        }))),
        Ok(_) => Ok(Json(json!({
            "summary": Value::Null,
            "error": "Claude returned an empty response; showing raw permission data only.",
            "raw": report,
        }))),
        Err(e) => Ok(Json(json!({
            "summary": Value::Null,
            "error": format!(
                "Could not run the local `claude` CLI ({e}); showing raw permission data only."
            ),
            "raw": report,
        }))),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_account_and_scopes() {
        let text = "github.com\n  \u{2713} Logged in to github.com account hotredsam (keyring)\n  - Active account: true\n  - Git operations protocol: https\n  - Token: gho_************\n  - Token scopes: 'gist', 'read:org', 'repo', 'workflow'\n";
        let a = parse_auth_status(text);
        assert_eq!(a.account.as_deref(), Some("hotredsam"));
        assert_eq!(
            a.scopes,
            vec![
                "gist".to_string(),
                "read:org".to_string(),
                "repo".to_string(),
                "workflow".to_string()
            ]
        );
    }

    #[test]
    fn empty_status_is_tolerated() {
        let a = parse_auth_status("");
        assert!(a.account.is_none());
        assert!(a.scopes.is_empty());
    }

    #[test]
    fn flags_delete_repo_and_full_repo_only() {
        let scopes = vec![
            "gist".to_string(),
            "repo".to_string(),
            "repo:status".to_string(),
            "delete_repo".to_string(),
            "workflow".to_string(),
        ];
        let flags = flag_over_broad(&scopes);
        let flagged: Vec<&str> = flags.iter().map(|f| f.scope.as_str()).collect();
        assert!(flagged.contains(&"repo"));
        assert!(flagged.contains(&"delete_repo"));
        // Sub-scopes and unrelated scopes are NOT flagged.
        assert!(!flagged.contains(&"repo:status"));
        assert!(!flagged.contains(&"gist"));
        assert!(!flagged.contains(&"workflow"));
        assert_eq!(flags.len(), 2);
    }

    #[test]
    fn derives_role_from_permissions_when_role_name_missing() {
        let v = json!({
            "role": Value::Null,
            "perms": {"admin": true, "maintain": true, "push": true, "pull": true, "triage": true},
            "default_branch": "main"
        });
        assert_eq!(derive_role(&v).as_deref(), Some("admin"));

        let v2 = json!({"role": Value::Null, "perms": {"admin": false, "push": true, "pull": true}});
        assert_eq!(derive_role(&v2).as_deref(), Some("write"));

        let v3 = json!({"role": "triage", "perms": {"admin": true}});
        // Explicit role_name wins over derived.
        assert_eq!(derive_role(&v3).as_deref(), Some("triage"));

        let v4 = json!({"perms": {}});
        assert_eq!(derive_role(&v4), None);
    }

    #[test]
    fn validates_repo_slugs() {
        assert!(valid_repo("owner/name"));
        assert!(valid_repo("hotredsam/Claude-Orchestration-Dashboard"));
        assert!(valid_repo("a.b_c/d-e.f"));
        assert!(!valid_repo("owner"));
        assert!(!valid_repo("owner/name/extra"));
        assert!(!valid_repo("owner/"));
        assert!(!valid_repo("ow ner/name"));
        assert!(!valid_repo("owner/na;me"));
    }
}
