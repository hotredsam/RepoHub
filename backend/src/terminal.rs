//! Embedded terminal over WebSocket (`/ws/terminal`).
//!
//! On connect, the client sends a single JSON frame describing which repo (if
//! any) to open the shell in and the initial terminal size:
//!
//! ```json
//! {"repo_id": <i64|null>, "cols": <u16?>, "rows": <u16?>}
//! ```
//!
//! We then spawn the user's login shell (`$SHELL`, falling back to `/bin/zsh`)
//! inside a real PTY via [`portable_pty`], rooted at the repo's local clone when
//! `repo_id` resolves to a cloned path, otherwise at the RepoHub data root.
//!
//! Two halves bridge the PTY to the socket:
//!   * a blocking thread drains the PTY master reader and ships raw bytes to the
//!     client as text frames (terminal output is byte-oriented; the frontend
//!     xterm.js feeds it straight in),
//!   * the async task handling inbound frames writes client keystrokes to the
//!     PTY writer and honours `{"type":"resize","cols":..,"rows":..}` control
//!     frames.
//!
//! When the socket closes or the PTY hits EOF, the child is killed and the
//! blocking reader thread is detached (NOT joined) so a lingering grandchild
//! holding the slave fd can never block teardown.

use std::io::{Read, Write};

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use serde::Deserialize;

use crate::state::AppState;
use crate::ws_origin::check_origin;

/// Feature router. Merged under the main app by the integrate step.
pub fn router() -> Router<AppState> {
    Router::new().route("/ws/terminal", get(ws_terminal))
}

/// First client frame on `/ws/terminal`: which repo + initial size.
#[derive(Debug, Deserialize)]
struct OpenRequest {
    #[serde(default)]
    repo_id: Option<i64>,
    #[serde(default)]
    cols: Option<u16>,
    #[serde(default)]
    rows: Option<u16>,
}

/// Inbound control frames (anything tagged with a `type`). Untagged text/binary
/// frames are treated as raw keystrokes and written straight to the PTY.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum Control {
    Resize { cols: u16, rows: u16 },
}

const DEFAULT_COLS: u16 = 80;
const DEFAULT_ROWS: u16 = 24;

async fn ws_terminal(
    ws: WebSocketUpgrade,
    headers: HeaderMap,
    State(state): State<AppState>,
) -> impl IntoResponse {
    // Reject cross-site WebSocket hijacking: this PTY endpoint is direct,
    // unconstrained RCE, so an upgrade from a foreign Origin must never proceed.
    if let Err(status) = check_origin(&headers, &state.cfg) {
        return status.into_response();
    }
    ws.on_upgrade(move |socket| handle_socket(socket, state))
        .into_response()
}

