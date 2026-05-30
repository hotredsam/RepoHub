//! Ticketing API backed by REAL GitHub issues via the `gh` CLI.
//!
//! GitHub Issues are the single source of truth (also visible on github.com).
//! Every `gh` invocation goes through `tokio::process::Command` with arguments
//! passed **separately** — issue titles/bodies/labels are never shell-interpolated,
//! so there is no command-injection surface.
//!
//! The default listing scope is "across the user's tracked repos" (the `repos`
//! table where `tracked = 1`); a single `?repo=owner/name` narrows to one repo.
//! Repos with issues disabled are tolerated: they simply contribute nothing.

use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tokio::process::Command;

use crate::error::{ApiResult, AppError};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/tickets", get(list_tickets).post(create_ticket))
        .route(
            "/api/tickets/:repo_owner/:repo_name/:number/comment",
            post(comment_ticket),
        )
        .route(
            "/api/tickets/:repo_owner/:repo_name/:number/state",
            post(set_ticket_state),
        )
}

// ---------------------------------------------------------------------------
// Raw JSON shapes from `gh issue list --json ...`
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct RawLabel {
    name: String,
}

#[derive(Debug, Deserialize)]
struct RawAssignee {
    login: String,
}

#[derive(Debug, Deserialize)]
struct RawIssue {
    number: i64,
    title: String,
    state: String,
    #[serde(default)]
    labels: Vec<RawLabel>,
    #[serde(default)]
    assignees: Vec<RawAssignee>,
    #[serde(rename = "updatedAt", default)]
    updated_at: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    body: Option<String>,
}

// ---------------------------------------------------------------------------
// Outgoing shape
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct Ticket {
    pub repo: String,
    pub number: i64,
    pub title: String,
    pub state: String,
    pub labels: Vec<String>,
    pub assignees: Vec<String>,
    pub updated_at: String,
    pub url: String,
    pub body: Option<String>,
}

impl Ticket {
    fn from_raw(repo: &str, r: RawIssue) -> Self {
        Ticket {
            repo: repo.to_string(),
            number: r.number,
            title: r.title,
            state: r.state,
            labels: r.labels.into_iter().map(|l| l.name).collect(),
            assignees: r.assignees.into_iter().map(|a| a.login).collect(),
            updated_at: r.updated_at,
            url: r.url,
            body: r.body,
        }
    }
}

// ---------------------------------------------------------------------------
// Validation helpers
// ---------------------------------------------------------------------------

/// Reject obviously malformed `owner/name` slugs before handing them to `gh`.
/// `gh` itself only sees these as `-R` arguments (never shell), but a strict
/// check yields clearer errors and avoids surprising `gh` parsing.
fn validate_repo(full_name: &str) -> ApiResult<()> {
    let parts: Vec<&str> = full_name.split('/').collect();
    let ok = parts.len() == 2
        && parts.iter().all(|p| {
            !p.is_empty()
                && p.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        });
    if ok {
        Ok(())
    } else {
        Err(AppError::msg(format!("invalid repo slug: {full_name}")))
    }
}

// ---------------------------------------------------------------------------
// GET /api/tickets
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    /// Restrict to a single `owner/name` repo. When absent, scope is all
    /// tracked repos.
    pub repo: Option<String>,
    /// `open` | `closed` | `all`. Defaults to `all` so the board shows both.
    pub state: Option<String>,
    /// Filter to issues carrying this label (post-merge, case-insensitive).
    pub label: Option<String>,
    /// Filter to issues assigned to this login (post-merge, case-insensitive).
    pub assignee: Option<String>,
}

/// Read-only: list issues across tracked repos (or a single `?repo=`).
async fn list_tickets(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<Vec<Ticket>>> {
    // Resolve the set of repos to query.
    let repos: Vec<String> = match &q.repo {
        Some(r) if !r.trim().is_empty() => {
            let r = r.trim().to_string();
            validate_repo(&r)?;
            vec![r]
        }
        _ => {
            // Default scope: every tracked repo's full_name.
            let rows = sqlx::query_as::<_, (String,)>(
                "SELECT full_name FROM repos WHERE tracked = 1 ORDER BY full_name",
            )
            .fetch_all(&state.db)
            .await?;
            rows.into_iter().map(|(full_name,)| full_name).collect()
        }
    };

    // `gh issue list` defaults to open-only; pass the requested state, defaulting
    // to `all` so closed tickets are visible too.
    let state_filter = q.state.as_deref().unwrap_or("all");

    let mut all: Vec<Ticket> = Vec::new();
    for repo in &repos {
        // Skip malformed slugs from the DB rather than aborting the whole list.
        if validate_repo(repo).is_err() {
            continue;
        }
        match list_repo_issues(repo, state_filter).await {
            Ok(mut issues) => all.append(&mut issues),
            // Repos with issues disabled (or otherwise unreadable) contribute
            // nothing instead of failing the request.
            Err(_) => continue,
        }
    }

    // Optional in-memory filtering by label / assignee (applied across repos).
    if let Some(label) = q.label.as_deref().filter(|s| !s.trim().is_empty()) {
        let needle = label.trim().to_ascii_lowercase();
        all.retain(|t| t.labels.iter().any(|l| l.to_ascii_lowercase() == needle));
    }
    if let Some(assignee) = q.assignee.as_deref().filter(|s| !s.trim().is_empty()) {
        let needle = assignee.trim().to_ascii_lowercase();
        all.retain(|t| t.assignees.iter().any(|a| a.to_ascii_lowercase() == needle));
    }

    // Newest-updated first across all repos.
    all.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));

    Ok(Json(all))
}

