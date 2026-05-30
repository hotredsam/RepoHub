//! Data models shared across feature modules.
//!
//! Each struct derives serde Serialize/Deserialize (for the JSON API) and
//! `sqlx::FromRow` (so it can be read directly from a SQLite query). All
//! timestamps are stored and exchanged as ISO8601 TEXT.

use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Repo {
    pub id: i64,
    pub full_name: String,
    pub name: String,
    pub owner: String,
    pub private: bool,
    pub language: Option<String>,
    pub description: Option<String>,
    pub default_branch: String,
    pub tracked: bool,
    pub local_path: Option<String>,
    pub clone_status: String,
    pub ahead: i64,
    pub behind: i64,
    pub dirty: bool,
    pub last_fetch: Option<String>,
    pub last_commit_at: Option<String>,
    pub disk_kb: i64,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Prompt {
    pub id: i64,
    pub repo_id: Option<i64>,
    pub scope: String,
    pub source: String,
    pub model: Option<String>,
    pub prompt: String,
    pub response: Option<String>,
    pub tokens_in: i64,
    pub tokens_out: i64,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct BulkJob {
    pub id: i64,
    pub kind: String,
    pub prompt: String,
    pub status: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct BulkJobItem {
    pub id: i64,
    pub job_id: i64,
    pub repo_id: i64,
    pub status: String,
    pub branch: Option<String>,
    pub log: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct InfraResource {
    pub id: i64,
    pub name: String,
    pub kind: String,
    pub host: String,
    pub port: i64,
    pub username: Option<String>,
    pub base_path: Option<String>,
    pub notes: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Setting {
    pub scope: String,
    pub repo_id: Option<i64>,
    pub key: String,
    pub value: String,
}
