//! Cross-site WebSocket hijacking guard.
//!
//! axum 0.7's [`WebSocketUpgrade`] does NOT validate the `Origin` header, and
//! WebSocket connections are not subject to the browser same-origin policy the
//! way `fetch()` is. Without a check, ANY web page the user visits could open
//! `ws://127.0.0.1:<port>/ws/terminal` and run arbitrary shell commands. We
//! therefore reject any upgrade whose `Origin` header is present but not a known
//! local origin before handing the socket to a feature handler.
//!
//! A *missing* `Origin` is allowed: non-browser clients (CLI tools, tests) do
//! not send one, and they are not the cross-origin threat this guards against —
//! the attack vector is specifically a browser on a malicious page, which always
//! attaches an `Origin`.

use axum::http::{HeaderMap, StatusCode};

use crate::config::Config;

/// Returns `Ok(())` when the request's `Origin` is absent or matches an allowed
/// local origin for this server; otherwise `Err(StatusCode::FORBIDDEN)`.
///
/// Allowed origins are `http(s)://127.0.0.1`, `http(s)://localhost` and
/// `http(s)://[::1]`, on either the configured API `port` or the Vite dev
/// server port (5173), with or without an explicit port.
pub fn check_origin(headers: &HeaderMap, cfg: &Config) -> Result<(), StatusCode> {
    let origin = match headers.get(axum::http::header::ORIGIN) {
        // No Origin header (non-browser client) — not the threat we guard.
        None => return Ok(()),
        Some(v) => match v.to_str() {
            Ok(s) => s,
            // A non-UTF-8 Origin is never a legitimate local browser origin.
            Err(_) => return Err(StatusCode::FORBIDDEN),
        },
    };

    if is_allowed_origin(origin, cfg.port) {
        Ok(())
    } else {
        tracing::warn!(%origin, "rejecting WebSocket upgrade from disallowed origin");
        Err(StatusCode::FORBIDDEN)
    }
}

/// True when `origin` is a local origin (loopback host) on the API port, the
/// Vite dev port, or no explicit port. Hosts other than loopback are rejected.
fn is_allowed_origin(origin: &str, api_port: u16) -> bool {
    // Strip the scheme; only http/https are valid browser ws upgrade origins.
    let rest = match origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"))
    {
        Some(r) => r,
        None => return false,
    };

    // An Origin has no path; split host[:port] only.
    let authority = rest.split('/').next().unwrap_or(rest);
    let (host, port) = split_host_port(authority);

    let host_ok = matches!(host, "127.0.0.1" | "localhost" | "[::1]" | "::1");
    if !host_ok {
        return false;
    }

    match port {
        // No explicit port (default 80/443) — accept on a loopback host.
        None => true,
        Some(p) => p == api_port || p == 5173,
    }
}

/// Split `host:port` into (host, Some(port)); IPv6 literals are bracketed
/// (`[::1]:5173`). Returns `(authority, None)` when no parseable port follows.
fn split_host_port(authority: &str) -> (&str, Option<u16>) {
    if let Some(end) = authority.strip_prefix('[') {
        // IPv6 literal: `[host]` or `[host]:port`.
        if let Some(idx) = end.find(']') {
            let host = &authority[..idx + 2]; // include the brackets
            let after = &end[idx + 1..];
            let port = after
                .strip_prefix(':')
                .and_then(|p| p.parse::<u16>().ok());
            return (host, port);
        }
        return (authority, None);
    }
    match authority.rsplit_once(':') {
        Some((host, port)) => match port.parse::<u16>() {
            Ok(p) => (host, Some(p)),
            Err(_) => (authority, None),
        },
        None => (authority, None),
    }
}