async fn handle_socket(mut socket: WebSocket, state: AppState) {
    // Wait for the opening JSON frame describing the session.
    let req: OpenRequest = loop {
        match socket.recv().await {
            Some(Ok(Message::Text(t))) => match serde_json::from_str::<OpenRequest>(&t) {
                Ok(r) => break r,
                Err(e) => {
                    let _ = socket
                        .send(Message::Text(format!("error: invalid request: {e}")))
                        .await;
                    return;
                }
            },
            // Tolerate an early ping; keep waiting for the real opener.
            Some(Ok(Message::Ping(data))) => {
                if socket.send(Message::Pong(data)).await.is_err() {
                    return;
                }
            }
            Some(Ok(Message::Close(_))) | None => return,
            Some(Ok(_)) => continue,
            Some(Err(_)) => return,
        }
    };

    // Resolve the working directory: repo local_path if it exists on disk, else
    // the RepoHub data root (parent of repos_dir()).
    let cwd = resolve_cwd(&state, req.repo_id).await;

    let size = PtySize {
        rows: req.rows.unwrap_or(DEFAULT_ROWS),
        cols: req.cols.unwrap_or(DEFAULT_COLS),
        pixel_width: 0,
        pixel_height: 0,
    };

    // Open the PTY pair.
    let pty_system = native_pty_system();
    let pair = match pty_system.openpty(size) {
        Ok(p) => p,
        Err(e) => {
            let _ = socket
                .send(Message::Text(format!("error: failed to open pty: {e}")))
                .await;
            return;
        }
    };

    // Build the shell command rooted at the resolved cwd.
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
    let mut cmd = CommandBuilder::new(&shell);
    cmd.cwd(&cwd);
    // Give programs a sane terminal type and announce the host app.
    cmd.env("TERM", "xterm-256color");
    cmd.env("REPOHUB_TERMINAL", "1");

    let mut child = match pair.slave.spawn_command(cmd) {
        Ok(c) => c,
        Err(e) => {
            let _ = socket
                .send(Message::Text(format!("error: failed to spawn `{shell}`: {e}")))
                .await;
            return;
        }
    };

    // Once the child is spawned the slave handle is no longer needed; dropping it
    // closes our copy of the slave fd so the master sees EOF when the child exits.
    drop(pair.slave);

    // The master half: a cloned reader for the blocking output pump, a writer for
    // inbound keystrokes, and the master itself (kept for resize()).
    let reader = match pair.master.try_clone_reader() {
        Ok(r) => r,
        Err(e) => {
            let _ = child.kill();
            let _ = socket
                .send(Message::Text(format!("error: pty reader: {e}")))
                .await;
            return;
        }
    };
    let mut writer = match pair.master.take_writer() {
        Ok(w) => w,
        Err(e) => {
            let _ = child.kill();
            let _ = socket
                .send(Message::Text(format!("error: pty writer: {e}")))
                .await;
            return;
        }
    };
    let master = pair.master;

    // Channel carrying raw PTY output bytes from the blocking reader thread back
    // to this async task for delivery over the socket.
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(256);

    // Blocking thread: drain the PTY master reader until EOF. `portable-pty`
    // readers are blocking, so this must live off the async runtime.
    let reader_handle = std::thread::spawn(move || {
        let mut reader = reader;
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break, // EOF: child exited / pty closed.
                Ok(n) => {
                    if out_tx.blocking_send(buf[..n].to_vec()).is_err() {
                        // Receiver gone (socket closed) — stop reading.
                        break;
                    }
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
    });

    // Main relay loop: forward PTY output to the client, and client frames into
    // the PTY (writes + resize control frames).
    loop {
        tokio::select! {
            // PTY produced output (or the reader thread ended).
            chunk = out_rx.recv() => {
                match chunk {
                    Some(bytes) => {
                        // xterm.js consumes a UTF-8/text stream; forward bytes
                        // lossily so partial multibyte sequences never abort us.
                        let text = String::from_utf8_lossy(&bytes).into_owned();
                        if socket.send(Message::Text(text)).await.is_err() {
                            break;
                        }
                    }
                    // Reader thread hit EOF and dropped its sender.
                    None => break,
                }
            }
            // Client sent a frame.
            inbound = socket.recv() => {
                match inbound {
                    Some(Ok(Message::Text(t))) => {
                        // A text frame might be a control message; if it parses
                        // as one, act on it, otherwise treat it as keystrokes.
                        if let Ok(Control::Resize { cols, rows }) =
                            serde_json::from_str::<Control>(&t)
                        {
                            let _ = master.resize(PtySize {
                                rows,
                                cols,
                                pixel_width: 0,
                                pixel_height: 0,
                            });
                        } else if writer.write_all(t.as_bytes()).is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Binary(b))) => {
                        if writer.write_all(&b).is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Ping(data))) => {
                        if socket.send(Message::Pong(data)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    // Pong / other frames: ignore.
                    Some(Ok(_)) => {}
                    Some(Err(_)) => break,
                }
            }
        }
    }

    // Tear down: kill the child, drop the writer/master, and reap the child.
    //
    // We deliberately do NOT join the reader thread. `child.kill()` only
    // signals the direct shell PID, not the PTY's foreground process group, so
    // a still-running grandchild (vim, a dev server, a nested `claude`) keeps
    // the slave fd open and the master reader never reaches EOF — joining would
    // block this async task (and leak an OS thread) forever. Detaching the
    // handle lets the task return immediately; the orphaned reader thread winds
    // down on its own once the grandchildren exit and EOF finally arrives.
    let _ = child.kill();
    drop(writer);
    drop(master);
    let _ = child.wait();
    drop(reader_handle);
    let _ = socket.send(Message::Close(None)).await;
}

/// Resolve the shell's working directory.
///
/// If `repo_id` is given and that repo has a `local_path` that exists on disk,
/// use it; otherwise fall back to the RepoHub data root (parent of the managed
/// `repos/` directory), defaulting to the root itself if it has no parent.
async fn resolve_cwd(state: &AppState, repo_id: Option<i64>) -> std::path::PathBuf {
    if let Some(id) = repo_id {
        if let Ok(Some((Some(p),))) =
            sqlx::query_as::<_, (Option<String>,)>("SELECT local_path FROM repos WHERE id = ?1")
                .bind(id)
                .fetch_optional(&state.db)
                .await
        {
            let path = std::path::PathBuf::from(p);
            if path.exists() {
                return path;
            }
        }
    }
    // Data root = parent of repos_dir() (i.e. cfg.root).
    state
        .cfg
        .repos_dir()
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| state.cfg.root.clone())
}
