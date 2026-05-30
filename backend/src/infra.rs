//! Infrastructure registry (NAS / SSH hosts) + a connectivity test.

use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::process::Command;

use crate::error::{ApiResult, AppError};
use crate::models::InfraResource;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/infra", get(list_infra).post(create_infra))
        .route("/api/infra/:id", axum::routing::delete(delete_infra))
        .route("/api/infra/:id/test", post(test_infra))
}

async fn list_infra(State(state): State<AppState>) -> ApiResult<Json<Vec<InfraResource>>> {
    let rows = sqlx::query_as::<_, InfraResource>(
        "SELECT * FROM infra_resources ORDER BY created_at DESC",
    )
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

#[derive(Debug, Deserialize)]
pub struct CreateInfraBody {
    pub name: String,
    pub kind: String,
    pub host: String,
    #[serde(default = "default_port")]
    pub port: i64,
    pub username: Option<String>,
    pub base_path: Option<String>,
    pub notes: Option<String>,
}

fn default_port() -> i64 {
    22
}

async fn create_infra(
    State(state): State<AppState>,
    Json(body): Json<CreateInfraBody>,
) -> ApiResult<Json<InfraResource>> {
    if body.name.trim().is_empty() || body.host.trim().is_empty() {
        return Err(AppError::msg("name and host are required"));
    }
    let res = sqlx::query(
        "INSERT INTO infra_resources (name, kind, host, port, username, base_path, notes, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
    )
    .bind(&body.name)
    .bind(&body.kind)
    .bind(&body.host)
    .bind(body.port)
    .bind(&body.username)
    .bind(&body.base_path)
    .bind(&body.notes)
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(&state.db)
    .await?;

    let row = sqlx::query_as::<_, InfraResource>("SELECT * FROM infra_resources WHERE id = ?1")
        .bind(res.last_insert_rowid())
        .fetch_one(&state.db)
        .await?;
    Ok(Json(row))
}

async fn delete_infra(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    sqlx::query("DELETE FROM infra_resources WHERE id = ?1")
        .bind(id)
        .execute(&state.db)
        .await?;
    Ok(Json(json!({ "deleted": id })))
}

#[derive(Debug, Serialize)]
pub struct TestResult {
    pub ok: bool,
    pub error: Option<String>,
}

async fn test_infra(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<TestResult>> {
    let row = sqlx::query_as::<_, InfraResource>("SELECT * FROM infra_resources WHERE id = ?1")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::msg(format!("infra {id} not found")))?;

    let target = match &row.username {
        Some(u) if !u.is_empty() => format!("{}@{}", u, row.host),
        _ => row.host.clone(),
    };
    let port = row.port.to_string();

    let out = Command::new("ssh")
        .args([
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=5",
            &target,
            "-p",
            &port,
            "true",
        ])
        .output()
        .await;

    let result = match out {
        Ok(o) if o.status.success() => TestResult {
            ok: true,
            error: None,
        },
        Ok(o) => TestResult {
            ok: false,
            error: Some(String::from_utf8_lossy(&o.stderr).trim().to_string()),
        },
        Err(e) => TestResult {
            ok: false,
            error: Some(e.to_string()),
        },
    };
    Ok(Json(result))
}
