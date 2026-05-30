// Helpers for opening same-origin WebSocket connections to the backend.
// In dev, Vite proxies "/ws" to the Rust backend; in prod they share an origin.

export function wsUrl(path: string): string {
  const proto = window.location.protocol === "https:" ? "wss" : "ws";
  const p = path.startsWith("/") ? path : `/${path}`;
  return `${proto}://${window.location.host}${p}`;
}

export interface ClaudeStreamHandlers {
  onChunk: (text: string) => void;
  onDone?: () => void;
  onError?: (msg: string) => void;
  onClose?: () => void;
}

/**
 * Open a Claude chat stream over /ws/claude.
 *
 * Sends the initial {repo_id, prompt} frame once the socket opens, then forwards
 * text frames to onChunk. The backend signals completion with a "[[done]]"
 * frame and errors as "error: ...".
 */
export function openClaudeStream(
  repo_id: number | null,
  prompt: string,
  handlers: ClaudeStreamHandlers,
): WebSocket {
  const sock = new WebSocket(wsUrl("/ws/claude"));

  sock.onopen = () => {
    sock.send(JSON.stringify({ repo_id, prompt }));
  };
  sock.onmessage = (ev) => {
    const data = String(ev.data);
    if (data === "[[done]]") {
      handlers.onDone?.();
      return;
    }
    if (data.startsWith("error: ")) {
      handlers.onError?.(data.slice("error: ".length));
      return;
    }
    handlers.onChunk(data);
  };
  sock.onerror = () => handlers.onError?.("websocket error");
  sock.onclose = () => handlers.onClose?.();

  return sock;
}

/**
 * Subscribe to backend status events on /ws/status. Returns the socket; call
 * `.close()` to unsubscribe.
 */
export function openStatusStream(onEvent: (event: unknown) => void): WebSocket {
  const sock = new WebSocket(wsUrl("/ws/status"));
  sock.onmessage = (ev) => {
    try {
      onEvent(JSON.parse(String(ev.data)));
    } catch {
      onEvent(String(ev.data));
    }
  };
  return sock;
}
