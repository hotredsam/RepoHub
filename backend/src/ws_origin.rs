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

/// Suffix of this Mac's tailnet hostnames. Remote browsers reach RepoHub via
/// Tailscale serve at `https://<host>.tail97ef37.ts.net`, so those https origins
/// must also pass the WS hijacking guard.
const TAILNET_SUFFIX: &str = ".tail97ef37.ts.net";

/// True when `origin` is a local origin (loopback host) on the API port, the
/// Vite dev port, or no explicit port, OR an https origin on this Mac's tailnet
/// (`*.tail97ef37.ts.net`). Other hosts are rejected.
fn is_allowed_origin(origin: &str, api_port: u16) -> bool {
    // Strip the scheme; only http/https are valid browser ws upgrade origins.
    let is_https = origin.starts_with("https://");
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

    // Tailscale serve fronts the app over https on a tailnet hostname. Accept any
    // `*.tail97ef37.ts.net` host (incl. the named `samuels-mac-mini`) over https,
    // on the default port (443) only.
    if is_https && port.is_none() && host.ends_with(TAILNET_SUFFIX) {
        return true;
    }

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

#[cfg(test)]
mod tests {
    use super::is_allowed_origin;

    const API_PORT: u16 = 8787;

    #[test]
    fn loopback_origins_allowed() {
        assert!(is_allowed_origin("http://127.0.0.1:8787", API_PORT));
        assert!(is_allowed_origin("http://localhost:5173", API_PORT));
        assert!(is_allowed_origin("http://[::1]", API_PORT));
        assert!(is_allowed_origin("http://localhost", API_PORT));
    }

    #[test]
    fn foreign_and_wrong_port_rejected() {
        assert!(!is_allowed_origin("http://evil.com", API_PORT));
        assert!(!is_allowed_origin("http://127.0.0.1:9999", API_PORT));
        assert!(!is_allowed_origin("ftp://127.0.0.1", API_PORT));
    }

    #[test]
    fn tailnet_https_origins_allowed() {
        assert!(is_allowed_origin(
            "https://samuels-mac-mini.tail97ef37.ts.net",
            API_PORT
        ));
        assert!(is_allowed_origin(
            "https://anything.tail97ef37.ts.net",
            API_PORT
        ));
    }

    #[test]
    fn tailnet_over_http_or_other_tailnet_rejected() {
        // Tailscale serve terminates TLS; plain http on the tailnet is not served.
        assert!(!is_allowed_origin(
            "http://samuels-mac-mini.tail97ef37.ts.net",
            API_PORT
        ));
        // A different tailnet suffix must not match.
        assert!(!is_allowed_origin(
            "https://host.tailXXXXXX.ts.net",
            API_PORT
        ));
        // Suffix must be a real subdomain boundary, not a lookalike host.
        assert!(!is_allowed_origin(
            "https://eviltail97ef37.ts.net.attacker.com",
            API_PORT
        ));
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
