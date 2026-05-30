import { useCallback, useEffect, useMemo, useState } from "react";
import { listRepos } from "../lib/api";
import type { Repo } from "../lib/types";

// Consistency.tsx — the styling/consistency engine UI.
//
// Pick a "golden" repo as the source of truth, choose which config bundles to
// stamp (formatting, repo metadata, token/secret hygiene, CLAUDE.md guidance),
// select the target repos, and Apply. The backend opens (or reuses) each
// target's `repohub-staging` branch and commits the changes there — nothing is
// ever merged to a default branch automatically. Review and merge happens in
// the Merge tab.

// ---- API contract (P8 backend) ----
//
// GET  /api/consistency/config            -> ConsistencyConfig
// PUT  /api/consistency/golden  {repo_id} -> ConsistencyConfig
// POST /api/consistency/apply             -> { results: ApplyResult[] }
//        body: { golden_id, target_ids, bundle_keys }

// Matches backend consistency.rs `ConfigResponse` (GET /config, PUT /golden).
interface ConsistencyConfig {
  golden_repo_id: number | null;
}

interface ApplyResult {
  repo_id: number;
  full_name?: string | null;
  ok: boolean;
  branch?: string | null;
  changed_files?: string[] | null;
  message?: string | null;
  error?: string | null;
}

interface ApplyResponse {
  results: ApplyResult[];
}

interface ApplyBody {
  golden_id: number;
  target_ids: number[];
  bundle_keys: string[];
}

