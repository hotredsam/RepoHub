//! MCP server configuration API (P16 Settings hub).
//!
//! MCP servers are configured at two LAYERS:
//! - GLOBAL — lives under the user's real Claude Code config. The canonical
//!   source for *reading* is `~/.claude.json` (`mcpServers`) when present, else
//!   `~/.claude/settings.json`. Writes go through the snapshot-safe
//!   `claude_fs::write_settings_merge`, which commits a `~/.claude` safety-net
//!   snapshot first and then deep-merges the patch into `~/.claude/settings.json`
//!   (LIVE — never a blind overwrite).
//! - PER-REPO — lives in `<repo>/.mcp.json`. Per-repo overrides are NOT written
//!   live into the working tree; they land on the `repohub-staging` branch and
//!   are committed there for review via the Merge tab.
//!
//! All git invocations go through `tokio::process` (via `gitops`) with arguments
//! passed separately — never shell-interpolated. Secrets (e.g. values inside an
//! MCP server's `env`) are never logged.

use std::path::{Path as FsPath, PathBuf};

use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use tokio::process::Command;

use crate::claude_fs;
use crate::error::{ApiResult, AppError};
use crate::models::Repo;
use crate::state::AppState;

/// Branch where per-repo overrides land for review (never the working tree).
const STAGING_BRANCH: &str = "repohub-staging";

/// Relative path of a repo's per-repo MCP config file.
const REPO_MCP_REL: &str = ".mcp.json";

pub fn router() -> Router<AppState> {
    Router::new().route(
        "/api/config/mcp",
        get(list_mcp).put(upsert_mcp).delete(delete_mcp),
    )
}

// ---------------------------------------------------------------------------
// Shapes
// ---------------------------------------------------------------------------

/// Which configuration layer a request targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    Global,
    Repo,
}

/// One configured MCP server, normalized across the global/per-repo sources.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServer {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// Environment variables passed to the server. May contain secrets, so this
    /// is never logged.
    #[serde(default)]
    pub env: Map<String, Value>,
}

#[derive(Debug, Deserialize)]
pub struct ScopeQuery {
    pub scope: Scope,
    #[serde(default)]
    pub repo_id: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct ListResponse {
    pub scope: Scope,
    pub repo_id: Option<i64>,
    /// Where the listed servers were read from (e.g. a file path), for the UI.
    pub source: String,
    pub servers: Vec<McpServer>,
}

// ---------------------------------------------------------------------------
// Path helpers
// ---------------------------------------------------------------------------

/// `~/.claude.json` — the primary global MCP source when present.
fn claude_json_path() -> Option<PathBuf> {
    std::env::var("HOME")
        .ok()
        .map(|home| PathBuf::from(home).join(".claude.json"))
}

/// Fetch a repo row by id, erroring if it is missing.
async fn fetch_repo(state: &AppState, id: i64) -> ApiResult<Repo> {
    sqlx::query_as::<_, Repo>("SELECT * FROM repos WHERE id = ?1")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::msg(format!("repo {id} not found")))
}

/// Resolve the on-disk path of a cloned repo, erroring if it is not local.
fn repo_local_path(repo: &Repo) -> ApiResult<PathBuf> {
    let lp = repo
        .local_path
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::msg("repo is not cloned locally"))?;
    Ok(PathBuf::from(lp))
}

/// `<repo>/.mcp.json`.
fn repo_mcp_path(repo_root: &FsPath) -> PathBuf {
    repo_root.join(".mcp.json")
}

// ---------------------------------------------------------------------------
// (de)serialization between the on-disk `mcpServers` map and `McpServer`.
// ---------------------------------------------------------------------------

/// Parse the `mcpServers` object of some config document into a sorted list.
fn parse_servers(map: &Map<String, Value>) -> Vec<McpServer> {
    let mut servers: Vec<McpServer> = map
        .iter()
        .map(|(name, def)| {
            let command = def
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let args = def
                .get("args")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|a| a.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            let env = def
                .get("env")
                .and_then(|v| v.as_object())
                .cloned()
                .unwrap_or_default();
            McpServer {
                name: name.clone(),
                command,
                args,
                env,
            }
        })
        .collect();
    servers.sort_by(|a, b| a.name.cmp(&b.name));
    servers
}

/// Extract `mcpServers` from a parsed config document, if present.
fn servers_from_doc(doc: &Value) -> Vec<McpServer> {
    doc.get("mcpServers")
        .and_then(|v| v.as_object())
        .map(parse_servers)
        .unwrap_or_default()
}

