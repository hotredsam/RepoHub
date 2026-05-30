import { useEffect, useRef } from "react";
import { Terminal as XTerm } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { wsUrl } from "../lib/ws";
import "@xterm/xterm/css/xterm.css";

interface TerminalProps {
  repoId: number | null;
}

/**
 * PTY-backed terminal scoped to a repo. Opens a WebSocket to /ws/terminal,
 * sends an initial {repo_id, cols, rows} frame, then pipes keystrokes to the
 * backend and backend output to the screen. Re-mounts (and reconnects) whenever
 * `repoId` changes.
 */
export default function Terminal({ repoId }: TerminalProps) {
  const containerRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;

    const term = new XTerm({
      cursorBlink: true,
      fontFamily:
        'ui-monospace, SFMono-Regular, "SF Mono", Menlo, Consolas, monospace',
      fontSize: 13,
      theme: {
        background: "#0b1120",
        foreground: "#e2e8f0",
        cursor: "#38bdf8",
      },
    });
    const fitAddon = new FitAddon();
    term.loadAddon(fitAddon);
    term.open(container);

    const safeFit = () => {
      try {
        fitAddon.fit();
      } catch {
        // container not laid out yet; ignore
      }
    };
    safeFit();

    const sock = new WebSocket(wsUrl("/ws/terminal"));
    sock.binaryType = "arraybuffer";
    let open = false;

    const sendResize = () => {
      if (sock.readyState !== WebSocket.OPEN) return;
      sock.send(
        JSON.stringify({ type: "resize", cols: term.cols, rows: term.rows }),
      );
    };

    sock.onopen = () => {
      open = true;
      safeFit();
      sock.send(
        JSON.stringify({ repo_id: repoId, cols: term.cols, rows: term.rows }),
      );
      term.focus();
    };

    sock.onmessage = (ev) => {
      const data = ev.data;
      if (typeof data === "string") {
        term.write(data);
      } else if (data instanceof ArrayBuffer) {
        term.write(new Uint8Array(data));
      } else if (data instanceof Blob) {
        data.arrayBuffer().then((buf) => term.write(new Uint8Array(buf)));
      }
    };

    sock.onclose = () => {
      open = false;
      term.write("\r\n\x1b[2m[terminal disconnected]\x1b[0m\r\n");
    };
    sock.onerror = () => {
      term.write("\r\n\x1b[31m[terminal connection error]\x1b[0m\r\n");
    };

    const dataSub = term.onData((chunk) => {
      if (open && sock.readyState === WebSocket.OPEN) sock.send(chunk);
    });

    const onResize = () => {
      safeFit();
      sendResize();
    };
    window.addEventListener("resize", onResize);

    const resizeObserver =
      typeof ResizeObserver !== "undefined"
        ? new ResizeObserver(() => onResize())
        : null;
    resizeObserver?.observe(container);

    return () => {
      window.removeEventListener("resize", onResize);
      resizeObserver?.disconnect();
      dataSub.dispose();
      open = false;
      try {
        sock.close();
      } catch {
        // already closed
      }
      term.dispose();
    };
  }, [repoId]);

  return (
    <div className="flex h-full flex-col rounded-xl border border-edge bg-panel">
      <div className="border-b border-edge px-4 py-2 text-sm font-semibold text-slate-200">
        Embedded terminal
      </div>
      {repoId == null ? (
        <div className="flex flex-1 items-center justify-center p-4 text-xs text-slate-500">
          Select a repo to open a terminal.
        </div>
      ) : (
        <div ref={containerRef} className="h-80 w-full overflow-hidden p-2" />
      )}
    </div>
  );
}
