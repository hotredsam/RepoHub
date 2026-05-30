//! Background scheduler: every `fetch_interval_secs`, fetch (never pull) each
//! tracked repo, recompute status, persist, and broadcast.

use std::path::PathBuf;
use std::time::Duration;

use crate::models::Repo;
use crate::state::AppState;
use crate::gitops;

/// Spawn the background fetch/status loop.
pub fn spawn(state: AppState) {
    tokio::spawn(async move {
        let interval = Duration::from_secs(state.cfg.fetch_interval_secs.max(10));
        loop {
            tokio::time::sleep(interval).await;
            if let Err(e) = tick(&state).await {
                tracing::warn!(error = %e, "scheduler tick failed");
            }
        }
    });
}

async fn tick(state: &AppState) -> anyhow::Result<()> {
    let repos = sqlx::query_as::<_, Repo>(
        "SELECT * FROM repos WHERE tracked = 1 AND local_path IS NOT NULL",
    )
    .fetch_all(&state.db)
    .await?;

    for repo in repos {
        let Some(lp) = repo.local_path.as_ref() else {
            continue;
        };
        let path = PathBuf::from(lp);
        if !path.join(".git").exists() {
            continue;
        }

        // FETCH ONLY — never pull.
        let _ = gitops::fetch(&path).await;
        let st = gitops::status(&path).await.unwrap_or_default();
        let last_commit = gitops::last_commit_iso(&path).await.unwrap_or(None);
        let now = chrono::Utc::now().to_rfc3339();

        sqlx::query(
            "UPDATE repos SET ahead = ?1, behind = ?2, dirty = ?3, last_fetch = ?4, \
             last_commit_at = ?5, updated_at = ?6 WHERE id = ?7",
        )
        .bind(st.ahead)
        .bind(st.behind)
        .bind(st.dirty)
        .bind(&now)
        .bind(last_commit)
        .bind(&now)
        .bind(repo.id)
        .execute(&state.db)
        .await?;

        let _ = state.status_tx.send(
            serde_json::json!({
                "event": "repo_status",
                "id": repo.id,
                "ahead": st.ahead,
                "behind": st.behind,
                "dirty": st.dirty,
            })
            .to_string(),
        );
    }
    Ok(())
}
