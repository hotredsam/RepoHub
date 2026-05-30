//! Drive the local `claude` CLI in headless streaming mode.
//!
//! We spawn `claude --print --output-format stream-json --verbose -p "<prompt>"`
//! with the repo path as the working directory, then parse the JSON-lines on
//! stdout, forwarding assistant text. A non-streaming collector is provided too.

use anyhow::{anyhow, Context};
use serde_json::Value;
use std::path::Path;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc::Sender;

/// Result of a Claude run.
#[derive(Debug, Clone, Default)]
pub struct RunOutcome {
    pub response: String,
    pub tokens_in: i64,
    pub tokens_out: i64,
}

/// Extract any assistant text from a single parsed stream-json line.
///
/// Be defensive: the format may carry assistant content as
/// `{type:"assistant", message:{content:[{type:"text", text:"..."}, ...]}}`
/// or as a plain `{type:"text", text:"..."}` chunk.
fn extract_assistant_text(v: &Value) -> Option<String> {
    let ty = v.get("type").and_then(|t| t.as_str());

    if ty == Some("assistant") {
        if let Some(content) = v.pointer("/message/content").and_then(|c| c.as_array()) {
            let mut buf = String::new();
            for block in content {
                if block.get("type").and_then(|t| t.as_str()) == Some("text") {
                    if let Some(t) = block.get("text").and_then(|t| t.as_str()) {
                        buf.push_str(t);
                    }
                }
            }
            if !buf.is_empty() {
                return Some(buf);
            }
        }
    }

    if ty == Some("text") {
        if let Some(t) = v.get("text").and_then(|t| t.as_str()) {
            return Some(t.to_string());
        }
    }

    None
}

/// Pull token usage out of a result/usage-bearing line, if present.
fn extract_usage(v: &Value) -> Option<(i64, i64)> {
    let usage = v
        .pointer("/message/usage")
        .or_else(|| v.get("usage"))?;
    let tin = usage
        .get("input_tokens")
        .and_then(|n| n.as_i64())
        .unwrap_or(0);
    let tout = usage
        .get("output_tokens")
        .and_then(|n| n.as_i64())
        .unwrap_or(0);
    Some((tin, tout))
}

/// Run Claude, streaming assistant text chunks to `tx` as they arrive.
pub async fn run_stream(
    cwd: &Path,
    prompt: &str,
    tx: Sender<String>,
) -> anyhow::Result<RunOutcome> {
    let mut child = Command::new("claude")
        .args([
            "--print",
            "--output-format",
            "stream-json",
            "--verbose",
            "-p",
            prompt,
        ])
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to spawn `claude` (is the CLI installed and on PATH?)")?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("could not capture claude stdout"))?;
    let mut reader = BufReader::new(stdout).lines();

    let mut outcome = RunOutcome::default();

    while let Some(line) = reader.next_line().await? {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            // Non-JSON line — forward verbatim so the user still sees something.
            let _ = tx.send(line.to_string()).await;
            outcome.response.push_str(line);
            continue;
        };

        if let Some(text) = extract_assistant_text(&v) {
            let _ = tx.send(text.clone()).await;
            outcome.response.push_str(&text);
        }
        if let Some((tin, tout)) = extract_usage(&v) {
            outcome.tokens_in += tin;
            outcome.tokens_out += tout;
        }
    }

    let status = child.wait().await?;
    if !status.success() {
        let mut err = String::new();
        if let Some(mut stderr) = child.stderr.take() {
            use tokio::io::AsyncReadExt;
            let _ = stderr.read_to_string(&mut err).await;
        }
        if outcome.response.is_empty() {
            return Err(anyhow!(
                "claude exited with {}: {}",
                status,
                err.trim()
            ));
        }
    }

    Ok(outcome)
}

/// Run Claude and collect the full assistant response (non-streaming).
pub async fn run_collect(cwd: &Path, prompt: &str) -> anyhow::Result<RunOutcome> {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(64);
    // Drain the channel concurrently so the sender never blocks on a full buffer.
    let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });
    let outcome = run_stream(cwd, prompt, tx).await;
    let _ = drain.await;
    outcome
}
