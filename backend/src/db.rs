//! SQLite database bootstrap.
//!
//! Connects to the on-disk database (creating the file if missing) and applies
//! an embedded idempotent schema. Uses runtime sqlx only — no compile-time
//! `query!` macros, so no `DATABASE_URL` is required at build time.

use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::SqlitePool;
use std::str::FromStr;

use crate::config::Config;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS repos (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    full_name       TEXT NOT NULL UNIQUE,
    name            TEXT NOT NULL,
    owner           TEXT NOT NULL,
    private         INTEGER NOT NULL DEFAULT 0,
    language        TEXT,
    description     TEXT,
    default_branch  TEXT NOT NULL DEFAULT 'main',
    tracked         INTEGER NOT NULL DEFAULT 0,
    local_path      TEXT,
    clone_status    TEXT NOT NULL DEFAULT 'absent',
    ahead           INTEGER NOT NULL DEFAULT 0,
    behind          INTEGER NOT NULL DEFAULT 0,
    dirty           INTEGER NOT NULL DEFAULT 0,
    last_fetch      TEXT,
    last_commit_at  TEXT,
    disk_kb         INTEGER NOT NULL DEFAULT 0,
    updated_at      TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS idx_repos_tracked ON repos(tracked);

CREATE TABLE IF NOT EXISTS prompts (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    repo_id     INTEGER,
    scope       TEXT NOT NULL,
    source      TEXT NOT NULL,
    model       TEXT,
    prompt      TEXT NOT NULL,
    response    TEXT,
    tokens_in   INTEGER NOT NULL DEFAULT 0,
    tokens_out  INTEGER NOT NULL DEFAULT 0,
    created_at  TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_prompts_repo ON prompts(repo_id);
CREATE INDEX IF NOT EXISTS idx_prompts_created ON prompts(created_at);

CREATE TABLE IF NOT EXISTS bulk_jobs (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    kind        TEXT NOT NULL,
    prompt      TEXT NOT NULL,
    status      TEXT NOT NULL,
    created_at  TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS bulk_job_items (
    id      INTEGER PRIMARY KEY AUTOINCREMENT,
    job_id  INTEGER NOT NULL,
    repo_id INTEGER NOT NULL,
    status  TEXT NOT NULL,
    branch  TEXT,
    log     TEXT,
    error   TEXT
);
CREATE INDEX IF NOT EXISTS idx_bulk_items_job ON bulk_job_items(job_id);

CREATE TABLE IF NOT EXISTS infra_resources (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    name        TEXT NOT NULL,
    kind        TEXT NOT NULL,
    host        TEXT NOT NULL,
    port        INTEGER NOT NULL DEFAULT 22,
    username    TEXT,
    base_path   TEXT,
    notes       TEXT,
    created_at  TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS settings (
    scope   TEXT NOT NULL,
    repo_id INTEGER,
    key     TEXT NOT NULL,
    value   TEXT NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_settings_unique
    ON settings(scope, IFNULL(repo_id, -1), key);

CREATE TABLE IF NOT EXISTS eval_suites (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    name        TEXT NOT NULL,
    scope       TEXT NOT NULL,
    repo_id     INTEGER,
    config_json TEXT,
    created_at  TEXT
);

CREATE TABLE IF NOT EXISTS eval_runs (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    suite_id    INTEGER,
    model       TEXT,
    score       REAL,
    passed      INTEGER,
    total       INTEGER,
    detail_json TEXT,
    created_at  TEXT
);
CREATE INDEX IF NOT EXISTS idx_eval_runs_suite ON eval_runs(suite_id);
"#;

/// Connect to the SQLite database and apply the embedded schema.
pub async fn init(cfg: &Config) -> anyhow::Result<SqlitePool> {
    let path = cfg.db_path();
    let opts = SqliteConnectOptions::from_str(&format!("sqlite://{}", path.display()))?
        .create_if_missing(true);

    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(opts)
        .await?;

    // Apply schema statement-by-statement so we can run them on the same pool.
    for stmt in SCHEMA.split(';') {
        let stmt = stmt.trim();
        if stmt.is_empty() {
            continue;
        }
        sqlx::query(stmt).execute(&pool).await?;
    }

    Ok(pool)
}
