import { useCallback, useEffect, useMemo, useState } from "react";
import {
  searchPrompts,
  promptAnalytics,
  reindexTranscripts,
  transcriptStatus,
  listRepos,
} from "../lib/api";
import type {
  Prompt,
  PromptAnalytics,
  TranscriptStatus,
  Repo,
} from "../lib/types";

const DEFAULT_LIMIT = 100;

function fmtDate(iso: string | null): string {
  if (!iso) return "—";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  return d.toLocaleString();
}

function truncate(s: string | null, n: number): string {
  if (!s) return "";
  const t = s.replace(/\s+/g, " ").trim();
  return t.length > n ? t.slice(0, n) + "…" : t;
}

export default function Prompts() {
  const [search, setSearch] = useState("");
  const [query, setQuery] = useState(""); // committed search term

  const [rows, setRows] = useState<Prompt[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const [analytics, setAnalytics] = useState<PromptAnalytics | null>(null);
  const [status, setStatus] = useState<TranscriptStatus | null>(null);
  const [repos, setRepos] = useState<Repo[]>([]);

  const [reindexing, setReindexing] = useState(false);
  const [reindexMsg, setReindexMsg] = useState<string | null>(null);

  const [expanded, setExpanded] = useState<number | null>(null);

  const repoName = useMemo(() => {
    const m = new Map<number, string>();
    for (const r of repos) m.set(r.id, r.full_name);
    return m;
  }, [repos]);

  const loadResults = useCallback(async (term: string) => {
    setLoading(true);
    setError(null);
    try {
      const data = await searchPrompts({
        search: term || undefined,
        limit: DEFAULT_LIMIT,
      });
      setRows(data);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      setRows([]);
    } finally {
      setLoading(false);
    }
  }, []);

  const loadSidecar = useCallback(async () => {
    try {
      const [a, s, rs] = await Promise.all([
        promptAnalytics(),
        transcriptStatus(),
        listRepos(),
      ]);
      setAnalytics(a);
      setStatus(s);
      setRepos(rs);
    } catch {
      // analytics/status are non-critical; ignore failures here
    }
  }, []);

  useEffect(() => {
    void loadResults("");
    void loadSidecar();
  }, [loadResults, loadSidecar]);

  const submitSearch = (e: React.FormEvent) => {
    e.preventDefault();
    setQuery(search);
    void loadResults(search);
  };

  const clearSearch = () => {
    setSearch("");
    setQuery("");
    void loadResults("");
  };

  const doReindex = async () => {
    setReindexing(true);
    setReindexMsg(null);
    try {
      const res = await reindexTranscripts();
      setReindexMsg(`Indexed ${res.indexed} prompt${res.indexed === 1 ? "" : "s"}.`);
      await Promise.all([loadResults(query), loadSidecar()]);
    } catch (e) {
      setReindexMsg(
        `Reindex failed: ${e instanceof Error ? e.message : String(e)}`,
      );
    } finally {
      setReindexing(false);
    }
  };

  return (
    <div className="mx-auto flex max-w-6xl flex-col gap-5">
      <div className="flex flex-wrap items-center gap-3">
        <h1 className="text-xl font-semibold">Prompts</h1>
        <span className="text-sm text-slate-500">
          Search your Claude prompt &amp; response history.
        </span>
        <div className="ml-auto flex items-center gap-3">
          {reindexMsg && (
            <span className="text-xs text-slate-400">{reindexMsg}</span>
          )}
          <button
            onClick={doReindex}
            disabled={reindexing}
            className="rounded-md border border-edge bg-panel px-3 py-1.5 text-sm text-slate-200 transition hover:border-accent/60 hover:text-accent disabled:cursor-not-allowed disabled:opacity-50"
          >
            {reindexing ? "Reindexing…" : "Reindex transcripts"}
          </button>
        </div>
      </div>

      <AnalyticsSummary analytics={analytics} status={status} />

      <form onSubmit={submitSearch} className="flex items-center gap-2">
        <input
          type="text"
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          placeholder="Search prompts and responses…"
          className="flex-1 rounded-md border border-edge bg-panel px-3 py-2 text-sm text-slate-200 placeholder:text-slate-500 focus:border-accent/60 focus:outline-none"
        />
        <button
          type="submit"
          className="rounded-md bg-accent/20 px-4 py-2 text-sm text-accent transition hover:bg-accent/30"
        >
          Search
        </button>
        {query && (
          <button
            type="button"
            onClick={clearSearch}
            className="rounded-md border border-edge px-3 py-2 text-sm text-slate-400 transition hover:text-slate-200"
          >
            Clear
          </button>
        )}
      </form>

      <div className="rounded-xl border border-edge bg-panel">
        <div className="flex items-center justify-between border-b border-edge px-4 py-2.5">
          <span className="text-sm font-medium text-slate-200">
            Results
            {!loading && (
              <span className="ml-2 text-xs text-slate-500">
                {rows.length}
                {rows.length >= DEFAULT_LIMIT ? "+" : ""} shown
                {query ? ` for “${query}”` : ""}
              </span>
            )}
          </span>
        </div>

        {error ? (
          <div className="px-4 py-10 text-center text-sm text-rose-400">
            {error}
            <div className="mt-3">
              <button
                onClick={() => loadResults(query)}
                className="rounded-md border border-edge px-3 py-1.5 text-xs text-slate-300 hover:text-slate-100"
              >
                Retry
              </button>
            </div>
          </div>
        ) : loading ? (
          <div className="px-4 py-10 text-center text-sm text-slate-500">
            Loading prompts…
          </div>
        ) : rows.length === 0 ? (
          <div className="px-4 py-10 text-center text-sm text-slate-500">
            {query
              ? "No prompts match your search."
              : "No prompts recorded yet. Try chatting with a repo, or reindex your transcripts."}
          </div>
        ) : (
          <div className="overflow-x-auto">
            <table className="w-full border-collapse text-sm">
              <thead>
                <tr className="text-left text-xs uppercase tracking-wide text-slate-500">
                  <th className="px-4 py-2 font-medium">When</th>
                  <th className="px-4 py-2 font-medium">Repo</th>
                  <th className="px-4 py-2 font-medium">Source</th>
                  <th className="px-4 py-2 font-medium">Prompt</th>
                  <th className="px-4 py-2 text-right font-medium">Tokens</th>
                </tr>
              </thead>
              <tbody>
                {rows.map((p) => {
                  const isOpen = expanded === p.id;
                  return (
                    <PromptRow
                      key={p.id}
                      p={p}
                      isOpen={isOpen}
                      repoName={
                        p.repo_id != null
                          ? repoName.get(p.repo_id) ?? `#${p.repo_id}`
                          : null
                      }
                      onToggle={() =>
                        setExpanded((cur) => (cur === p.id ? null : p.id))
                      }
                    />
                  );
                })}
              </tbody>
            </table>
          </div>
        )}
      </div>
    </div>
  );
}

function PromptRow({
  p,
  isOpen,
  repoName,
  onToggle,
}: {
  p: Prompt;
  isOpen: boolean;
  repoName: string | null;
  onToggle: () => void;
}) {
  return (
    <>
      <tr
        onClick={onToggle}
        className="cursor-pointer border-t border-edge align-top transition hover:bg-edge/40"
      >
        <td className="whitespace-nowrap px-4 py-2.5 text-xs text-slate-400">
          {fmtDate(p.created_at)}
        </td>
        <td className="whitespace-nowrap px-4 py-2.5 text-xs text-slate-300">
          {repoName ?? (
            <span className="text-slate-500">app</span>
          )}
        </td>
        <td className="whitespace-nowrap px-4 py-2.5">
          <span className="rounded bg-edge px-2 py-0.5 text-[11px] text-slate-300">
            {p.source}
          </span>
        </td>
        <td className="px-4 py-2.5 text-slate-200">
          {truncate(p.prompt, 120)}
        </td>
        <td className="whitespace-nowrap px-4 py-2.5 text-right text-xs text-slate-400">
          {p.tokens_in}/{p.tokens_out}
        </td>
      </tr>
      {isOpen && (
        <tr className="border-t border-edge bg-bg/40">
          <td colSpan={5} className="px-4 py-3">
            <div className="flex flex-col gap-3">
              <Section label="Prompt" body={p.prompt} />
              <Section
                label="Response"
                body={p.response ?? "(no response recorded)"}
              />
              <div className="flex flex-wrap gap-4 text-[11px] text-slate-500">
                <span>scope: {p.scope}</span>
                {p.model && <span>model: {p.model}</span>}
                <span>tokens in: {p.tokens_in}</span>
                <span>tokens out: {p.tokens_out}</span>
                <span>id: {p.id}</span>
              </div>
            </div>
          </td>
        </tr>
      )}
    </>
  );
}

function Section({ label, body }: { label: string; body: string }) {
  return (
    <div>
      <div className="mb-1 text-[11px] uppercase tracking-wide text-slate-500">
        {label}
      </div>
      <pre className="max-h-64 overflow-auto whitespace-pre-wrap break-words rounded-md border border-edge bg-panel p-3 text-xs text-slate-200">
        {body}
      </pre>
    </div>
  );
}

function AnalyticsSummary({
  analytics,
  status,
}: {
  analytics: PromptAnalytics | null;
  status: TranscriptStatus | null;
}) {
  const totalPrompts = analytics
    ? analytics.per_repo.reduce((acc, r) => acc + r.count, 0)
    : null;

  return (
    <div className="grid grid-cols-2 gap-3 sm:grid-cols-4">
      <Stat
        label="Total prompts"
        value={totalPrompts != null ? totalPrompts.toLocaleString() : "—"}
      />
      <Stat
        label="Tokens in"
        value={
          analytics ? analytics.total_tokens_in.toLocaleString() : "—"
        }
      />
      <Stat
        label="Tokens out"
        value={
          analytics ? analytics.total_tokens_out.toLocaleString() : "—"
        }
      />
      <Stat
        label="Indexed transcripts"
        value={status ? status.indexed.toLocaleString() : "—"}
        sub={
          status?.last_reindex
            ? `last ${fmtDate(status.last_reindex)}`
            : undefined
        }
      />
    </div>
  );
}

function Stat({
  label,
  value,
  sub,
}: {
  label: string;
  value: string;
  sub?: string;
}) {
  return (
    <div className="rounded-xl border border-edge bg-panel px-4 py-3">
      <div className="text-[11px] uppercase tracking-wide text-slate-500">
        {label}
      </div>
      <div className="mt-1 text-lg font-semibold text-slate-100">{value}</div>
      {sub && <div className="mt-0.5 text-[11px] text-slate-500">{sub}</div>}
    </div>
  );
}