// Local same-origin fetch helpers. (api.ts is owned by another unit; these
// mirror its conventions: throw on non-ok, parse JSON, surface body.error.)
async function apiRequest<T>(path: string, init?: RequestInit): Promise<T> {
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

const getConsistencyConfig = () =>
  apiRequest<ConsistencyConfig>("/api/consistency/config");

const putGoldenRepo = (repo_id: number) =>
  apiRequest<ConsistencyConfig>("/api/consistency/golden", {
    method: "PUT",
    body: JSON.stringify({ repo_id }),
  });

const applyConsistency = (body: ApplyBody) =>
  apiRequest<ApplyResponse>("/api/consistency/apply", {
    method: "POST",
    body: JSON.stringify(body),
  });

// ---- Bundle catalogue ----

interface BundleDef {
  key: string;
  title: string;
  description: string;
}

const BUNDLES: BundleDef[] = [
  {
    key: "format",
    title: "Formatting",
    description:
      "Copy the golden repo's editor/formatter config (.editorconfig, rustfmt.toml, .prettierrc, clippy.toml, eslint config) so every repo formats and lints identically.",
  },
  {
    key: "meta",
    title: "Repo metadata",
    description:
      "Sync shared metadata scaffolding — .gitignore, LICENSE, and the README status-table skeleton — to match the golden repo's house style.",
  },
  {
    key: "tokens",
    title: "Token & secret hygiene",
    description:
      "Apply the golden repo's secret-scanning guards: .gitignore secret rules, .env.example, and gitleaks / pre-commit configuration. Never copies actual secrets.",
  },
  {
    key: "claude",
    title: "CLAUDE.md guidance",
    description:
      "Propagate the golden repo's CLAUDE.md (conventions, build order, house rules) so Claude behaves consistently across every repo.",
  },
];

type Banner = { kind: "ok" | "err"; text: string } | null;

export default function Consistency() {
  const [repos, setRepos] = useState<Repo[] | null>(null);
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState<string | null>(null);

  // golden repo selection
  const [goldenId, setGoldenId] = useState<number | "">("");
  const [savedGoldenId, setSavedGoldenId] = useState<number | null>(null);
  const [savingGolden, setSavingGolden] = useState(false);
  const [goldenBanner, setGoldenBanner] = useState<Banner>(null);

  // bundle selection
  const [selectedBundles, setSelectedBundles] = useState<Set<string>>(
    () => new Set(BUNDLES.map((b) => b.key)),
  );

  // target selection
  const [targetIds, setTargetIds] = useState<Set<number>>(() => new Set());

  // apply
  const [applying, setApplying] = useState(false);
  const [applyBanner, setApplyBanner] = useState<Banner>(null);
  const [results, setResults] = useState<ApplyResult[] | null>(null);

  const load = useCallback(() => {
    setLoading(true);
    setLoadError(null);
    Promise.all([
      listRepos(),
      getConsistencyConfig().catch(() => null as ConsistencyConfig | null),
    ])
      .then(([r, cfg]) => {
        setRepos(r);
        if (cfg) {
          setSavedGoldenId(cfg.golden_repo_id);
          if (cfg.golden_repo_id != null) setGoldenId(cfg.golden_repo_id);
        }
      })
      .catch((e) => setLoadError(e instanceof Error ? e.message : String(e)))
      .finally(() => setLoading(false));
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  // Eligible repos: cloned locally (have a working tree to commit into).
  const cloneable = useMemo(
    () => (repos ?? []).filter((r) => r.local_path),
    [repos],
  );

  const golden = useMemo(
    () => (repos ?? []).find((r) => r.id === goldenId) ?? null,
    [repos, goldenId],
  );

  const reposById = useMemo(() => {
    const m = new Map<number, Repo>();
    for (const r of repos ?? []) m.set(r.id, r);
    return m;
  }, [repos]);

  // Target candidates exclude the chosen golden repo.
  const targetCandidates = useMemo(
    () => cloneable.filter((r) => r.id !== goldenId),
    [cloneable, goldenId],
  );

  const saveGolden = async () => {
    if (goldenId === "") return;
    setSavingGolden(true);
    setGoldenBanner(null);
    try {
      const cfg = await putGoldenRepo(goldenId);
      setSavedGoldenId(cfg.golden_repo_id);
      setGoldenBanner({ kind: "ok", text: "Golden repo saved." });
    } catch (e) {
      setGoldenBanner({
        kind: "err",
        text: e instanceof Error ? e.message : String(e),
      });
    } finally {
      setSavingGolden(false);
    }
  };

  const toggleBundle = (key: string) =>
    setSelectedBundles((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });

  const toggleTarget = (id: number) =>
    setTargetIds((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });

  const allTargetsSelected =
    targetCandidates.length > 0 &&
    targetCandidates.every((r) => targetIds.has(r.id));

  const toggleAllTargets = () =>
    setTargetIds(() =>
      allTargetsSelected ? new Set() : new Set(targetCandidates.map((r) => r.id)),
    );

  const canApply =
    goldenId !== "" &&
    targetIds.size > 0 &&
    selectedBundles.size > 0 &&
    !applying;

  const apply = async () => {
    if (goldenId === "") {
      setApplyBanner({ kind: "err", text: "Choose a golden repo first." });
      return;
    }
    const target_ids = [...targetIds].filter((id) => id !== goldenId);
    if (target_ids.length === 0) {
      setApplyBanner({ kind: "err", text: "Select at least one target repo." });
      return;
    }
    const bundle_keys = [...selectedBundles];
    if (bundle_keys.length === 0) {
      setApplyBanner({ kind: "err", text: "Select at least one bundle." });
      return;
    }
    setApplying(true);
    setApplyBanner(null);
    setResults(null);
    try {
      const resp = await applyConsistency({
        golden_id: goldenId,
        target_ids,
        bundle_keys,
      });
      setResults(resp.results ?? []);
      const okCount = (resp.results ?? []).filter((r) => r.ok).length;
      const total = (resp.results ?? []).length;
      setApplyBanner({
        kind: okCount === total ? "ok" : "err",
        text: `Applied to ${okCount}/${total} repos. Review the changes on each repo's repohub-staging branch in the Merge tab.`,
      });
    } catch (e) {
      setApplyBanner({
        kind: "err",
        text: e instanceof Error ? e.message : String(e),
      });
    } finally {
      setApplying(false);
    }
  };

  return (
    <div className="mx-auto max-w-5xl space-y-6">
      <header className="rounded-xl border border-edge bg-panel p-6">
        <h1 className="text-2xl font-semibold tracking-tight">Consistency</h1>
        <p className="mt-2 text-sm text-slate-400">
          Stamp shared config bundles from a{" "}
          <span className="text-slate-200">golden</span> repo onto your other
          repos so formatting, metadata, secret hygiene and CLAUDE.md guidance
          stay uniform. Changes always land on each repo's{" "}
          <code className="rounded bg-edge px-1 py-0.5 text-xs text-slate-200">
            repohub-staging
          </code>{" "}
          branch — never merged automatically. Review and merge them in the{" "}
          <span className="text-slate-200">Merge</span> tab.
        </p>
      </header>

      {loading ? (
        <p className="text-sm text-slate-400">Loading repos…</p>
      ) : loadError ? (
        <div className="flex items-center justify-between rounded-md bg-rose-500/10 px-3 py-2 text-sm text-rose-400">
          <span>Failed to load: {loadError}</span>
          <button
            onClick={load}
            className="rounded border border-rose-500/40 px-2 py-0.5 text-xs hover:bg-rose-500/10"
          >
            Retry
          </button>
        </div>
      ) : (
        <>
          {/* ---- Golden repo ---- */}
          <section className="rounded-xl border border-edge bg-panel p-6">
            <h2 className="text-lg font-semibold">Golden repo</h2>
            <p className="mt-1 text-xs text-slate-500">
              The source of truth that bundles are copied from. Only cloned
              repos can be used.
            </p>

            <div className="mt-4 flex flex-wrap items-end gap-3">
              <label className="flex min-w-[16rem] flex-1 flex-col gap-1.5">
                <span className="text-xs font-medium text-slate-400">
                  Source repo
                </span>
                <select
                  value={goldenId === "" ? "" : String(goldenId)}
                  onChange={(e) =>
                    setGoldenId(e.target.value === "" ? "" : Number(e.target.value))
                  }
                  className="w-full rounded-md border border-edge bg-slate-900/60 px-3 py-2 text-sm text-slate-200 outline-none focus:border-accent"
                >
                  <option value="">Select a repo…</option>
                  {cloneable.map((r) => (
                    <option key={r.id} value={r.id}>
                      {r.full_name}
                    </option>
                  ))}
                </select>
              </label>
              <button
                onClick={saveGolden}
                disabled={savingGolden || goldenId === ""}
                className="rounded-md bg-accent/20 px-4 py-2 text-sm text-accent transition hover:bg-accent/30 disabled:cursor-not-allowed disabled:opacity-40"
              >
                {savingGolden ? "Saving…" : "Save"}
              </button>
            </div>

            {savedGoldenId != null && (
              <p className="mt-2 text-xs text-slate-500">
                Saved golden:{" "}
                <span className="text-slate-300">
                  {reposById.get(savedGoldenId)?.full_name ??
                    `repo #${savedGoldenId}`}
                </span>
              </p>
            )}
            {cloneable.length === 0 && (
              <p className="mt-2 text-xs text-amber-400">
                No cloned repos yet. Clone a repo on the Dashboard to use it as a
                golden source.
              </p>
            )}

            {goldenBanner && <BannerRow banner={goldenBanner} />}
          </section>

          {/* ---- Bundles ---- */}
          <section className="rounded-xl border border-edge bg-panel p-6">
            <h2 className="text-lg font-semibold">Config bundles</h2>
            <p className="mt-1 text-xs text-slate-500">
              Choose which slices of config to copy from the golden repo.
            </p>
            <div className="mt-4 grid gap-3 sm:grid-cols-2">
              {BUNDLES.map((b) => {
                const checked = selectedBundles.has(b.key);
                return (
                  <label
                    key={b.key}
                    className={`flex cursor-pointer gap-3 rounded-xl border p-4 transition ${
                      checked ? "border-accent/40 bg-accent/5" : "border-edge"
                    }`}
                  >
                    <input
                      type="checkbox"
                      checked={checked}
                      onChange={() => toggleBundle(b.key)}
                      className="mt-0.5 h-4 w-4 shrink-0 accent-accent"
                    />
                    <span>
                      <span className="block font-medium text-slate-200">
                        {b.title}
                        <code className="ml-2 rounded bg-edge px-1.5 py-0.5 text-[10px] text-slate-400">
                          {b.key}
                        </code>
                      </span>
                      <span className="mt-1 block text-sm text-slate-400">
                        {b.description}
                      </span>
                    </span>
                  </label>
                );
              })}
            </div>
          </section>

          {/* ---- Targets ---- */}
          <section className="rounded-xl border border-edge bg-panel p-6">
            <div className="flex items-center justify-between">
              <h2 className="text-lg font-semibold">Target repos</h2>
              {targetCandidates.length > 0 && (
                <button
                  onClick={toggleAllTargets}
                  className="rounded-md border border-edge px-2.5 py-1 text-xs text-slate-300 transition hover:bg-edge"
                >
                  {allTargetsSelected ? "Clear all" : "Select all"}
                </button>
              )}
            </div>
            <p className="mt-1 text-xs text-slate-500">
              Repos to stamp the selected bundles onto. The golden repo is
              excluded. {targetIds.size} selected.
            </p>

            {targetCandidates.length === 0 ? (
              <p className="mt-4 rounded-lg border border-dashed border-edge px-4 py-6 text-center text-sm text-slate-500">
                {goldenId === ""
                  ? "Select a golden repo to see eligible targets."
                  : "No other cloned repos available as targets."}
              </p>
            ) : (
              <ul className="mt-4 divide-y divide-edge">
                {targetCandidates.map((r) => {
                  const checked = targetIds.has(r.id);
                  return (
                    <li key={r.id}>
                      <label className="flex cursor-pointer items-center gap-3 py-2.5">
                        <input
                          type="checkbox"
                          checked={checked}
                          onChange={() => toggleTarget(r.id)}
                          className="h-4 w-4 shrink-0 accent-accent"
                        />
                        <span className="min-w-0 flex-1">
                          <span className="block truncate text-sm text-slate-200">
                            {r.full_name}
                          </span>
                          {r.description && (
                            <span className="block truncate text-xs text-slate-500">
                              {r.description}
                            </span>
                          )}
                        </span>
                        <span className="flex shrink-0 items-center gap-2 text-[11px]">
                          {r.language && (
                            <span className="rounded-full bg-edge px-2 py-0.5 text-slate-300">
                              {r.language}
                            </span>
                          )}
                          <span
                            className={`rounded-full px-2 py-0.5 ${
                              r.dirty
                                ? "bg-amber-500/15 text-amber-300"
                                : "bg-emerald-500/15 text-emerald-300"
                            }`}
                          >
                            {r.dirty ? "dirty" : "clean"}
                          </span>
                        </span>
                      </label>
                    </li>
                  );
                })}
              </ul>
            )}
          </section>

          {/* ---- Apply ---- */}
          <section className="rounded-xl border border-edge bg-panel p-6">
            <div className="flex flex-wrap items-center justify-between gap-3">
              <div className="text-sm text-slate-400">
                {golden ? (
                  <>
                    Copy{" "}
                    <span className="text-slate-200">
                      {selectedBundles.size}
                    </span>{" "}
                    bundle{selectedBundles.size === 1 ? "" : "s"} from{" "}
                    <span className="text-slate-200">{golden.full_name}</span> to{" "}
                    <span className="text-slate-200">{targetIds.size}</span> repo
                    {targetIds.size === 1 ? "" : "s"}.
                  </>
                ) : (
                  "Pick a golden repo, bundles, and targets to apply."
                )}
              </div>
              <button
                onClick={apply}
                disabled={!canApply}
                className="rounded-md bg-accent/20 px-4 py-2 text-sm text-accent transition hover:bg-accent/30 disabled:cursor-not-allowed disabled:opacity-40"
              >
                {applying ? "Applying…" : "Apply to repohub-staging"}
              </button>
            </div>

            {applyBanner && <BannerRow banner={applyBanner} />}

            {results && results.length > 0 && (
              <ul className="mt-4 space-y-2">
                {results.map((res) => {
                  const name =
                    res.full_name ??
                    reposById.get(res.repo_id)?.full_name ??
                    `repo #${res.repo_id}`;
                  return (
                    <li
                      key={res.repo_id}
                      className="rounded-lg border border-edge bg-slate-900/40 p-3"
                    >
                      <div className="flex flex-wrap items-center gap-2">
                        <span
                          className={`text-sm ${
                            res.ok ? "text-emerald-400" : "text-rose-400"
                          }`}
                        >
                          {res.ok ? "✓" : "✕"}
                        </span>
                        <span className="font-medium text-slate-200">
                          {name}
                        </span>
                        {res.branch && (
                          <code className="rounded bg-edge px-1.5 py-0.5 text-[11px] text-slate-300">
                            {res.branch}
                          </code>
                        )}
                        {res.ok &&
                          res.changed_files &&
                          res.changed_files.length > 0 && (
                            <span className="text-xs text-slate-500">
                              {res.changed_files.length} file
                              {res.changed_files.length === 1 ? "" : "s"} changed
                            </span>
                          )}
                      </div>
                      {(res.message || res.error) && (
                        <p
                          className={`mt-1 text-xs ${
                            res.ok ? "text-slate-500" : "text-rose-400"
                          }`}
                        >
                          {res.error ?? res.message}
                        </p>
                      )}
                      {res.ok &&
                        res.changed_files &&
                        res.changed_files.length > 0 && (
                          <div className="mt-2 flex flex-wrap gap-1.5">
                            {res.changed_files.map((f) => (
                              <code
                                key={f}
                                className="rounded bg-edge px-1.5 py-0.5 text-[11px] text-slate-300"
                              >
                                {f}
                              </code>
                            ))}
                          </div>
                        )}
                    </li>
                  );
                })}
              </ul>
            )}
          </section>
        </>
      )}
    </div>
  );
}

function BannerRow({ banner }: { banner: NonNullable<Banner> }) {
  return (
    <p
      className={`mt-3 rounded-md px-3 py-2 text-sm ${
        banner.kind === "ok"
          ? "bg-emerald-500/10 text-emerald-400"
          : "bg-rose-500/10 text-rose-400"
      }`}
    >
      {banner.text}
    </p>
  );
}
