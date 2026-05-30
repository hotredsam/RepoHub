//! GitHub access via the system `gh` CLI (no octocrab).
//!
//! The user is already logged in with `gh`; we reuse that session for both the
//! token and repo listing/deletion.

use anyhow::{anyhow, Context};
use serde::Deserialize;
use tokio::process::Command;

/// Normalized repo info pulled from `gh repo list`.
#[derive(Debug, Clone)]
pub struct GhRepo {
    pub full_name: String,
    pub name: String,
    pub owner: String,
    pub is_private: bool,
    pub language: Option<String>,
    pub description: Option<String>,
    pub default_branch: String,
    /// Disk usage reported by GitHub, in kilobytes.
    pub disk_usage: i64,
    pub updated_at: String,
    pub pushed_at: String,
}

/// Return the current `gh` auth token.
pub async fn token() -> anyhow::Result<String> {
    let out = Command::new("gh")
        .args(["auth", "token"])
        .output()
        .await
        .context("failed to run `gh auth token`")?;
    if !out.status.success() {
        return Err(anyhow!(
            "gh auth token failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

// --- Raw JSON shapes from `gh repo list --json ...` ---

#[derive(Debug, Deserialize)]
struct RawOwner {
    login: String,
}

#[derive(Debug, Deserialize)]
struct RawLanguage {
    name: String,
}

#[derive(Debug, Deserialize)]
struct RawBranchRef {
    name: String,
}

#[derive(Debug, Deserialize)]
struct RawRepo {
    #[serde(rename = "nameWithOwner")]
    name_with_owner: String,
    name: String,
    owner: RawOwner,
    #[serde(rename = "isPrivate")]
    is_private: bool,
    #[serde(rename = "primaryLanguage")]
    primary_language: Option<RawLanguage>,
    description: Option<String>,
    #[serde(rename = "defaultBranchRef")]
    default_branch_ref: Option<RawBranchRef>,
    #[serde(rename = "diskUsage")]
    disk_usage: Option<i64>,
    #[serde(rename = "updatedAt")]
    updated_at: Option<String>,
    #[serde(rename = "pushedAt")]
    pushed_at: Option<String>,
}

/// List the user's repos via `gh repo list`.
pub async fn list_repos() -> anyhow::Result<Vec<GhRepo>> {
    let fields = "nameWithOwner,name,owner,isPrivate,primaryLanguage,description,defaultBranchRef,diskUsage,updatedAt,pushedAt";
    let out = Command::new("gh")
        .args(["repo", "list", "--limit", "300", "--json", fields])
        .output()
        .await
        .context("failed to run `gh repo list`")?;
    if !out.status.success() {
        return Err(anyhow!(
            "gh repo list failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }

    let raw: Vec<RawRepo> =
        serde_json::from_slice(&out.stdout).context("parsing gh repo list JSON")?;

    Ok(raw
        .into_iter()
        .map(|r| GhRepo {
            full_name: r.name_with_owner,
            name: r.name,
            owner: r.owner.login,
            is_private: r.is_private,
            language: r.primary_language.map(|l| l.name),
            description: r.description.filter(|d| !d.is_empty()),
            default_branch: r
                .default_branch_ref
                .map(|b| b.name)
                .unwrap_or_else(|| "main".to_string()),
            disk_usage: r.disk_usage.unwrap_or(0),
            updated_at: r.updated_at.unwrap_or_default(),
            pushed_at: r.pushed_at.unwrap_or_default(),
        })
        .collect())
}

/// Delete a remote repo via `gh repo delete <full_name> --yes`.
pub async fn delete_repo(full_name: &str) -> anyhow::Result<()> {
    let out = Command::new("gh")
        .args(["repo", "delete", full_name, "--yes"])
        .output()
        .await
        .context("failed to run `gh repo delete`")?;
    if !out.status.success() {
        return Err(anyhow!(
            "gh repo delete {} failed: {}",
            full_name,
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(())
}

/// Build the HTTPS clone URL for a `owner/name` repo.
pub fn clone_url(full_name: &str) -> String {
    format!("https://github.com/{full_name}.git")
}
