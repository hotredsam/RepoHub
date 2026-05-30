//! Git operations via the system `git` CLI (no git2).
//!
//! Safety: this module never pulls or merges onto `main` implicitly. `pull` is
//! only invoked by explicit user actions; the scheduler only `fetch`es.

use anyhow::{anyhow, Context};
use std::path::Path;
use tokio::process::Command;

/// Branch divergence + working-tree cleanliness for a repo.
#[derive(Debug, Clone, Copy, Default)]
pub struct GitStatus {
    pub ahead: i64,
    pub behind: i64,
    pub dirty: bool,
}

/// Run `git <args>` in `cwd`, returning stdout on success.
async fn git(cwd: &Path, args: &[&str]) -> anyhow::Result<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .await
        .with_context(|| format!("failed to spawn git {:?}", args))?;
    if !out.status.success() {
        return Err(anyhow!(
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Clone `url` into `dest`.
pub async fn clone(url: &str, dest: &Path) -> anyhow::Result<()> {
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await.ok();
    }
    let dest_s = dest.to_string_lossy().to_string();
    let out = Command::new("git")
        .args(["clone", url, &dest_s])
        .output()
        .await
        .context("failed to spawn git clone")?;
    if !out.status.success() {
        return Err(anyhow!(
            "git clone failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(())
}

/// `git fetch --all --prune`.
pub async fn fetch(path: &Path) -> anyhow::Result<()> {
    git(path, &["fetch", "--all", "--prune"]).await?;
    Ok(())
}

/// `git pull --ff-only` (explicit user action only).
pub async fn pull(path: &Path) -> anyhow::Result<()> {
    git(path, &["pull", "--ff-only"]).await?;
    Ok(())
}

/// Compute ahead/behind vs upstream and whether the tree is dirty.
pub async fn status(path: &Path) -> anyhow::Result<GitStatus> {
    // ahead/behind vs upstream; falls back to 0/0 when there is no upstream.
    let (ahead, behind) = match git(
        path,
        &["rev-list", "--left-right", "--count", "@{u}...HEAD"],
    )
    .await
    {
        Ok(s) => {
            let mut parts = s.split_whitespace();
            let behind = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
            let ahead = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
            (ahead, behind)
        }
        Err(_) => (0, 0),
    };

    let porcelain = git(path, &["status", "--porcelain"]).await.unwrap_or_default();
    let dirty = !porcelain.trim().is_empty();

    Ok(GitStatus {
        ahead,
        behind,
        dirty,
    })
}

/// Ensure a staging branch exists and is checked out (`git checkout -B name`).
pub async fn ensure_staging_branch(path: &Path, name: &str) -> anyhow::Result<()> {
    git(path, &["checkout", "-B", name]).await?;
    Ok(())
}

/// ISO8601 timestamp of the last commit, or `None` if there are no commits.
pub async fn last_commit_iso(path: &Path) -> anyhow::Result<Option<String>> {
    match git(path, &["log", "-1", "--format=%cI"]).await {
        Ok(s) => {
            let s = s.trim().to_string();
            Ok(if s.is_empty() { None } else { Some(s) })
        }
        Err(_) => Ok(None),
    }
}

/// Stage everything and commit on the current branch. Returns whether a commit
/// was actually created (no-op when there was nothing to commit).
pub async fn commit_all(path: &Path, message: &str) -> anyhow::Result<bool> {
    git(path, &["add", "-A"]).await?;
    // `git commit` exits non-zero when there is nothing to commit; treat that as
    // a successful no-op rather than an error.
    let out = Command::new("git")
        .args(["commit", "-m", message])
        .current_dir(path)
        .output()
        .await
        .context("failed to spawn git commit")?;
    Ok(out.status.success())
}
