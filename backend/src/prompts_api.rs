//! Prompt history search + lightweight analytics.

use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::error::ApiResult;
use crate::models::Prompt;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/prompts", get(search_prompts))
        .route("/api/prompts/analytics", get(analytics))
}

#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    pub search: Option<String>,
    pub repo_id: Option<i64>,
    pub limit: Option<i64>,
}

async fn search_prompts(
    State(state): State<AppState>,
    Query(q): Query<SearchQuery>,
) -> ApiResult<Json<Vec<Prompt>>> {
    let limit = q.limit.unwrap_or(100).clamp(1, 1000);
    let like = q
        .search
        .as_ref()
        .filter(|s| !s.trim().is_empty())
        .map(|s| format!("%{}%", s.trim()));

    let rows = match (like, q.repo_id) {
        (Some(pat), Some(id)) => {
            sqlx::query_as::<_, Prompt>(
                "SELECT * FROM prompts WHERE repo_id = ?1 \
                 AND (prompt LIKE ?2 OR IFNULL(response,'') LIKE ?2) \
                 ORDER BY created_at DESC LIMIT ?3",
            )
            .bind(id)
            .bind(pat)
            .bind(limit)
            .fetch_all(&state.db)
            .await?
        }
        (Some(pat), None) => {
            sqlx::query_as::<_, Prompt>(
                "SELECT * FROM prompts \
                 WHERE prompt LIKE ?1 OR IFNULL(response,'') LIKE ?1 \
                 ORDER BY created_at DESC LIMIT ?2",
            )
            .bind(pat)
            .bind(limit)
            .fetch_all(&state.db)
            .await?
        }
        (None, Some(id)) => {
            sqlx::query_as::<_, Prompt>(
                "SELECT * FROM prompts WHERE repo_id = ?1 ORDER BY created_at DESC LIMIT ?2",
            )
            .bind(id)
            .bind(limit)
            .fetch_all(&state.db)
            .await?
        }
        (None, None) => {
            sqlx::query_as::<_, Prompt>(
                "SELECT * FROM prompts ORDER BY created_at DESC LIMIT ?1",
            )
            .bind(limit)
            .fetch_all(&state.db)
            .await?
        }
    };
    Ok(Json(rows))
}

#[derive(Debug, Serialize)]
pub struct PerRepo {
    pub repo_id: Option<i64>,
    pub count: i64,
}

#[derive(Debug, Serialize)]
pub struct PerDay {
    pub day: String,
    pub count: i64,
}

#[derive(Debug, Serialize)]
pub struct Analytics {
    pub total_tokens_in: i64,
    pub total_tokens_out: i64,
    pub per_repo: Vec<PerRepo>,
    pub per_day: Vec<PerDay>,
}

async fn analytics(State(state): State<AppState>) -> ApiResult<Json<Analytics>> {
    let (tin, tout): (i64, i64) = sqlx::query_as::<_, (Option<i64>, Option<i64>)>(
        "SELECT SUM(tokens_in), SUM(tokens_out) FROM prompts",
    )
    .fetch_one(&state.db)
    .await
    .map(|(a, b)| (a.unwrap_or(0), b.unwrap_or(0)))?;

    let per_repo = sqlx::query_as::<_, (Option<i64>, i64)>(
        "SELECT repo_id, COUNT(*) FROM prompts GROUP BY repo_id ORDER BY COUNT(*) DESC",
    )
    .fetch_all(&state.db)
    .await?
    .into_iter()
    .map(|(repo_id, count)| PerRepo { repo_id, count })
    .collect();

    let per_day = sqlx::query_as::<_, (String, i64)>(
        "SELECT substr(created_at,1,10) AS day, COUNT(*) FROM prompts \
         GROUP BY day ORDER BY day DESC LIMIT 30",
    )
    .fetch_all(&state.db)
    .await?
    .into_iter()
    .map(|(day, count)| PerDay { day, count })
    .collect();

    Ok(Json(Analytics {
        total_tokens_in: tin,
        total_tokens_out: tout,
        per_repo,
        per_day,
    }))
}
