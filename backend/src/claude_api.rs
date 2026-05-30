//! Claude chat over WebSocket (`/ws/claude`) plus a small convenience HTTP
//! endpoint for recent prompts.

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use std::path::PathBuf;

use crate::claude_runner;
use crate::error::ApiResult;
use crate::models::Prompt;
use crate::state::AppState;
use crate::ws_origin::check_origin;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/ws/claude", get(ws_claude))
        .route("/api/prompts/recent", get(recent_prompts))
}

/// First client frame on `/ws/claude`.
#[derive(Debug, Deserialize)]
struct ClaudeRequest {
    repo_id: Option<i64>,
    prompt: String,
}

async fn ws_claude(
    ws: WebSocketUpgrade,
    headers: HeaderMap,
    State(state): State<AppState>,
) -> impl IntoResponse {
    // Reject cross-site WebSocket hijacking before upgrading.
    if let Err(status) = check_origin(&headers, &state.cfg) {
        return status.into_response();
    }
    ws.on_upgrade(move |socket| handle_socket(socket, state))
        .into_response()
}

async fn handle_socket(mut socket: WebSocket, state: AppState) {
    // Wait for the first JSON frame describing the request.
    let req: ClaudeRequest = loop {
        match socket.recv().await {
            Some(Ok(Message::Text(t))) => match serde_json::from_str::<ClaudeRequest>(&t) {
                Ok(r) => break r,
                Err(e) => {
                    let _ = socket
                        .send(Message::Text(format!("error: invalid request: {e}")))
                        .await;
                    return;
                }
            },
            Some(Ok(Message::Close(_))) | None => return,
            Some(Ok(_)) => continue,
            Some(Err(_)) => return,
        }
    };

    // Resolve the working directory: repo local_path if given, else data root.
    let cwd = match req.repo_id {
        Some(id) => {
            match sqlx::query_as::<_, (Option<String>,)>(
                "SELECT local_path FROM repos WHERE id = ?1",
            )
            .bind(id)
            .fetch_optional(&state.db)
            .await
            {
                Ok(Some((Some(p),))) => PathBuf::from(p),
                _ => state.cfg.root.clone(),
            }
        }
        None => state.cfg.root.clone(),
    };

    let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(64);

    // Spawn the Claude run; forward chunks to the websocket as they arrive.
    let prompt = req.prompt.clone();
    let cwd_run = cwd.clone();
    let run = tokio::spawn(async move {
        claude_runner::run_stream(&cwd_run, &prompt, tx).await
    });

    while let Some(chunk) = rx.recv().await {
        if socket.send(Message::Text(chunk)).await.is_err() {
            break;
        }
    }

    let outcome = run.await;

    match outcome {
        Ok(Ok(out)) => {
            // Persist the prompt + response.
            let scope = if req.repo_id.is_some() { "repo" } else { "app" };
            let _ = sqlx::query(
                "INSERT INTO prompts (repo_id, scope, source, model, prompt, response, \
                    tokens_in, tokens_out, created_at) \
                 VALUES (?1, ?2, 'chat', NULL, ?3, ?4, ?5, ?6, ?7)",
            )
            .bind(req.repo_id)
            .bind(scope)
            .bind(&req.prompt)
            .bind(&out.response)
            .bind(out.tokens_in)
            .bind(out.tokens_out)
            .bind(chrono::Utc::now().to_rfc3339())
            .execute(&state.db)
            .await;
            let _ = socket.send(Message::Text("[[done]]".to_string())).await;
        }
        Ok(Err(e)) => {
            let _ = socket.send(Message::Text(format!("error: {e}"))).await;
        }
        Err(e) => {
            let _ = socket
                .send(Message::Text(format!("error: task panicked: {e}")))
                .await;
        }
    }
    let _ = socket.send(Message::Close(None)).await;
}

#[derive(Debug, Deserialize)]
pub struct RecentQuery {
    pub repo_id: Option<i64>,
}

async fn recent_prompts(
    State(state): State<AppState>,
    Query(q): Query<RecentQuery>,
) -> ApiResult<Json<Vec<Prompt>>> {
    let rows = match q.repo_id {
        Some(id) => {
            sqlx::query_as::<_, Prompt>(
                "SELECT * FROM prompts WHERE repo_id = ?1 ORDER BY created_at DESC LIMIT 50",
            )
            .bind(id)
            .fetch_all(&state.db)
            .await?
        }
        None => {
            sqlx::query_as::<_, Prompt>(
                "SELECT * FROM prompts ORDER BY created_at DESC LIMIT 50",
            )
            .fetch_all(&state.db)
            .await?
        }
    };
    Ok(Json(rows))
}
