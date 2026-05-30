import { useEffect, useMemo, useState } from "react";
import { listRepos } from "../lib/api";
import type { Repo } from "../lib/types";

// Connections.tsx
// Placeholder for the cross-repo integration graph. The full drag-to-integrate
// dependency graph (reactflow) arrives in a later phase; for now we explain the
// concept and list the repos that will become nodes in that graph.
export default function Connections() {
  const [repos, setRepos] = useState<Repo[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [query, setQuery] = useState("");

  async function load() {
    setLoading(true);
    setError(null);
    try {
      const data = await listRepos();
      setRepos(data);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => {
    void load();
  }, []);

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    const sorted = [...repos].sort((a, b) =>
      a.full_name.localeCompare(b.full_name),
    );
    if (!q) return sorted;
    return sorted.filter(
      (r) =>
        r.full_name.toLowerCase().includes(q) ||
        (r.language ?? "").toLowerCase().includes(q) ||
        (r.description ?? "").toLowerCase().includes(q),
    );
  }, [repos, query]);

  const trackedCount = useMemo(
    () => repos.filter((r) => r.tracked).length,
    [repos],
  );

  return (
    <div className="mx-auto flex max-w-5xl flex-col gap-6">
      {/* Intro / concept card */}
      <div className="rounded-xl border border-edge bg-panel p-6">
        <div className="flex items-start justify-between gap-4">
          <div>
            <h1 className="text-xl font-semibold">Connections</h1>
            <p className="mt-2 max-w-2xl text-sm text-slate-400">
              Map how your repositories relate to one another. Soon you will be
              able to <span className="text-accent">drag one repo onto another</span>{" "}
              to declare an integration — a shared library, an API contract, or a
              deployment dependency — and Claude will scaffold the wiring across
              both repos on their{" "}
              <code className="rounded bg-edge px-1 py-0.5 text-xs text-slate-200">
                repohub-staging
              </code>{" "}
              branches.
            </p>
          </div>
          <span className="shrink-0 rounded-full border border-edge bg-edge/40 px-3 py-1 text-xs text-slate-400">
            graph coming soon
          </span>
        </div>

        <div className="mt-5 grid gap-3 sm:grid-cols-3">
          <ConceptStep
            n={1}
            title="Pick a source"
            body="Choose a repo whose code or API another project should consume."
          />
          <ConceptStep
            n={2}
            title="Drag to a target"
            body="Drop it onto a target repo to create a directed connection."
          />
          <ConceptStep
            n={3}
            title="Let Claude wire it"
            body="A bulk staging job scaffolds the integration in both repos."
          />
        </div>
      </div>

      {/* Repo list = future graph nodes */}
      <div className="rounded-xl border border-edge bg-panel p-6">
        <div className="flex flex-wrap items-center justify-between gap-3">
          <div>
            <h2 className="text-base font-semibold">Graph nodes</h2>
            <p className="mt-1 text-xs text-slate-500">
              {loading
                ? "Loading repositories…"
                : `${repos.length} repos · ${trackedCount} tracked`}
            </p>
          </div>
          <div className="flex items-center gap-2">
            <input
              type="text"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder="Filter repos…"
              className="w-48 rounded-md border border-edge bg-edge/30 px-3 py-1.5 text-sm text-slate-200 placeholder:text-slate-500 focus:border-accent focus:outline-none"
            />
            <button
              onClick={() => void load()}
              disabled={loading}
              className="rounded-md border border-edge px-3 py-1.5 text-sm text-slate-300 transition hover:bg-edge hover:text-slate-100 disabled:opacity-50"
            >
              Refresh
            </button>
          </div>
        </div>

        <div className="mt-5">
          {error ? (
            <div className="rounded-lg border border-rose-500/40 bg-rose-500/10 p-4 text-sm text-rose-300">
              <p className="font-medium">Could not load repositories.</p>
              <p className="mt-1 text-rose-300/80">{error}</p>
              <button
                onClick={() => void load()}
                className="mt-3 rounded-md border border-rose-500/40 px-3 py-1 text-xs text-rose-200 transition hover:bg-rose-500/20"
              >
                Try again
              </button>
            </div>
          ) : loading ? (
            <div className="grid gap-2 sm:grid-cols-2">
              {Array.from({ length: 4 }).map((_, i) => (
                <div
                  key={i}
                  className="h-16 animate-pulse rounded-lg border border-edge bg-edge/20"
                />
              ))}
            </div>
          ) : repos.length === 0 ? (
            <div className="rounded-lg border border-dashed border-edge p-8 text-center">
              <p className="text-sm text-slate-300">No repositories yet.</p>
              <p className="mt-1 text-xs text-slate-500">
                Refresh from GitHub on the Dashboard to populate the graph.
              </p>
            </div>
          ) : filtered.length === 0 ? (
            <div className="rounded-lg border border-dashed border-edge p-8 text-center text-sm text-slate-400">
              No repos match “{query}”.
            </div>
          ) : (
            <ul className="grid gap-2 sm:grid-cols-2">
              {filtered.map((r) => (
                <NodeCard key={r.id} repo={r} />
              ))}
            </ul>
          )}
        </div>
      </div>
    </div>
  );
}

function ConceptStep({
  n,
  title,
  body,
}: {
  n: number;
  title: string;
  body: string;
}) {
  return (
    <div className="rounded-lg border border-edge bg-edge/20 p-4">
      <div className="flex items-center gap-2">
        <span className="flex h-6 w-6 items-center justify-center rounded-full bg-accent/20 text-xs font-semibold text-accent">
          {n}
        </span>
        <span className="text-sm font-medium text-slate-200">{title}</span>
      </div>
      <p className="mt-2 text-xs text-slate-400">{body}</p>
    </div>
  );
}

function NodeCard({ repo }: { repo: Repo }) {
  return (
    <li
      className="group cursor-grab rounded-lg border border-edge bg-edge/20 p-3 transition hover:border-accent/50"
      draggable={false}
      title="Drag-to-integrate arrives in a later phase"
    >
      <div className="flex items-center justify-between gap-2">
        <div className="min-w-0">
          <div className="flex items-center gap-2">
            <span className="truncate text-sm font-medium text-slate-200">
              {repo.full_name}
            </span>
            {repo.private && (
              <span className="shrink-0 rounded bg-edge px-1.5 py-0.5 text-[10px] uppercase tracking-wide text-slate-400">
                private
              </span>
            )}
          </div>
          {repo.description && (
            <p className="mt-1 truncate text-xs text-slate-500">
              {repo.description}
            </p>
          )}
        </div>
        <div className="flex shrink-0 flex-col items-end gap-1 text-[11px]">
          {repo.language && (
            <span className="rounded bg-edge px-1.5 py-0.5 text-slate-300">
              {repo.language}
            </span>
          )}
          <span
            className={
              repo.tracked ? "text-emerald-400" : "text-slate-500"
            }
          >
            {repo.tracked ? "tracked" : "untracked"}
          </span>
        </div>
      </div>
    </li>
  );
}