/// `gh issue list/view --json` field set. Kept in one place so the listing and
/// single-issue re-fetch (after a mutation) always deserialize the same shape.
const ISSUE_FIELDS: &str = "number,title,state,labels,assignees,updatedAt,url,body";

/// Re-fetch one issue as a full `Ticket` via `gh issue view`. Used after a
/// create/comment/state mutation so the API returns the same `Ticket` shape the
/// listing does (the frontend optimistically renders the returned ticket).
async fn view_repo_issue(repo: &str, number: i64) -> anyhow::Result<Ticket> {
    let number_s = number.to_string();
    let out = Command::new("gh")
        .args([
            "issue",
            "view",
            &number_s,
            "-R",
            repo,
            "--json",
            ISSUE_FIELDS,
        ])
        .output()
        .await?;

    if !out.status.success() {
        anyhow::bail!(
            "gh issue view {} -R {} failed: {}",
            number,
            repo,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }

    let raw: RawIssue = serde_json::from_slice(&out.stdout)?;
    Ok(Ticket::from_raw(repo, raw))
}

/// Run `gh issue list -R <repo> ...` for a single repo. Returns `Err` (so the
/// caller can skip it) when issues are disabled or `gh` otherwise fails.
async fn list_repo_issues(repo: &str, state_filter: &str) -> anyhow::Result<Vec<Ticket>> {
    let fields = ISSUE_FIELDS;
    let out = Command::new("gh")
        .args([
            "issue",
            "list",
            "-R",
            repo,
            "--state",
            state_filter,
            "--json",
            fields,
            "--limit",
            "100",
        ])
        .output()
        .await?;

    if !out.status.success() {
        anyhow::bail!(
            "gh issue list -R {} failed: {}",
            repo,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }

    let raw: Vec<RawIssue> = serde_json::from_slice(&out.stdout)?;
    Ok(raw
        .into_iter()
        .map(|r| Ticket::from_raw(repo, r))
        .collect())
}

// ---------------------------------------------------------------------------
// POST /api/tickets — create an issue
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct CreateBody {
    pub repo: String,
    pub title: String,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub labels: Vec<String>,
}

async fn create_ticket(
    State(state): State<AppState>,
    Json(body): Json<CreateBody>,
) -> ApiResult<Json<Ticket>> {
    let _ = &state; // state unused beyond router wiring; kept for signature parity
    let repo = body.repo.trim();
    validate_repo(repo)?;
    if body.title.trim().is_empty() {
        return Err(AppError::msg("title is required"));
    }

    // Build args separately — no shell interpolation of user-supplied strings.
    let mut args: Vec<String> = vec![
        "issue".into(),
        "create".into(),
        "-R".into(),
        repo.into(),
        "--title".into(),
        body.title.clone(),
        // Always pass --body so `gh` never drops into an interactive editor.
        "--body".into(),
        body.body.clone().unwrap_or_default(),
    ];
    for label in &body.labels {
        let label = label.trim();
        if !label.is_empty() {
            args.push("--label".into());
            args.push(label.into());
        }
    }

    let out = Command::new("gh")
        .args(&args)
        .output()
        .await
        .map_err(|e| AppError::msg(format!("failed to run `gh issue create`: {e}")))?;

    if !out.status.success() {
        return Err(AppError::msg(format!(
            "gh issue create failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }

    // `gh issue create` prints the new issue URL on stdout.
    let stdout = String::from_utf8_lossy(&out.stdout);
    let url = stdout
        .lines()
        .map(str::trim)
        .find(|l| l.contains("/issues/"))
        .unwrap_or_else(|| stdout.trim())
        .to_string();
    let number = parse_issue_number(&url)
        .ok_or_else(|| AppError::msg(format!("could not parse new issue number from: {url}")))?;

    // Re-fetch the created issue so the response is a full `Ticket` (matching the
    // listing shape the frontend renders), not just a {repo,number,url} stub.
    let ticket = view_repo_issue(repo, number)
        .await
        .map_err(|e| AppError::msg(format!("created issue but failed to load it: {e}")))?;
    Ok(Json(ticket))
}

/// Extract the trailing issue number from a `.../issues/<n>` URL.
fn parse_issue_number(url: &str) -> Option<i64> {
    url.rsplit('/').next().and_then(|s| s.parse::<i64>().ok())
}

// ---------------------------------------------------------------------------
// POST /api/tickets/:repo_owner/:repo_name/:number/comment
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct CommentBody {
    pub body: String,
}

async fn comment_ticket(
    State(state): State<AppState>,
    Path((repo_owner, repo_name, number)): Path<(String, String, i64)>,
    Json(body): Json<CommentBody>,
) -> ApiResult<Json<Ticket>> {
    let _ = &state;
    let repo = format!("{repo_owner}/{repo_name}");
    validate_repo(&repo)?;
    if body.body.trim().is_empty() {
        return Err(AppError::msg("comment body is required"));
    }

    let number_s = number.to_string();
    let out = Command::new("gh")
        .args([
            "issue",
            "comment",
            &number_s,
            "-R",
            &repo,
            "--body",
            &body.body,
        ])
        .output()
        .await
        .map_err(|e| AppError::msg(format!("failed to run `gh issue comment`: {e}")))?;

    if !out.status.success() {
        return Err(AppError::msg(format!(
            "gh issue comment failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }

    // Re-fetch so the response is a full `Ticket` (the comment bumps updatedAt);
    // the frontend optimistically renders the returned ticket.
    let ticket = view_repo_issue(&repo, number)
        .await
        .map_err(|e| AppError::msg(format!("commented but failed to reload issue: {e}")))?;
    Ok(Json(ticket))
}

// ---------------------------------------------------------------------------
// POST /api/tickets/:repo_owner/:repo_name/:number/state
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct StateBody {
    /// `"closed"` or `"open"`.
    pub state: String,
    #[serde(default)]
    pub labels_add: Vec<String>,
    #[serde(default)]
    pub labels_remove: Vec<String>,
}

async fn set_ticket_state(
    State(state): State<AppState>,
    Path((repo_owner, repo_name, number)): Path<(String, String, i64)>,
    Json(body): Json<StateBody>,
) -> ApiResult<Json<Ticket>> {
    let _ = &state;
    let repo = format!("{repo_owner}/{repo_name}");
    validate_repo(&repo)?;
    let number_s = number.to_string();

    // 1) close / reopen
    let verb = match body.state.trim().to_ascii_lowercase().as_str() {
        "closed" | "close" => Some("close"),
        "open" | "reopen" => Some("reopen"),
        "" => None, // allow label-only edits with no state change
        other => {
            return Err(AppError::msg(format!(
                "invalid state '{other}' (expected 'open' or 'closed')"
            )))
        }
    };

    if let Some(verb) = verb {
        let out = Command::new("gh")
            .args(["issue", verb, &number_s, "-R", &repo])
            .output()
            .await
            .map_err(|e| AppError::msg(format!("failed to run `gh issue {verb}`: {e}")))?;
        if !out.status.success() {
            return Err(AppError::msg(format!(
                "gh issue {verb} failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
    }

    // 2) label add/remove via `gh issue edit` (args passed separately).
    let mut edit_args: Vec<String> = vec![
        "issue".into(),
        "edit".into(),
        number_s.clone(),
        "-R".into(),
        repo.clone(),
    ];
    let mut have_edit = false;
    for label in &body.labels_add {
        let label = label.trim();
        if !label.is_empty() {
            edit_args.push("--add-label".into());
            edit_args.push(label.into());
            have_edit = true;
        }
    }
    for label in &body.labels_remove {
        let label = label.trim();
        if !label.is_empty() {
            edit_args.push("--remove-label".into());
            edit_args.push(label.into());
            have_edit = true;
        }
    }

    if have_edit {
        let out = Command::new("gh")
            .args(&edit_args)
            .output()
            .await
            .map_err(|e| AppError::msg(format!("failed to run `gh issue edit`: {e}")))?;
        if !out.status.success() {
            return Err(AppError::msg(format!(
                "gh issue edit failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
    }

    // Re-fetch so the response is a full, current `Ticket` (new state + labels);
    // the frontend optimistically renders the returned ticket.
    let ticket = view_repo_issue(&repo, number)
        .await
        .map_err(|e| AppError::msg(format!("updated issue but failed to reload it: {e}")))?;
    Ok(Json(ticket))
}