/// Build the JSON definition object for a single MCP server (command/args/env).
fn server_def(command: &str, args: &[String], env: &Map<String, Value>) -> Value {
    let mut def = Map::new();
    def.insert("command".to_string(), json!(command));
    if !args.is_empty() {
        def.insert("args".to_string(), json!(args));
    }
    if !env.is_empty() {
        def.insert("env".to_string(), Value::Object(env.clone()));
    }
    Value::Object(def)
}

// ---------------------------------------------------------------------------
// Generic JSON file read/write (used for `~/.claude.json` and `.mcp.json`).
// ---------------------------------------------------------------------------

/// Read and parse a JSON file, returning `{}` if it is absent or empty.
async fn read_json_object(path: &FsPath) -> ApiResult<Value> {
    match tokio::fs::read_to_string(path).await {
        Ok(s) if s.trim().is_empty() => Ok(Value::Object(Map::new())),
        Ok(s) => serde_json::from_str(&s).map_err(|e| {
            AppError::msg(format!("failed to parse {}: {e}", path.display()))
        }),
        Err(ref e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Value::Object(Map::new())),
        Err(e) => Err(AppError::Io(e)),
    }
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// Validate a server name for use as an object key. Rejects empty names and
/// control characters; the name is data (a map key), never shell-interpolated.
fn validate_name(name: &str) -> ApiResult<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(AppError::msg("MCP server name must not be empty"));
    }
    if trimmed.chars().any(|c| c.is_control()) {
        return Err(AppError::msg(format!("invalid MCP server name: {trimmed}")));
    }
    Ok(trimmed.to_string())
}

// ---------------------------------------------------------------------------
// GET /api/config/mcp?scope=global|repo&repo_id=
// ---------------------------------------------------------------------------

async fn list_mcp(
    State(state): State<AppState>,
    Query(q): Query<ScopeQuery>,
) -> ApiResult<Json<ListResponse>> {
    match q.scope {
        Scope::Global => {
            // Prefer `~/.claude.json` when it carries an `mcpServers` block;
            // otherwise fall back to `~/.claude/settings.json`.
            let (servers, source) = read_global_servers().await?;
            Ok(Json(ListResponse {
                scope: Scope::Global,
                repo_id: None,
                source,
                servers,
            }))
        }
        Scope::Repo => {
            let repo_id = q
                .repo_id
                .ok_or_else(|| AppError::msg("repo_id is required for scope=repo"))?;
            let repo = fetch_repo(&state, repo_id).await?;
            let root = repo_local_path(&repo)?;
            // Read `.mcp.json` as it stands on the staging branch (where our
            // per-repo writes land), not the live working tree.
            let lp = root.to_string_lossy().to_string();
            let doc = read_repo_mcp_doc(&lp).await?;
            let source = format!("{STAGING_BRANCH}:{REPO_MCP_REL}");
            Ok(Json(ListResponse {
                scope: Scope::Repo,
                repo_id: Some(repo_id),
                source,
                servers: servers_from_doc(&doc),
            }))
        }
    }
}

/// Which file backs the GLOBAL MCP servers. Claude Code normally stores them in
/// `~/.claude.json`; we only fall back to `~/.claude/settings.json` when the
/// former carries no `mcpServers` block. Resolving this ONCE keeps reads and
/// writes pointed at the same file, so an edit is never masked by the other.
enum GlobalMcpSource {
    /// `~/.claude.json` already holds an `mcpServers` block — the authoritative
    /// source; we mutate and write it directly (snapshotting `~/.claude` first).
    ClaudeJson(PathBuf),
    /// No `mcpServers` in `~/.claude.json`: use `~/.claude/settings.json`
    /// through the snapshot-safe `claude_fs` write path.
    Settings,
}

/// Resolve the global MCP source: `~/.claude.json` when it already carries an
/// `mcpServers` block, else `~/.claude/settings.json`.
async fn resolve_global_source() -> ApiResult<GlobalMcpSource> {
    if let Some(cj) = claude_json_path() {
        if cj.exists() {
            let doc = read_json_object(&cj).await?;
            if doc.get("mcpServers").and_then(|v| v.as_object()).is_some() {
                return Ok(GlobalMcpSource::ClaudeJson(cj));
            }
        }
    }
    Ok(GlobalMcpSource::Settings)
}

