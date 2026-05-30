import { useCallback, useEffect, useMemo, useState } from "react";

// Merge.tsx — P9 merge model UX.
//
// Lists tracked + cloned repos whose `repohub-staging` branch is ahead of the
// default branch (GET /api/merge/pending), lets the user pick which ones and a
// merge strategy, then OPENS AND IMMEDIATELY MERGES the staged work into each
// repo's DEFAULT branch (POST /api/merge/finish). Per-repo results — PR url,
// merged check, or the error verbatim (e.g. branch protection) — are shown.
//
// The API helpers below are defined locally to keep this change scoped to a
// single file; they follow the same same-origin/error-body conventions as
// src/lib/api.ts.

// ---- response/request shapes (mirror backend/src/merge.rs) ----

interface PendingEntry {
  repo_id: number;
  full_name: string;
  default_branch: string;
  commits_ahead: number;
}

interface FinishResult {
  repo_id: number;
  full_name: string;
  pr_url: string | null;
  merged: boolean;
  error: string | null;
}

interface FinishResponse {
  summary: string;
  note: string;
  results: FinishResult[];
}

type Strategy = "squash" | "merge";

// ---- local same-origin fetch helpers (mirror src/lib/api.ts conventions) ----

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(path, {
    headers: { "Content-Type": "application/json", ...(init?.headers ?? {}) },
    ...init,
  });
  if (!res.ok) {
    let msg = `${res.status} ${res.statusText}`;
    try {
      const body = await res.json();
      if (body && typeof body.error === "string") msg = body.error;
    } catch {
      // ignore non-JSON error bodies
    }
    throw new Error(`${path} failed: ${msg}`);
  }
  if (res.status === 204) return undefined as T;
  const text = await res.text();
  return (text ? JSON.parse(text) : undefined) as T;
}

const getMergePending = () => request<PendingEntry[]>("/api/merge/pending");

const finishMerge = (repo_ids: number[], strategy: Strategy) =>
  request<FinishResponse>("/api/merge/finish", {
    method: "POST",
    body: JSON.stringify({ repo_ids, strategy }),
  });

// ---------------------------------------------------------------------------

