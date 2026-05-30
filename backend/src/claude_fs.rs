//! Manage the user's real `~/.claude` configuration directory and its git
//! versioning (the P16 safety net).
//!
//! GLOBAL Claude Code settings are applied LIVE to `~/.claude/settings.json`.
//! To make that safe we keep `~/.claude` under git and snapshot (commit) before
//! every change. The git repo is initialized LAZILY — only the first time the
//! user actually makes a change via the API — NEVER on boot.
//!
//! All git/gh invocations go through `tokio::process` with arguments passed
//! separately (never shell-interpolated). Secrets are never logged.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use tokio::process::Command;

use crate::error::{ApiResult, AppError};

/// Contents of the `~/.claude` safety-net `.gitignore`.
///
/// The real `~/.claude` holds many heavy, ephemeral, and potentially
/// secret/PII-bearing entries whose exact names vary across installs and grow
/// over time (sessions, caches, plugins, local settings, …). A denylist is
/// fragile here — any future heavy/secret directory we did not enumerate would
/// be committed. So we use an ALLOWLIST: ignore everything, then explicitly
/// un-ignore ONLY the config files RepoHub manages. This guarantees the snapshot
/// never balloons and never captures secrets, even as the directory evolves.
const GITIGNORE_BODY: &str = "\
# RepoHub safety-net repo for ~/.claude.
# Allowlist: ignore everything, then re-include only the files RepoHub versions.
# This keeps heavy/ephemeral/secret paths (sessions/, cache/, plugins/,
# settings.local.json, .credentials*, …) out of the snapshot by default.
*
!.gitignore
!settings.json
!keybindings.json
!agents/
!agents/**
";

/// Absolute path to the user's `~/.claude` directory.
///
/// Resolves `$HOME`; falls back to `.claude` in the current directory only if
/// `$HOME` is unset (which should not happen in practice).
pub fn claude_dir() -> PathBuf {
    match std::env::var("HOME") {
        Ok(home) => PathBuf::from(home).join(".claude"),
        Err(_) => PathBuf::from(".claude"),
    }
}

/// Path to `~/.claude/settings.json`.
fn settings_path() -> PathBuf {
    claude_dir().join("settings.json")
}

/// Run `git <args>` inside `~/.claude`, returning stdout on success.
///
/// Arguments are passed separately to the process — never shell-interpolated —
/// so untrusted input (e.g. commit hashes) cannot break out.
async fn git(args: &[&str]) -> ApiResult<String> {
    let dir = claude_dir();
    let out = Command::new("git")
        .args(args)
        .current_dir(&dir)
        .output()
        .await
        .map_err(|e| AppError::msg(format!("failed to spawn git: {e}")))?;
    if !out.status.success() {
        return Err(AppError::msg(format!(
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Whether `~/.claude` is already a git repository.
async fn is_repo() -> bool {
    claude_dir().join(".git").exists()
}

/// LAZILY initialize the `~/.claude` git safety-net repo if it is not one yet.
///
/// On first init this also writes a `.gitignore` excluding heavy/secret/
/// ephemeral paths. Idempotent and best-effort-friendly. NEVER called on boot —
/// only when the user actually makes a change via the API.
pub async fn ensure_repo() -> ApiResult<()> {
    let dir = claude_dir();
    tokio::fs::create_dir_all(&dir).await?;

    if is_repo().await {
        // Still ensure the .gitignore exists (idempotent repair).
        ensure_gitignore(&dir).await?;
        return Ok(());
    }

    git(&["init"]).await?;
    ensure_gitignore(&dir).await?;
    Ok(())
}

/// Write `~/.claude/.gitignore` if it is absent. Idempotent.
async fn ensure_gitignore(dir: &Path) -> ApiResult<()> {
    let path = dir.join(".gitignore");
    if !path.exists() {
        tokio::fs::write(&path, GITIGNORE_BODY).await?;
    }
    Ok(())
}

/// Snapshot the current state of `~/.claude`: `ensure_repo()` then
/// `git add -A && git commit -m <msg>`. Allow-empty so a commit is always
/// recorded. Best-effort: failures are surfaced but callers may ignore them.
pub async fn snapshot(msg: &str) -> ApiResult<()> {
    ensure_repo().await?;
    git(&["add", "-A"]).await?;
    // `--allow-empty` so we always have a commit even when nothing changed.
    git(&["commit", "--allow-empty", "-m", msg]).await?;
    Ok(())
}

/// Read and parse `~/.claude/settings.json`, returning `{}` if absent or empty.
pub async fn read_settings() -> ApiResult<Value> {
    let path = settings_path();
    match tokio::fs::read_to_string(&path).await {
        Ok(s) if s.trim().is_empty() => Ok(Value::Object(Map::new())),
        Ok(s) => serde_json::from_str(&s)
            .map_err(|e| AppError::msg(format!("failed to parse ~/.claude/settings.json: {e}"))),
        Err(ref e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Value::Object(Map::new())),
        Err(e) => Err(AppError::Io(e)),
    }
}

/// Recursively deep-merge `patch` into `target`.
///
/// Objects are merged key-by-key; any non-object value (including arrays) in
/// `patch` replaces the corresponding value in `target`. A `null` in the patch
/// overwrites with `null` (callers wanting a delete should handle that above).
pub fn deep_merge(target: &mut Value, patch: &Value) {
    match (target, patch) {
        (Value::Object(t), Value::Object(p)) => {
            for (k, v) in p {
                deep_merge(t.entry(k.clone()).or_insert(Value::Null), v);
            }
        }
        (t, p) => {
            *t = p.clone();
        }
    }
}

/// Apply a settings patch LIVE to `~/.claude/settings.json`.
///
/// 1. Snapshot the current state ("repohub: before change") for the safety net.
/// 2. Deep-merge `patch` into the existing settings (MERGE keys, never blind
///    overwrite).
/// 3. Write the merged result back, pretty-printed.
pub async fn write_settings_merge(patch: &Value) -> ApiResult<()> {
    // Safety net: commit BEFORE we touch the file.
    snapshot("repohub: before change").await?;

    let mut current = read_settings().await?;
    // Guarantee we are merging into an object even if the file held a scalar.
    if !current.is_object() {
        current = Value::Object(Map::new());
    }
    deep_merge(&mut current, patch);

    let pretty = serde_json::to_string_pretty(&current)
        .map_err(|e| AppError::msg(format!("failed to serialize settings: {e}")))?;

    let dir = claude_dir();
    tokio::fs::create_dir_all(&dir).await?;
    tokio::fs::write(settings_path(), pretty.as_bytes()).await?;
    Ok(())
}

/// Write a COMPLETE settings object LIVE to `~/.claude/settings.json`.
///
/// Unlike [`write_settings_merge`] this replaces the whole document (callers use
/// it when they must REMOVE a key — deep-merge can only add/replace keys, never
/// delete). It still snapshots the safety-net repo BEFORE the write so the prior
/// state is recoverable, and performs a single atomic write so the live file is
/// never left in a half-applied state (e.g. a stray `"mcpServers": null`).
pub async fn write_settings_full(settings: &Value) -> ApiResult<()> {
    // Safety net: commit BEFORE we touch the file.
    snapshot("repohub: before change").await?;

    let pretty = serde_json::to_string_pretty(settings)
        .map_err(|e| AppError::msg(format!("failed to serialize settings: {e}")))?;

    let dir = claude_dir();
    tokio::fs::create_dir_all(&dir).await?;
    tokio::fs::write(settings_path(), pretty.as_bytes()).await?;
    Ok(())
}

/// One entry in the `~/.claude` change history.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HistoryEntry {
    pub hash: String,
    pub msg: String,
    pub date: String,
}

/// Git log of `~/.claude`, newest first. Best-effort: returns `[]` if the
/// directory is not a git repo or the log cannot be read.
pub async fn history() -> Vec<HistoryEntry> {
    if !is_repo().await {
        return Vec::new();
    }
    // Use a unit separator unlikely to appear in a subject line.
    let fmt = "--pretty=format:%H\x1f%s\x1f%cI";
    let out = match git(&["log", fmt]).await {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    out.lines()
        .filter_map(|line| {
            let mut parts = line.split('\x1f');
            let hash = parts.next()?.to_string();
            let msg = parts.next().unwrap_or_default().to_string();
            let date = parts.next().unwrap_or_default().to_string();
            if hash.is_empty() {
                return None;
            }
            Some(HistoryEntry { hash, msg, date })
        })
        .collect()
}

/// Revert `~/.claude/settings.json` to its state at `hash`.
///
/// Best-effort: snapshots first (so the revert itself is recoverable), then
/// `git checkout <hash> -- settings.json`. The hash is passed as a separate
/// argument and never shell-interpolated.
pub async fn revert(hash: &str) -> ApiResult<()> {
    snapshot("repohub: before revert").await?;
    git(&["checkout", hash, "--", "settings.json"]).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Agents — `<name>.md` with YAML-ish frontmatter (hand-rolled, no extra crate).
// ---------------------------------------------------------------------------

/// A Claude Code subagent definition: YAML frontmatter fields + a Markdown body
/// that serves as the system prompt.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Agent {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Comma/space-separated tool list as authored in the frontmatter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<String>,
    /// The Markdown body below the frontmatter — the agent's system prompt.
    #[serde(default)]
    pub body: String,
}

/// Parse a single scalar value from a YAML-ish frontmatter line, trimming
/// optional surrounding quotes.
fn unquote(s: &str) -> String {
    let s = s.trim();
    let s = s.strip_prefix('"').unwrap_or(s);
    let s = s.strip_suffix('"').unwrap_or(s);
    let s = s.strip_prefix('\'').unwrap_or(s);
    let s = s.strip_suffix('\'').unwrap_or(s);
    s.to_string()
}

/// Parse an agent Markdown document (frontmatter + body) into an `Agent`, where
/// `name` is the file stem. Exposed so callers reading agents from sources other
/// than the live filesystem (e.g. `git show <branch>:path`) can reuse the parser.
pub fn parse_agent_public(name: &str, content: &str) -> Agent {
    parse_agent(name, content)
}

/// Parse an agent Markdown document (frontmatter + body) into an `Agent`.
///
/// Hand-rolled simple parser: a leading `---` line, `key: value` pairs until a
/// closing `---`, then the remainder as the body. If there is no frontmatter the
/// whole content becomes the body.
fn parse_agent(name: &str, content: &str) -> Agent {
    let mut agent = Agent {
        name: name.to_string(),
        ..Default::default()
    };

    let trimmed = content.trim_start_matches('\u{feff}');
    let mut lines = trimmed.lines();

    // Frontmatter must open with a `---` line.
    let first = lines.clone().next().map(str::trim);
    if first != Some("---") {
        agent.body = content.to_string();
        return agent;
    }
    // Consume the opening fence.
    lines.next();

    let mut body_lines: Vec<&str> = Vec::new();
    let mut in_frontmatter = true;
    for line in lines {
        if in_frontmatter {
            if line.trim() == "---" {
                in_frontmatter = false;
                continue;
            }
            if let Some((k, v)) = line.split_once(':') {
                let key = k.trim().to_ascii_lowercase();
                let val = unquote(v);
                match key.as_str() {
                    "name" => {
                        if !val.is_empty() {
                            agent.name = val;
                        }
                    }
                    "description" => agent.description = Some(val),
                    "model" => agent.model = Some(val),
                    "tools" => agent.tools = Some(val),
                    _ => {}
                }
            }
        } else {
            body_lines.push(line);
        }
    }

    agent.body = body_lines.join("\n").trim_start_matches('\n').to_string();
    agent
}

/// Format an `Agent` back into a Markdown document with YAML frontmatter.
fn format_agent(agent: &Agent) -> String {
    let mut out = String::from("---\n");
    out.push_str(&format!("name: {}\n", agent.name));
    if let Some(d) = &agent.description {
        out.push_str(&format!("description: {d}\n"));
    }
    if let Some(m) = &agent.model {
        out.push_str(&format!("model: {m}\n"));
    }
    if let Some(t) = &agent.tools {
        out.push_str(&format!("tools: {t}\n"));
    }
    out.push_str("---\n\n");
    out.push_str(&agent.body);
    if !agent.body.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Sanitize an agent name into a safe `<name>.md` filename (no path traversal,
/// no separators).
fn agent_file_name(name: &str) -> ApiResult<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(AppError::msg("agent name must not be empty"));
    }
    if trimmed.contains('/')
        || trimmed.contains('\\')
        || trimmed.contains("..")
        || trimmed.contains('\0')
    {
        return Err(AppError::msg(format!("invalid agent name: {trimmed}")));
    }
    Ok(format!("{trimmed}.md"))
}

/// Read all agents in `dir` (e.g. `~/.claude/agents` or `<repo>/.claude/agents`).
///
/// Best-effort: a missing directory yields an empty list; unreadable or
/// non-`.md` entries are skipped.
pub async fn read_agents(dir: &Path) -> ApiResult<Vec<Agent>> {
    let mut agents = Vec::new();
    let mut rd = match tokio::fs::read_dir(dir).await {
        Ok(rd) => rd,
        Err(ref e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(agents),
        Err(e) => return Err(AppError::Io(e)),
    };
    while let Some(entry) = rd.next_entry().await? {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let stem = match path.file_stem().and_then(|s| s.to_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };
        if let Ok(content) = tokio::fs::read_to_string(&path).await {
            agents.push(parse_agent(&stem, &content));
        }
    }
    agents.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(agents)
}

/// Write an agent to `dir/<name>.md`, creating `dir` if needed.
pub async fn write_agent(dir: &Path, agent: &Agent) -> ApiResult<()> {
    let file = agent_file_name(&agent.name)?;
    tokio::fs::create_dir_all(dir).await?;
    let doc = format_agent(agent);
    tokio::fs::write(dir.join(file), doc.as_bytes()).await?;
    Ok(())
}

/// Delete `dir/<name>.md`. Best-effort: a missing file is treated as success.
pub async fn delete_agent(dir: &Path, name: &str) -> ApiResult<()> {
    let file = agent_file_name(name)?;
    match tokio::fs::remove_file(dir.join(file)).await {
        Ok(()) => Ok(()),
        Err(ref e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(AppError::Io(e)),
    }
}
