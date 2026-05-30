//! Status WebSocket feature module.
//!
//! Exposes `GET /ws/status`: clients that connect here are subscribed to the
//! shared [`AppState::status_tx`] broadcast channel and receive every JSON
//! status event (repo status changes, scheduler ticks, bulk-job progress, …)
//! pushed by other parts of the backend, forwarded verbatim as text frames.

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use tokio::sync::broadcast::error::RecvError;

use crate::state::AppState;
use crate::ws_origin::check_origin;

/// Feature router. Merged under the main app by the integrate step.
pub fn router() -> Router<AppState> {
    Router::new().route("/ws/status", get(ws_status))
}

/// Upgrade the HTTP connection to a WebSocket and hand it to [`handle_socket`].
async fn ws_status(
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

/// Per-connection task: relay broadcast events to the client and watch for the
/// client closing the connection (including responding to pings).
async fn handle_socket(mut socket: WebSocket, state: AppState) {
    // Each subscriber gets its own receiver; events broadcast after this point
    // are delivered to this client.
    let mut rx = state.status_tx.subscribe();

    loop {
        tokio::select! {
            // A status event was broadcast — forward it to the client.
            recv = rx.recv() => {
                match recv {
                    Ok(payload) => {
                        if socket.send(Message::Text(payload)).await.is_err() {
                            // Client went away mid-send.
                            break;
                        }
                    }
                    // Slow consumer lagged behind the channel buffer; keep going
                    // and just deliver subsequent events. The client will get a
                    // fresh full picture on the next status broadcast.
                    Err(RecvError::Lagged(_)) => continue,
                    // All senders dropped — nothing more will ever arrive.
                    Err(RecvError::Closed) => break,
                }
            }
            // Read inbound frames so we notice client closes and answer pings.
            inbound = socket.recv() => {
                match inbound {
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(Message::Ping(data))) => {
                        if socket.send(Message::Pong(data)).await.is_err() {
                            break;
                        }
                    }
                    // Ignore any other client-sent frames; this endpoint is
                    // server-push only.
                    Some(Ok(_)) => {}
                    Some(Err(_)) => break,
                }
            }
        }
    }
}