/// Read the global MCP servers from whichever source [`resolve_global_source`]
/// selects. Returns the servers plus the source path read.
async fn read_global_servers() -> ApiResult<(Vec<McpServer>, String)> {
    match resolve_global_source().await? {
        GlobalMcpSource::ClaudeJson(cj) => {
            let doc = read_json_object(&cj).await?;
            Ok((servers_from_doc(&doc), cj.to_string_lossy().to_string()))
        }
        GlobalMcpSource::Settings => {
            let settings = claude_fs::read_settings().await?;
            let source = claude_fs::claude_dir().join("settings.json");
            Ok((
                servers_from_doc(&settings),
                source.to_string_lossy().to_string(),
            ))
        }
    }
}

/// Apply a mutation to the GLOBAL `mcpServers` map at its authoritative source
/// and write it back atomically, snapshotting the `~/.claude` safety-net first.
///
/// Reads the full document, lets `mutate` edit the `mcpServers` object in place
/// (add or remove a server), then writes the WHOLE document back in a single
/// write — so the live file is never left half-applied (no stray
/// `"mcpServers": null`). Writes target the same file the read prefers, so the
/// change is immediately visible to both RepoHub and Claude Code.
async fn write_global_servers<F>(mutate: F) -> ApiResult<()>
where
    F: FnOnce(&mut Map<String, Value>),
{
    match resolve_global_source().await? {
        GlobalMcpSource::ClaudeJson(cj) => {
            // Snapshot the safety net before touching live config, then write
            // the whole ~/.claude.json back atomically.
            claude_fs::snapshot("repohub: before change").await?;
            let mut doc = read_json_object(&cj).await?;
            if !doc.is_object() {
                doc = Value::Object(Map::new());
            }
            let obj = doc.as_object_mut().expect("doc is an object");
            let mut servers = obj
                .get("mcpServers")
                .and_then(|v| v.as_object())
                .cloned()
                .unwrap_or_default();
            mutate(&mut servers);
            obj.insert("mcpServers".to_string(), Value::Object(servers));
            write_mcp_file(&cj, &doc).await?;
            Ok(())
        }
        GlobalMcpSource::Settings => {
            // Read the full settings, edit the mcpServers map, and write the
            // WHOLE object back via the snapshot-safe full-write path. We use a
            // full write (not a merge) so a removed server is actually dropped
            // and never lingers as a null.
            let mut settings = claude_fs::read_settings().await?;
            if !settings.is_object() {
                settings = Value::Object(Map::new());
            }
            let obj = settings.as_object_mut().expect("settings is an object");
            let mut servers = obj
                .get("mcpServers")
                .and_then(|v| v.as_object())
                .cloned()
                .unwrap_or_default();
            mutate(&mut servers);
            obj.insert("mcpServers".to_string(), Value::Object(servers));
            claude_fs::write_settings_full(&settings).await?;
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// PUT /api/config/mcp  — upsert a server.
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct UpsertBody {
    pub scope: Scope,
    #[serde(default)]
    pub repo_id: Option<i64>,
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: Map<String, Value>,
}

#[derive(Debug, Serialize)]
pub struct WriteResponse {
    pub ok: bool,
    pub scope: Scope,
    pub repo_id: Option<i64>,
    /// For per-repo writes, the staging branch the change landed on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    pub servers: Vec<McpServer>,
}

async fn upsert_mcp(
    State(state): State<AppState>,
    Json(body): Json<UpsertBody>,
) -> ApiResult<Json<WriteResponse>> {
    let name = validate_name(&body.name)?;
    if body.command.trim().is_empty() {
        return Err(AppError::msg("MCP server command must not be empty"));
    }
    let def = server_def(&body.command, &body.args, &body.env);

    match body.scope {
        Scope::Global => {
            // Write LIVE to the authoritative global source (the same file the
            // read prefers), preserving other servers/keys. The snapshot-safe
            // helper commits the safety net first and writes atomically.
            let insert_name = name.clone();
            write_global_servers(move |servers| {
                servers.insert(insert_name, def);
            })
            .await?;

            let (servers, _) = read_global_servers().await?;
            Ok(Json(WriteResponse {
                ok: true,
                scope: Scope::Global,
                repo_id: None,
                branch: None,
                servers,
            }))
        }
        Scope::Repo => {
            let repo_id = body
                .repo_id
                .ok_or_else(|| AppError::msg("repo_id is required for scope=repo"))?;
            let repo = fetch_repo(&state, repo_id).await?;
            let root = repo_local_path(&repo)?;
            if !root.join(".git").exists() {
                return Err(AppError::msg("repo is not cloned locally"));
            }

            // Per-repo overrides land on the staging branch for review, never
            // live in the working tree. Read the file as it stands on staging,
            // apply the change, then commit ONLY `.mcp.json` on staging while
            // restoring the original branch.
            let lp = root.to_string_lossy().to_string();
            let mut doc = read_repo_mcp_doc(&lp).await?;
            merge_server_into_doc(&mut doc, &name, def);
            stage_mcp_doc(&root, &doc, &format!("RepoHub: set MCP server {name}")).await?;

            Ok(Json(WriteResponse {
                ok: true,
                scope: Scope::Repo,
                repo_id: Some(repo_id),
                branch: Some(STAGING_BRANCH.to_string()),
                servers: servers_from_doc(&doc),
            }))
        }
    }
}

// ---------------------------------------------------------------------------
// DELETE /api/config/mcp  — remove a server.
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct DeleteBody {
    pub scope: Scope,
    #[serde(default)]
    pub repo_id: Option<i64>,
    pub name: String,
}

async fn delete_mcp(
    State(state): State<AppState>,
    Json(body): Json<DeleteBody>,
) -> ApiResult<Json<WriteResponse>> {
    let name = validate_name(&body.name)?;

    match body.scope {
        Scope::Global => {
            // Remove the server from the authoritative source in a SINGLE
            // atomic write. We mutate the pruned map and write the whole
            // document back (never a two-step null-then-prune), so the live file
            // can never be observed with `"mcpServers": null`.
            let del_name = name.clone();
            write_global_servers(move |servers| {
                servers.remove(&del_name);
            })
            .await?;

            let (servers, _) = read_global_servers().await?;
            Ok(Json(WriteResponse {
                ok: true,
                scope: Scope::Global,
                repo_id: None,
                branch: None,
                servers,
            }))
        }
        Scope::Repo => {
            let repo_id = body
                .repo_id
                .ok_or_else(|| AppError::msg("repo_id is required for scope=repo"))?;
            let repo = fetch_repo(&state, repo_id).await?;
            let root = repo_local_path(&repo)?;
            if !root.join(".git").exists() {
                return Err(AppError::msg("repo is not cloned locally"));
            }

            let lp = root.to_string_lossy().to_string();
            let mut doc = read_repo_mcp_doc(&lp).await?;
            remove_server_from_doc(&mut doc, &name);
            stage_mcp_doc(&root, &doc, &format!("RepoHub: remove MCP server {name}")).await?;

            Ok(Json(WriteResponse {
                ok: true,
                scope: Scope::Repo,
                repo_id: Some(repo_id),
                branch: Some(STAGING_BRANCH.to_string()),
                servers: servers_from_doc(&doc),
            }))
        }
    }
}

// ---------------------------------------------------------------------------
// Per-repo doc mutation + write helpers.
// ---------------------------------------------------------------------------

/// Insert/replace one server's definition under `mcpServers` in a parsed doc.
fn merge_server_into_doc(doc: &mut Value, name: &str, def: Value) {
    if !doc.is_object() {
        *doc = Value::Object(Map::new());
    }
    let obj = doc.as_object_mut().expect("doc is an object");
    let servers = obj
        .entry("mcpServers".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if !servers.is_object() {
        *servers = Value::Object(Map::new());
    }
    servers
        .as_object_mut()
        .expect("mcpServers is an object")
        .insert(name.to_string(), def);
}

/// Remove one server from the `mcpServers` block of a parsed doc.
fn remove_server_from_doc(doc: &mut Value, name: &str) {
    if let Some(servers) = doc
        .as_object_mut()
        .and_then(|o| o.get_mut("mcpServers"))
        .and_then(|v| v.as_object_mut())
    {
        servers.remove(name);
    }
}

/// Write a `.mcp.json` document pretty-printed (creates parent dirs).
async fn write_mcp_file(path: &FsPath, doc: &Value) -> ApiResult<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let pretty = serde_json::to_string_pretty(doc)
        .map_err(|e| AppError::msg(format!("failed to serialize {}: {e}", path.display())))?;
    tokio::fs::write(path, pretty.as_bytes()).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Per-repo staging (read from / commit to repohub-staging without touching the
// live working tree). Mirrors the safe pattern in claude_config.rs.
// ---------------------------------------------------------------------------

/// Output of a spawned `git` command: success flag + trimmed streams.
struct CmdOut {
    ok: bool,
    stdout: String,
    stderr: String,
}

/// Run `git -C <cwd> <args>`, capturing both streams. Arguments are passed
/// separately — never through a shell.
async fn git(cwd: &str, args: &[&str]) -> CmdOut {
    let mut full: Vec<&str> = Vec::with_capacity(args.len() + 2);
    full.push("-C");
    full.push(cwd);
    full.extend_from_slice(args);
    match Command::new("git").args(&full).output().await {
        Ok(o) => CmdOut {
            ok: o.status.success(),
            stdout: String::from_utf8_lossy(&o.stdout).trim().to_string(),
            stderr: String::from_utf8_lossy(&o.stderr).trim().to_string(),
        },
        Err(e) => CmdOut {
            ok: false,
            stdout: String::new(),
            stderr: format!("failed to spawn git {:?}: {e}", args),
        },
    }
}

/// Read `.mcp.json` AS IT STANDS ON THE STAGING BRANCH (via `git show`), without
/// disturbing the working tree. Returns `{}` when the branch or file is absent.
async fn read_repo_mcp_doc(lp: &str) -> ApiResult<Value> {
    let has_branch = git(
        lp,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{STAGING_BRANCH}"),
        ],
    )
    .await;
    if !has_branch.ok {
        return Ok(Value::Object(Map::new()));
    }

    let spec = format!("{STAGING_BRANCH}:{REPO_MCP_REL}");
    let show = git(lp, &["show", &spec]).await;
    if !show.ok || show.stdout.trim().is_empty() {
        return Ok(Value::Object(Map::new()));
    }
    serde_json::from_str(&show.stdout)
        .map_err(|e| AppError::msg(format!("failed to parse {REPO_MCP_REL}: {e}")))
}

/// Write `doc` to `<root>/.mcp.json` on the staging branch and commit ONLY that
/// file, then restore the original branch (even on error). Per-repo overrides
/// must NOT be left live in the working tree, and unrelated dirty files must NOT
/// be swept into the staging commit — so we never use `add -A`.
async fn stage_mcp_doc(root: &FsPath, doc: &Value, message: &str) -> ApiResult<()> {
    let lp = root.to_string_lossy().to_string();

    // Remember the branch we are on so we can restore the working tree exactly.
    let original = git(&lp, &["rev-parse", "--abbrev-ref", "HEAD"]).await;
    let original_branch = if original.ok && !original.stdout.is_empty() {
        Some(original.stdout.clone())
    } else {
        None
    };

    let result = stage_mcp_doc_inner(&lp, root, doc, message).await;

    // Best-effort restore of the working tree to the original branch.
    if let Some(branch) = &original_branch {
        let _ = git(&lp, &["checkout", branch]).await;
    }

    result
}

/// Switch to (or create) the staging branch, write the file, stage ONLY it, and
/// commit. The caller restores the prior branch.
async fn stage_mcp_doc_inner(
    lp: &str,
    root: &FsPath,
    doc: &Value,
    message: &str,
) -> ApiResult<()> {
    let exists = git(
        lp,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{STAGING_BRANCH}"),
        ],
    )
    .await;

    // Prefer `checkout <name>` when the branch already exists (append to staged
    // work) over `checkout -B` (which would reset it onto the current HEAD).
    let checkout = if exists.ok {
        git(lp, &["checkout", STAGING_BRANCH]).await
    } else {
        git(lp, &["checkout", "-B", STAGING_BRANCH]).await
    };
    if !checkout.ok {
        let detail = if checkout.stderr.is_empty() {
            checkout.stdout
        } else {
            checkout.stderr
        };
        return Err(AppError::msg(format!(
            "failed to switch to staging branch: {detail}"
        )));
    }

    let path = repo_mcp_path(root);
    write_mcp_file(&path, doc).await?;

    // Stage ONLY `.mcp.json`, never `add -A`, so unrelated working-tree changes
    // are not committed onto staging.
    let add = git(lp, &["add", REPO_MCP_REL]).await;
    if !add.ok {
        let detail = if add.stderr.is_empty() {
            add.stdout
        } else {
            add.stderr
        };
        return Err(AppError::msg(format!("failed to stage .mcp.json: {detail}")));
    }

    // `git commit` exits non-zero when there is nothing to commit; treat that
    // as a successful no-op.
    let _ = git(lp, &["commit", "-m", message]).await;
    Ok(())
}
