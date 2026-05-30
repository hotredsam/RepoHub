import { useEffect, useRef, useState } from "react";
import type { Repo, BulkJobWithItems } from "../lib/types";
import { listRepos, createBulkPrompt } from "../lib/api";
import { openClaudeStream } from "../lib/ws";
import { VoiceInput, isSupported as voiceSupported } from "../lib/voice";

interface Msg {
  role: "user" | "assistant" | "system";
  text: string;
}

const GCP_MIGRATION_TEMPLATE = `Migrate this repository to Google Cloud.

- Identify the current hosting/cloud provider and any provider-specific SDKs, config, or infra-as-code.
- Replace them with equivalent Google Cloud services (Cloud Run, Cloud SQL, GCS, Firestore, Pub/Sub, etc.) as appropriate.
- Update environment variables, deployment manifests, and CI to target Google Cloud.
- Add a short MIGRATION.md describing what changed and any manual follow-up steps.
Make the changes on the staging branch; do not touch main.`;

export default function Chat() {
  const [repos, setRepos] = useState<Repo[]>([]);
  const [selected, setSelected] = useState<Set<number>>(new Set());
  const [allRepos, setAllRepos] = useState(true);
  const [prompt, setPrompt] = useState("");
  const [messages, setMessages] = useState<Msg[]>([]);
  const [busy, setBusy] = useState(false);
  const [listening, setListening] = useState(false);
  const [job, setJob] = useState<BulkJobWithItems | null>(null);

  const voiceRef = useRef<VoiceInput | null>(null);
  const baseTextRef = useRef("");
  const threadRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    listRepos().then(setRepos).catch(() => setRepos([]));
  }, []);

  useEffect(() => {
    threadRef.current?.scrollTo({ top: threadRef.current.scrollHeight });
  }, [messages, job]);

  const toggleRepo = (id: number) => {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
    setAllRepos(false);
  };

  const targetIds = (): number[] => {
    if (allRepos) return repos.filter((r) => r.tracked).map((r) => r.id);
    return [...selected];
  };

  const toggleMic = () => {
    if (!voiceSupported()) {
      setMessages((m) => [
        ...m,
        { role: "system", text: "Voice input is not supported in this browser." },
      ]);
      return;
    }
    if (listening) {
      voiceRef.current?.stop();
      setListening(false);
      return;
    }
    baseTextRef.current = prompt ? prompt + " " : "";
    const v = new VoiceInput({
      onResult: (text) => setPrompt(baseTextRef.current + text),
      onError: (msg) =>
        setMessages((m) => [...m, { role: "system", text: `Voice error: ${msg}` }]),
      onEnd: () => setListening(false),
    });
    voiceRef.current = v;
    if (v.start()) setListening(true);
  };

  const runBulk = async () => {
    const ids = targetIds();
    if (ids.length === 0) {
      setMessages((m) => [
        ...m,
        { role: "system", text: "No tracked repos selected for a bulk run." },
      ]);
      return;
    }
    setBusy(true);
    setJob(null);
    setMessages((m) => [...m, { role: "user", text: prompt }]);
    try {
      const result = await createBulkPrompt({
        repo_ids: ids,
        prompt,
        kind: "chat",
      });
      setJob(result);
      setMessages((m) => [
        ...m,
        {
          role: "assistant",
          text: `Bulk job #${result.id} finished: ${result.items.length} repo(s).`,
        },
      ]);
      setPrompt("");
    } catch (e) {
      setMessages((m) => [
        ...m,
        { role: "system", text: e instanceof Error ? e.message : String(e) },
      ]);
    } finally {
      setBusy(false);
    }
  };

  const runChat = () => {
    const text = prompt.trim();
    if (!text) return;
    setBusy(true);
    setMessages((m) => [...m, { role: "user", text }, { role: "assistant", text: "" }]);
    setPrompt("");

    openClaudeStream(null, text, {
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

  const hasTargets = allRepos || selected.size > 0;

  const go = () => {
    if (busy || !prompt.trim()) return;
    if (hasTargets) runBulk();
    else runChat();
  };

  const useGcpPreset = () => {
    setPrompt(GCP_MIGRATION_TEMPLATE);
    if (selected.size === 0) setAllRepos(true);
  };

  return (
    <div className="mx-auto flex h-full max-w-5xl gap-4">
      {/* Targets sidebar */}
      <aside className="hidden w-64 shrink-0 flex-col rounded-xl border border-edge bg-panel p-4 md:flex">
        <h2 className="text-sm font-semibold text-slate-200">Targets</h2>
        <label className="mt-3 flex items-center gap-2 text-sm text-slate-300">
          <input
            type="checkbox"
            checked={allRepos}
            onChange={(e) => {
              setAllRepos(e.target.checked);
              if (e.target.checked) setSelected(new Set());
            }}
          />
          All tracked repos
        </label>
        <div className="mt-3 flex-1 space-y-1 overflow-auto">
          {repos.map((r) => (
            <label
              key={r.id}
              className="flex items-center gap-2 rounded px-1 py-0.5 text-xs text-slate-400 hover:bg-edge"
            >
              <input
                type="checkbox"
                checked={selected.has(r.id)}
                onChange={() => toggleRepo(r.id)}
              />
              <span className="truncate">{r.name}</span>
            </label>
          ))}
        </div>
        <button
          onClick={useGcpPreset}
          className="mt-3 rounded-md border border-edge px-2 py-1.5 text-xs text-accent hover:bg-accent/10"
        >
          Migrate selected to Google Cloud
        </button>
      </aside>

      {/* Thread + composer */}
      <section className="flex flex-1 flex-col">
        <div
          ref={threadRef}
          className="flex-1 space-y-3 overflow-auto rounded-xl border border-edge bg-panel p-4"
        >
          {messages.length === 0 && (
            <p className="text-sm text-slate-500">
              Ask anything. With no targets selected this is a streamed app-level chat;
              pick repos to fan a prompt out across them on a staging branch.
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
          {job && (
            <div className="rounded-lg border border-edge bg-base/40 p-3 text-xs text-slate-300">
              <div className="font-semibold text-slate-200">Job #{job.id} · {job.status}</div>
              <ul className="mt-2 space-y-1">
                {job.items.map((it) => (
                  <li key={it.id} className="flex justify-between gap-2">
                    <span className="truncate">repo {it.repo_id}</span>
                    <span className="text-slate-400">{it.status}{it.error ? `: ${it.error}` : ""}</span>
                  </li>
                ))}
              </ul>
            </div>
          )}
        </div>

        <div className="mt-3 rounded-xl border border-edge bg-panel p-3">
          <textarea
            value={prompt}
            onChange={(e) => setPrompt(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) go();
            }}
            rows={3}
            placeholder="Type a prompt… (Cmd/Ctrl+Enter to send)"
            className="w-full resize-none rounded-md border border-edge bg-base/40 p-2 text-sm text-slate-100 outline-none focus:border-accent"
          />
          <div className="mt-2 flex items-center gap-2">
            <button
              onClick={toggleMic}
              className={`rounded-md px-3 py-1.5 text-sm ${
                listening
                  ? "bg-rose-500/20 text-rose-300"
                  : "border border-edge text-slate-300 hover:bg-edge"
              }`}
              title="Voice to text"
            >
              {listening ? "● Stop" : "🎙 Mic"}
            </button>
            <span className="text-xs text-slate-500">
              {hasTargets ? "Bulk across targets" : "App-level chat"}
            </span>
            <button
              onClick={go}
              disabled={busy || !prompt.trim()}
              className="ml-auto rounded-md bg-accent/20 px-4 py-1.5 text-sm text-accent disabled:opacity-40"
            >
              {busy ? "Running…" : "Go"}
            </button>
          </div>
        </div>
      </section>
    </div>
  );
}
