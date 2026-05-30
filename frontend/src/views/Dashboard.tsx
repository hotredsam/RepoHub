import { useCallback, useEffect, useMemo, useState } from "react";
import type { Repo } from "../lib/types";
import {
  listRepos,
  refreshRepos,
  pullAllRepos,
  trackRepo,
  cloneRepo,
  pullRepo,
  deleteRepos,
} from "../lib/api";

// ---- helpers ----

function timeAgo(iso: string | null): string {
  if (!iso) return "—";
  const t = Date.parse(iso);
  if (Number.isNaN(t)) return "—";
  const secs = Math.max(0, Math.floor((Date.now() - t) / 1000));
  const units: [number, string][] = [
    [60, "s"],
    [60, "m"],
    [24, "h"],
    [7, "d"],
    [4.345, "w"],
    [12, "mo"],
    [Number.POSITIVE_INFINITY, "y"],
  ];
  let v = secs;
  let label = "s";
  for (const [div, l] of units) {
    if (v < div) {
      label = l;
      break;
    }
    v = Math.floor(v / div);
    label = l;
  }
  return `${v}${label} ago`;
}

function fmtDisk(kb: number): string {
  if (!kb) return "0 KB";
  if (kb < 1024) return `${kb} KB`;
  const mb = kb / 1024;
  if (mb < 1024) return `${mb.toFixed(1)} MB`;
  return `${(mb / 1024).toFixed(2)} GB`;
}

function Badge({
  children,
  tone = "default",
}: {
  children: React.ReactNode;
  tone?: "default" | "ahead" | "behind" | "dirty" | "accent";
}) {
  const tones: Record<string, string> = {
    default: "bg-edge text-slate-300",
    ahead: "bg-emerald-500/15 text-emerald-300",
    behind: "bg-amber-500/15 text-amber-300",
    dirty: "bg-rose-500/15 text-rose-300",
    accent: "bg-accent/15 text-accent",
  };
  return (
    <span
      className={`inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-[11px] font-medium ${tones[tone]}`}
    >
      {children}
    </span>
  );
}

// ---- component ----

