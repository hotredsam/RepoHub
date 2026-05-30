//! Transcript indexer: walk `~/.claude/projects/**/*.jsonl`, extract user
//! prompts + assistant responses, map each session's cwd to a tracked repo, and
//! upsert `Prompt` rows with `source = "transcript"`.

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::error::ApiResult;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/transcripts/reindex", post(reindex))
        .route("/api/transcripts/status", get(status))
}

#[derive(Debug, Serialize)]
pub struct ReindexResult {
    pub indexed: i64,
}

#[derive(Debug, Serialize)]
pub struct TranscriptStatus {
    pub indexed: i64,
    pub last_reindex: Option<String>,
}

fn projects_dir() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(|h| {
        PathBuf::from(h).join(".claude").join("projects")
    })
}

/// Collect all *.jsonl files under a directory tree (best-effort).
fn collect_jsonl(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_jsonl(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            out.push(path);
        }
    }
}

/// Pull plain text out of a message-content value (string or block array).
fn content_text(content: &Value) -> String {
    if let Some(s) = content.as_str() {
        return s.to_string();
    }
    if let Some(arr) = content.as_array() {
        let mut buf = String::new();
        for block in arr {
            if let Some(t) = block.get("text").and_then(|t| t.as_str()) {
                buf.push_str(t);
            } else if let Some(s) = block.as_str() {
                buf.push_str(s);
            }
        }
        return buf;
    }
    String::new()
}

async fn reindex(State(state): State<AppState>) -> ApiResult<Json<ReindexResult>> {
    // Map normalized local_path -> repo_id for cwd attribution.
    let repos = sqlx::query_as::<_, (i64, Option<String>)>(
        "SELECT id, local_path FROM repos WHERE local_path IS NOT NULL",
    )
    .fetch_all(&state.db)
    .await?;
    let mut path_to_repo: HashMap<String, i64> = HashMap::new();
    for (id, lp) in repos {
        if let Some(lp) = lp {
            path_to_repo.insert(lp, id);
        }
    }

    let Some(dir) = projects_dir() else {
        return Ok(Json(ReindexResult { indexed: 0 }));
    };

    let mut files = Vec::new();
    collect_jsonl(&dir, &mut files);

    let mut indexed = 0i64;

    for file in files {
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };

        // Determine the session cwd by scanning lines for a `cwd` field.
        let mut session_cwd: Option<String> = None;
        // Pending user prompt awaiting the next assistant response.
        let mut pending_user: Option<(String, String)> = None; // (prompt, created_at)

        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let Ok(v) = serde_json::from_str::<Value>(line) else {
                continue;
            };

            if session_cwd.is_none() {
                if let Some(c) = v.get("cwd").and_then(|c| c.as_str()) {
                    session_cwd = Some(c.to_string());
                }
            }

            let role = v
                .get("type")
                .and_then(|t| t.as_str())
                .or_else(|| v.pointer("/message/role").and_then(|r| r.as_str()));

            let ts = v
                .get("timestamp")
                .and_then(|t| t.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| chrono::Utc::now().to_rfc3339());

            match role {
                Some("user") => {
                    let content = v
                        .pointer("/message/content")
                        .or_else(|| v.get("content"))
                        .cloned()
                        .unwrap_or(Value::Null);
                    let txt = content_text(&content);
                    if !txt.trim().is_empty() {
                        pending_user = Some((txt, ts));
                    }
                }
                Some("assistant") => {
                    let content = v
                        .pointer("/message/content")
                        .or_else(|| v.get("content"))
                        .cloned()
                        .unwrap_or(Value::Null);
                    let resp = content_text(&content);
                    if let Some((prompt, created_at)) = pending_user.take() {
                        let repo_id = session_cwd
                            .as_ref()
                            .and_then(|c| path_to_repo.get(c).copied());
                        let scope = if repo_id.is_some() { "repo" } else { "app" };

                        // Dedup: skip if an identical transcript row already exists.
                        let exists: Option<(i64,)> = sqlx::query_as::<_, (i64,)>(
                            "SELECT id FROM prompts WHERE source = 'transcript' \
                             AND prompt = ?1 AND created_at = ?2 LIMIT 1",
                        )
                        .bind(&prompt)
                        .bind(&created_at)
                        .fetch_optional(&state.db)
                        .await?;

                        if exists.is_none() {
                            sqlx::query(
                                "INSERT INTO prompts (repo_id, scope, source, model, prompt, \
                                    response, tokens_in, tokens_out, created_at) \
                                 VALUES (?1, ?2, 'transcript', NULL, ?3, ?4, 0, 0, ?5)",
                            )
                            .bind(repo_id)
                            .bind(scope)
                            .bind(&prompt)
                            .bind(&resp)
                            .bind(&created_at)
                            .execute(&state.db)
                            .await?;
                            indexed += 1;
                        }
                    }
                }
                _ => {}
            }
        }
    }

    Ok(Json(ReindexResult { indexed }))
}

async fn status(State(state): State<AppState>) -> ApiResult<Json<TranscriptStatus>> {
    let (count, last): (i64, Option<String>) = sqlx::query_as::<_, (i64, Option<String>)>(
        "SELECT COUNT(*), MAX(created_at) FROM prompts WHERE source = 'transcript'",
    )
    .fetch_one(&state.db)
    .await?;
    Ok(Json(TranscriptStatus {
        indexed: count,
        last_reindex: last,
    }))
}