export default function Merge() {
  const [pending, setPending] = useState<PendingEntry[] | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const [selected, setSelected] = useState<Set<number>>(new Set());
  const [strategy, setStrategy] = useState<Strategy>("squash");

  const [merging, setMerging] = useState(false);
  const [response, setResponse] = useState<FinishResponse | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);

  const load = useCallback(() => {
    setLoading(true);
    setError(null);
    getMergePending()
      .then((list) => {
        setPending(list);
        // Drop selections that are no longer pending.
        setSelected((prev) => {
          const valid = new Set(list.map((e) => e.repo_id));
          const next = new Set<number>();
          for (const id of prev) if (valid.has(id)) next.add(id);
          return next;
        });
      })
      .catch((e) => setError(e instanceof Error ? e.message : String(e)))
      .finally(() => setLoading(false));
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  const toggleSelect = (id: number) =>
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });

  const allSelected = useMemo(
    () => !!pending && pending.length > 0 && selected.size === pending.length,
    [pending, selected],
  );

  const toggleAll = () =>
    setSelected((prev) => {
      if (!pending) return prev;
      if (prev.size === pending.length) return new Set();
      return new Set(pending.map((e) => e.repo_id));
    });

  const resultsById = useMemo(() => {
    const map = new Map<number, FinishResult>();
    for (const r of response?.results ?? []) map.set(r.repo_id, r);
    return map;
  }, [response]);

  const runMerge = async () => {
    if (selected.size === 0) return;
    setMerging(true);
    setActionError(null);
    setResponse(null);
    try {
      const resp = await finishMerge([...selected], strategy);
      setResponse(resp);
      // Refresh pending so successfully-merged repos drop off the list.
      load();
    } catch (e) {
      setActionError(e instanceof Error ? e.message : String(e));
    } finally {
      setMerging(false);
    }
  };

  return (
    <div className="mx-auto max-w-4xl space-y-6">
      <div>
        <h1 className="text-2xl font-semibold tracking-tight">Merge</h1>
        <p className="mt-1 text-sm text-slate-400">
          Bring staged work from{" "}
          <code className="rounded bg-edge px-1 py-0.5 text-xs text-slate-200">
            repohub-staging
          </code>{" "}
          into each repo's default branch.
        </p>
      </div>

      {/* ---- WARNING banner ---- */}
      <div className="rounded-xl border border-amber-500/40 bg-amber-500/10 p-4">
        <div className="flex items-start gap-3">
          <span className="mt-0.5 text-lg leading-none text-amber-400">⚠</span>
          <div className="text-sm text-amber-200">
            <p className="font-semibold text-amber-300">
              This opens a pull request and immediately merges it.
            </p>
            <p className="mt-1 text-amber-200/90">
              Selected repos have their{" "}
              <code className="rounded bg-amber-500/15 px-1 py-0.5 text-xs">
                repohub-staging
              </code>{" "}
              branch pushed, a PR opened against the default branch (e.g.{" "}
              <span className="font-mono">main</span>), and that PR{" "}
              <span className="font-semibold">merged right away</span>. It does
              not stop at "PR opened." Branch-protection rules or required checks
              can block the merge — those failures are shown verbatim and never
              reported as success.
            </p>
          </div>
        </div>
      </div>

      {/* ---- Controls + list ---- */}
      <section className="rounded-xl border border-edge bg-panel p-6">
        <div className="flex flex-wrap items-center justify-between gap-3">
          <h2 className="text-lg font-semibold">Staged changes</h2>
          <div className="flex items-center gap-3">
            {/* strategy toggle */}
            <div
              role="group"
              aria-label="Merge strategy"
              className="flex overflow-hidden rounded-md border border-edge text-xs"
            >
              {(["squash", "merge"] as Strategy[]).map((s) => (
                <button
                  key={s}
                  type="button"
                  onClick={() => setStrategy(s)}
                  disabled={merging}
                  className={`px-3 py-1.5 transition disabled:opacity-40 ${
                    strategy === s
                      ? "bg-accent/20 text-accent"
                      : "text-slate-400 hover:bg-edge"
                  }`}
                >
                  {s === "squash" ? "Squash" : "Merge commit"}
                  {s === "squash" && (
                    <span className="ml-1 text-[10px] uppercase tracking-wide opacity-60">
                      default
                    </span>
                  )}
                </button>
              ))}
            </div>
            <button
              onClick={runMerge}
              disabled={merging || selected.size === 0}
              className="rounded-md bg-amber-500/20 px-4 py-2 text-sm font-medium text-amber-300 transition hover:bg-amber-500/30 disabled:cursor-not-allowed disabled:opacity-40"
            >
              {merging
                ? "Merging…"
                : `Open + merge selected${
                    selected.size > 0 ? ` (${selected.size})` : ""
                  }`}
            </button>
          </div>
        </div>

        {actionError && (
          <p className="mt-3 rounded-md bg-rose-500/10 px-3 py-2 text-sm text-rose-400">
            {actionError}
          </p>
        )}

        {response && (
          <p className="mt-3 rounded-md bg-emerald-500/10 px-3 py-2 text-sm text-emerald-300">
            {response.summary}
          </p>
        )}

        <div className="mt-5">
          {loading ? (
            <p className="text-sm text-slate-400">Loading staged changes…</p>
          ) : error ? (
            <div className="flex items-center justify-between rounded-md bg-rose-500/10 px-3 py-2 text-sm text-rose-400">
              <span>{error}</span>
              <button
                onClick={load}
                className="rounded border border-rose-500/40 px-2 py-0.5 text-xs hover:bg-rose-500/10"
              >
                Retry
              </button>
            </div>
          ) : !pending || pending.length === 0 ? (
            <p className="rounded-lg border border-dashed border-edge px-4 py-8 text-center text-sm text-slate-500">
              No staged changes to merge.
            </p>
          ) : (
            <>
              <label className="mb-2 flex w-fit cursor-pointer items-center gap-2 text-xs text-slate-400">
                <input
                  type="checkbox"
                  checked={allSelected}
                  onChange={toggleAll}
                  className="h-4 w-4 accent-current text-accent"
                  aria-label="Select all repos"
                />
                Select all ({pending.length})
              </label>
              <ul className="space-y-2">
                {pending.map((entry) => {
                  const result = resultsById.get(entry.repo_id);
                  const isSelected = selected.has(entry.repo_id);
                  return (
                    <li
                      key={entry.repo_id}
                      className={`rounded-lg border p-3 transition ${
                        isSelected ? "border-accent/60" : "border-edge"
                      } bg-slate-900/40`}
                    >
                      <div className="flex flex-wrap items-center gap-3">
                        <input
                          type="checkbox"
                          checked={isSelected}
                          onChange={() => toggleSelect(entry.repo_id)}
                          disabled={merging}
                          className="h-4 w-4 accent-current text-accent disabled:opacity-40"
                          aria-label={`Select ${entry.full_name}`}
                        />
                        <span className="font-medium text-slate-200">
                          {entry.full_name}
                        </span>
                        <span className="rounded bg-edge px-1.5 py-0.5 text-[11px] text-slate-400">
                          → {entry.default_branch}
                        </span>
                        <span className="text-xs text-slate-400">
                          {entry.commits_ahead} commit
                          {entry.commits_ahead === 1 ? "" : "s"} ahead
                        </span>
                        {result && (
                          <span className="ml-auto flex items-center gap-2 text-xs">
                            {result.merged ? (
                              <span className="text-emerald-400">✓ merged</span>
                            ) : (
                              <span className="text-rose-400">✕ not merged</span>
                            )}
                            {result.pr_url && (
                              <a
                                href={result.pr_url}
                                target="_blank"
                                rel="noreferrer"
                                className="text-accent underline-offset-2 hover:underline"
                              >
                                PR
                              </a>
                            )}
                          </span>
                        )}
                      </div>
                      {result?.error && (
                        <pre className="mt-2 max-h-40 overflow-auto whitespace-pre-wrap break-words rounded-md bg-rose-500/10 px-3 py-2 text-xs text-rose-300">
                          {result.error}
                        </pre>
                      )}
                    </li>
                  );
                })}
              </ul>
            </>
          )}
        </div>

        {response?.note && (
          <p className="mt-4 text-xs text-slate-500">{response.note}</p>
        )}
      </section>
    </div>
  );
}