export default function Dashboard() {
  const [repos, setRepos] = useState<Repo[] | null>(null);
  const [loadErr, setLoadErr] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  const [refreshing, setRefreshing] = useState(false);
  const [pullingAll, setPullingAll] = useState(false);
  const [busy, setBusy] = useState<Record<number, string>>({});
  const [actionErr, setActionErr] = useState<string | null>(null);

  const [selected, setSelected] = useState<Set<number>>(new Set());
  const [showDelete, setShowDelete] = useState(false);

  const load = useCallback(async () => {
    setLoading(true);
    setLoadErr(null);
    try {
      const data = await listRepos();
      setRepos(data);
    } catch (e) {
      setLoadErr(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const setRepoBusy = (id: number, label: string | null) =>
    setBusy((prev) => {
      const next = { ...prev };
      if (label) next[id] = label;
      else delete next[id];
      return next;
    });

  // patch a single repo in state (returned by mutating endpoints)
  const patchRepo = (r: Repo) =>
    setRepos((prev) =>
      prev ? prev.map((x) => (x.id === r.id ? r : x)) : prev,
    );

  const handleRefresh = async () => {
    setRefreshing(true);
    setActionErr(null);
    try {
      const data = await refreshRepos();
      setRepos(data);
    } catch (e) {
      setActionErr(e instanceof Error ? e.message : String(e));
    } finally {
      setRefreshing(false);
    }
  };

  const handlePullAll = async () => {
    setPullingAll(true);
    setActionErr(null);
    try {
      const data = await pullAllRepos();
      setRepos(data);
    } catch (e) {
      setActionErr(e instanceof Error ? e.message : String(e));
    } finally {
      setPullingAll(false);
    }
  };

  const runRepoAction = async (
    id: number,
    label: string,
    fn: () => Promise<Repo>,
  ) => {
    setRepoBusy(id, label);
    setActionErr(null);
    try {
      patchRepo(await fn());
    } catch (e) {
      setActionErr(e instanceof Error ? e.message : String(e));
    } finally {
      setRepoBusy(id, null);
    }
  };

  const toggleSelect = (id: number) =>
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });

  const selectedRepos = useMemo(
    () => (repos ?? []).filter((r) => selected.has(r.id)),
    [repos, selected],
  );

  // ---- render states ----

  if (loading && repos === null) {
    return (
      <div className="mx-auto max-w-6xl">
        <Header
          onRefresh={handleRefresh}
          onPullAll={handlePullAll}
          refreshing={refreshing}
          pullingAll={pullingAll}
          disabled
        />
        <div className="mt-10 text-center text-sm text-slate-400">
          Loading repositories…
        </div>
      </div>
    );
  }

  if (loadErr && repos === null) {
    return (
      <div className="mx-auto max-w-6xl">
        <Header
          onRefresh={handleRefresh}
          onPullAll={handlePullAll}
          refreshing={refreshing}
          pullingAll={pullingAll}
          disabled
        />
        <div className="mt-8 rounded-xl border border-rose-500/40 bg-rose-500/5 p-6 text-sm text-rose-300">
          <p className="font-medium">Failed to load repositories.</p>
          <p className="mt-1 text-rose-300/80">{loadErr}</p>
          <button
            onClick={() => void load()}
            className="mt-4 rounded-md border border-edge bg-panel px-3 py-1.5 text-slate-200 hover:bg-edge"
          >
            Retry
          </button>
        </div>
      </div>
    );
  }

  const list = repos ?? [];

  return (
    <div className="mx-auto max-w-6xl">
      <Header
        onRefresh={handleRefresh}
        onPullAll={handlePullAll}
        refreshing={refreshing}
        pullingAll={pullingAll}
      />

      {actionErr && (
        <div className="mt-4 flex items-start justify-between gap-3 rounded-lg border border-rose-500/40 bg-rose-500/5 px-4 py-2 text-sm text-rose-300">
          <span>{actionErr}</span>
          <button
            onClick={() => setActionErr(null)}
            className="text-rose-300/70 hover:text-rose-200"
          >
            ✕
          </button>
        </div>
      )}

      {selected.size > 0 && (
        <div className="mt-4 flex items-center gap-3 rounded-lg border border-accent/30 bg-accent/10 px-4 py-2 text-sm">
          <span className="text-accent">{selected.size} selected</span>
          <button
            onClick={() => setSelected(new Set())}
            className="text-slate-400 hover:text-slate-200"
          >
            Clear
          </button>
          <button
            onClick={() => setShowDelete(true)}
            className="ml-auto rounded-md bg-rose-500/20 px-3 py-1.5 text-rose-200 hover:bg-rose-500/30"
          >
            Delete selected
          </button>
        </div>
      )}

      {list.length === 0 ? (
        <div className="mt-8 rounded-xl border border-edge bg-panel p-10 text-center">
          <p className="text-slate-300">No repositories yet.</p>
          <p className="mt-1 text-sm text-slate-400">
            Pull your GitHub repos with “Refresh from GitHub”.
          </p>
          <button
            onClick={handleRefresh}
            disabled={refreshing}
            className="mt-4 rounded-md bg-accent/20 px-4 py-2 text-sm text-accent hover:bg-accent/30 disabled:opacity-50"
          >
            {refreshing ? "Refreshing…" : "Refresh from GitHub"}
          </button>
        </div>
      ) : (
        <div className="mt-5 grid grid-cols-1 gap-4 sm:grid-cols-2 lg:grid-cols-3">
          {list.map((r) => (
            <RepoCard
              key={r.id}
              repo={r}
              selected={selected.has(r.id)}
              onToggleSelect={() => toggleSelect(r.id)}
              busy={busy[r.id] ?? null}
              onTrack={(tracked) =>
                runRepoAction(r.id, tracked ? "Tracking…" : "Untracking…", () =>
                  trackRepo(r.id, tracked),
                )
              }
              onClone={() =>
                runRepoAction(r.id, "Cloning…", () => cloneRepo(r.id))
              }
              onPull={() => runRepoAction(r.id, "Pulling…", () => pullRepo(r.id))}
            />
          ))}
        </div>
      )}

      {showDelete && (
        <DeleteModal
          repos={selectedRepos}
          onClose={() => setShowDelete(false)}
          onDeleted={(ids) => {
            setRepos((prev) =>
              prev ? prev.filter((x) => !ids.includes(x.id)) : prev,
            );
            setSelected(new Set());
            setShowDelete(false);
          }}
          onError={(m) => setActionErr(m)}
        />
      )}
    </div>
  );
}

// ---- header ----

function Header({
  onRefresh,
  onPullAll,
  refreshing,
  pullingAll,
  disabled,
}: {
  onRefresh: () => void;
  onPullAll: () => void;
  refreshing: boolean;
  pullingAll: boolean;
  disabled?: boolean;
}) {
  return (
    <div className="flex flex-wrap items-center gap-3">
      <div>
        <h1 className="text-xl font-semibold tracking-tight">Dashboard</h1>
        <p className="text-sm text-slate-400">
          All your GitHub repos, status and managed clones.
        </p>
      </div>
      <div className="ml-auto flex gap-2">
        <button
          onClick={onRefresh}
          disabled={disabled || refreshing}
          className="rounded-md border border-edge bg-panel px-3 py-1.5 text-sm text-slate-200 hover:bg-edge disabled:opacity-50"
        >
          {refreshing ? "Refreshing…" : "Refresh from GitHub"}
        </button>
        <button
          onClick={onPullAll}
          disabled={disabled || pullingAll}
          className="rounded-md border border-edge bg-panel px-3 py-1.5 text-sm text-slate-200 hover:bg-edge disabled:opacity-50"
        >
          {pullingAll ? "Pulling…" : "Pull all"}
        </button>
      </div>
    </div>
  );
}

// ---- repo card ----

function RepoCard({
  repo,
  selected,
  onToggleSelect,
  busy,
  onTrack,
  onClone,
  onPull,
}: {
  repo: Repo;
  selected: boolean;
  onToggleSelect: () => void;
  busy: string | null;
  onTrack: (tracked: boolean) => void;
  onClone: () => void;
  onPull: () => void;
}) {
  const cloned = repo.clone_status === "cloned" || !!repo.local_path;

  return (
    <div
      className={`flex flex-col rounded-xl border bg-panel p-4 transition ${
        selected ? "border-accent/60" : "border-edge"
      }`}
    >
      <div className="flex items-start gap-2">
        <input
          type="checkbox"
          checked={selected}
          onChange={onToggleSelect}
          className="mt-1 h-4 w-4 accent-current text-accent"
          aria-label={`Select ${repo.full_name}`}
        />
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2">
            <span className="truncate font-medium text-slate-100" title={repo.full_name}>
              {repo.name}
            </span>
            {repo.private && <Badge>private</Badge>}
          </div>
          <p className="truncate text-xs text-slate-500" title={repo.full_name}>
            {repo.owner}
          </p>
        </div>
      </div>

      <p className="mt-2 line-clamp-2 min-h-[2.5rem] text-sm text-slate-400">
        {repo.description || "No description."}
      </p>

      <div className="mt-3 flex flex-wrap gap-1.5">
        {repo.language && <Badge tone="accent">{repo.language}</Badge>}
        {repo.tracked && <Badge tone="default">tracked</Badge>}
        {repo.ahead > 0 && <Badge tone="ahead">↑{repo.ahead}</Badge>}
        {repo.behind > 0 && <Badge tone="behind">↓{repo.behind}</Badge>}
        {repo.dirty && <Badge tone="dirty">dirty</Badge>}
        {!cloned && <Badge tone="default">{repo.clone_status || "not cloned"}</Badge>}
      </div>

      <div className="mt-3 flex items-center justify-between text-[11px] text-slate-500">
        <span title={repo.last_commit_at ?? undefined}>
          commit {timeAgo(repo.last_commit_at)}
        </span>
        <span>{fmtDisk(repo.disk_kb)}</span>
      </div>

      <div className="mt-3 flex gap-2 border-t border-edge pt-3">
        {busy ? (
          <span className="text-xs text-slate-400">{busy}</span>
        ) : (
          <>
            {!repo.tracked ? (
              <button
                onClick={() => onTrack(true)}
                className="rounded-md bg-accent/20 px-2.5 py-1 text-xs text-accent hover:bg-accent/30"
              >
                Track
              </button>
            ) : (
              <button
                onClick={() => onTrack(false)}
                className="rounded-md border border-edge px-2.5 py-1 text-xs text-slate-300 hover:bg-edge"
              >
                Untrack
              </button>
            )}
            {repo.tracked && !cloned && (
              <button
                onClick={onClone}
                className="rounded-md border border-edge px-2.5 py-1 text-xs text-slate-300 hover:bg-edge"
              >
                Clone
              </button>
            )}
            {cloned && (
              <button
                onClick={onPull}
                className="rounded-md border border-edge px-2.5 py-1 text-xs text-slate-300 hover:bg-edge"
              >
                Pull
              </button>
            )}
            <a
              href={`https://github.com/${repo.full_name}`}
              target="_blank"
              rel="noreferrer"
              className="ml-auto self-center text-xs text-slate-500 hover:text-slate-300"
            >
              GitHub ↗
            </a>
          </>
        )}
      </div>
    </div>
  );
}

// ---- delete modal ----

function DeleteModal({
  repos,
  onClose,
  onDeleted,
  onError,
}: {
  repos: Repo[];
  onClose: () => void;
  onDeleted: (ids: number[]) => void;
  onError: (msg: string) => void;
}) {
  const [deleteLocal, setDeleteLocal] = useState(true);
  const [deleteRemote, setDeleteRemote] = useState(false);
  const [confirm, setConfirm] = useState("");
  const [working, setWorking] = useState(false);

  const ids = repos.map((r) => r.id);
  const needConfirm = deleteRemote ? "DELETE" : "delete";
  const confirmed = confirm.trim() === needConfirm;

  const handleDelete = async () => {
    setWorking(true);
    try {
      await deleteRepos({
        ids,
        delete_local: deleteLocal,
        delete_remote: deleteRemote,
      });
      onDeleted(ids);
    } catch (e) {
      onError(e instanceof Error ? e.message : String(e));
      setWorking(false);
      onClose();
    }
  };

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-4"
      onClick={onClose}
    >
      <div
        className="w-full max-w-md rounded-xl border border-edge bg-panel p-5 shadow-xl"
        onClick={(e) => e.stopPropagation()}
      >
        <h2 className="text-lg font-semibold text-slate-100">
          Delete {repos.length} repositor{repos.length === 1 ? "y" : "ies"}
        </h2>

        <ul className="mt-3 max-h-32 overflow-auto rounded-md border border-edge bg-black/20 p-2 text-xs text-slate-400">
          {repos.map((r) => (
            <li key={r.id} className="truncate">
              {r.full_name}
            </li>
          ))}
        </ul>

        <div className="mt-4 space-y-2 text-sm text-slate-300">
          <label className="flex items-center gap-2">
            <input
              type="checkbox"
              checked={deleteLocal}
              onChange={(e) => setDeleteLocal(e.target.checked)}
              className="h-4 w-4 text-accent"
            />
            Delete local clone(s) from ~/RepoHub/repos
          </label>
          <label className="flex items-center gap-2">
            <input
              type="checkbox"
              checked={deleteRemote}
              onChange={(e) => setDeleteRemote(e.target.checked)}
              className="h-4 w-4 text-accent"
            />
            <span className={deleteRemote ? "text-rose-300" : ""}>
              Delete remote on GitHub (irreversible)
            </span>
          </label>
        </div>

        {deleteRemote && (
          <p className="mt-3 rounded-md border border-rose-500/40 bg-rose-500/5 px-3 py-2 text-xs text-rose-300">
            This permanently deletes the repos on GitHub via{" "}
            <code>gh repo delete</code>. This cannot be undone.
          </p>
        )}

        <div className="mt-4">
          <label className="text-xs text-slate-400">
            Type <span className="font-mono text-slate-200">{needConfirm}</span>{" "}
            to confirm:
          </label>
          <input
            value={confirm}
            onChange={(e) => setConfirm(e.target.value)}
            className="mt-1 w-full rounded-md border border-edge bg-black/30 px-3 py-1.5 text-sm text-slate-100 outline-none focus:border-accent/60"
            autoFocus
          />
        </div>

        <div className="mt-5 flex justify-end gap-2">
          <button
            onClick={onClose}
            disabled={working}
            className="rounded-md border border-edge px-3 py-1.5 text-sm text-slate-300 hover:bg-edge disabled:opacity-50"
          >
            Cancel
          </button>
          <button
            onClick={handleDelete}
            disabled={!confirmed || working || ids.length === 0}
            className="rounded-md bg-rose-500/80 px-3 py-1.5 text-sm font-medium text-white hover:bg-rose-500 disabled:opacity-40"
          >
            {working ? "Deleting…" : "Delete"}
          </button>
        </div>
      </div>
    </div>
  );
}
