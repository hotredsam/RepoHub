import { useEffect, useRef, useState } from "react";
import type { Repo } from "../lib/types";
import { listRepos } from "../lib/api";
import { openClaudeStream } from "../lib/ws";

interface Msg {
  role: "user" | "assistant" | "system";
  text: string;
}

export default function RepoDetail() {
  const [repos, setRepos] = useState<Repo[]>([]);
  const [repoId, setRepoId] = useState<number | null>(null);
  const [messages, setMessages] = useState<Msg[]>([]);
  const [prompt, setPrompt] = useState("");
  const [busy, setBusy] = useState(false);
  const threadRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    listRepos()
      .then((r) => {
        setRepos(r);
        const firstTracked = r.find((x) => x.tracked) ?? r[0];
        if (firstTracked) setRepoId(firstTracked.id);
      })
      .catch(() => setRepos([]));
  }, []);

  useEffect(() => {
    threadRef.current?.scrollTo({ top: threadRef.current.scrollHeight });
  }, [messages]);

  const repo = repos.find((r) => r.id === repoId) ?? null;

  const send = () => {
    const text = prompt.trim();
    if (!text || repoId == null || busy) return;
    setBusy(true);
    setMessages((m) => [...m, { role: "user", text }, { role: "assistant", text: "" }]);
    setPrompt("");
    openClaudeStream(repoId, text, {
      onChunk: (chunk) =>
        setMessages((m) => {
          const next = [...m];
          const last = next[next.length - 1];
          if (last && last.role === "assistant") last.text += chunk;
          return next;
        }),
      onDone: () => setBusy(false),
      onError: (msg) => {
        setMessages((m) => [...m, { role: "system", text: msg }]);
        setBusy(false);
      },
      onClose: () => setBusy(false),
    });
  };

  return (
    <div className="mx-auto max-w-5xl space-y-4">
      <div className="flex items-center gap-3">
        <label className="text-sm text-slate-400">Repo</label>
        <select
          value={repoId ?? ""}
          onChange={(e) => {
            setRepoId(Number(e.target.value));
            setMessages([]);
          }}
          className="rounded-md border border-edge bg-panel px-3 py-1.5 text-sm text-slate-200"
        >
          {repos.map((r) => (
            <option key={r.id} value={r.id}>
              {r.full_name}
            </option>
          ))}
        </select>
      </div>

      {repo && (
        <div className="grid grid-cols-2 gap-4 rounded-xl border border-edge bg-panel p-4 text-sm md:grid-cols-4">
          <Stat label="Branch" value={repo.default_branch} />
          <Stat label="Ahead / Behind" value={`${repo.ahead} / ${repo.behind}`} />
          <Stat label="Dirty" value={repo.dirty ? "yes" : "no"} />
          <Stat label="Language" value={repo.language ?? "—"} />
          <Stat label="Clone" value={repo.clone_status} />
          <Stat label="Tracked" value={repo.tracked ? "yes" : "no"} />
          <Stat label="Last commit" value={repo.last_commit_at ?? "—"} />
          <Stat label="Disk (KB)" value={String(repo.disk_kb)} />
        </div>
      )}

      <div className="grid gap-4 lg:grid-cols-3">
        {/* Claude chat */}
        <div className="lg:col-span-2 flex flex-col rounded-xl border border-edge bg-panel">
          <div className="border-b border-edge px-4 py-2 text-sm font-semibold text-slate-200">
            Claude — {repo?.name ?? "no repo"}
          </div>
          <div ref={threadRef} className="h-80 space-y-2 overflow-auto p-4">
            {messages.length === 0 && (
              <p className="text-sm text-slate-500">
                Ask Claude about this repo. Runs in the repo's working directory.
              </p>
            )}
            {messages.map((m, i) => (
              <div
                key={i}
                className={`whitespace-pre-wrap rounded-lg px-3 py-2 text-sm ${
                  m.role === "user"
                    ? "bg-accent/15 text-slate-100"
                    : m.role === "system"
                      ? "bg-rose-500/10 text-rose-300"
                      : "bg-edge/60 text-slate-200"
                }`}
              >
                {m.text || (m.role === "assistant" ? "…" : "")}
              </div>
            ))}
          </div>
          <div className="border-t border-edge p-3">
            <div className="flex gap-2">
              <input
                value={prompt}
                onChange={(e) => setPrompt(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") send();
                }}
                placeholder="Message Claude…"
                className="flex-1 rounded-md border border-edge bg-base/40 px-3 py-1.5 text-sm text-slate-100 outline-none focus:border-accent"
              />
              <button
                onClick={send}
                disabled={busy || !prompt.trim() || repoId == null}
                className="rounded-md bg-accent/20 px-4 py-1.5 text-sm text-accent disabled:opacity-40"
              >
                {busy ? "…" : "Send"}
              </button>
            </div>
          </div>
        </div>

        {/* Terminal placeholder */}
        <div className="rounded-xl border border-edge bg-panel p-4">
          <div className="text-sm font-semibold text-slate-200">Embedded terminal</div>
          <p className="mt-2 text-xs text-slate-500">
            A real PTY-backed terminal scoped to this repo arrives in a later phase (P5).
          </p>
          <div className="mt-3 h-48 rounded-md border border-edge bg-base/60 p-3 font-mono text-xs text-slate-600">
            $ _
          </div>
        </div>
      </div>
    </div>
  );
}

function Stat({ label, value }: { label: string; value: string }) {
  return (
    <div>
      <div className="text-xs uppercase tracking-wide text-slate-500">{label}</div>
      <div className="mt-0.5 truncate text-slate-200">{value}</div>
    </div>
  );
}
