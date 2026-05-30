import { useCallback, useEffect, useMemo, useState } from "react";

// AiOps.tsx — P16 unified Settings hub, "AI Ops" panel.
//
// Three concerns, layered GLOBAL ⇄ PER-REPO like the rest of the Settings hub:
//   1. Evals — Vertex AI Gen AI Evaluation. List/create suites for this scope,
//      Run a suite (POST /api/evals/:id/run), and review the run history
//      (GET /api/evals/runs). Live runs are gated on gcloud readiness.
//   2. Fine-tuning — placeholder only ("Coming soon").
//   3. API keys / secrets — these live in Google Cloud Secret Manager and are
//      fetched on demand by agents; we only ever list referenced secret NAMES,
//      never any value.
//
// The API helpers below are defined locally (mirroring src/lib/api.ts's
// same-origin/error-body conventions) to keep this change scoped to a single
// file, exactly as Connections.tsx / Merge.tsx do.

// ---- props ----

interface AiOpsProps {
  /** Settings layer this panel edits: the global default or a per-repo override. */
  scope: "global" | "repo";
  /** Repo this panel is scoped to, when `scope === "repo"`. */
  repoId?: number | null;
}

// ---- response/request shapes (mirror backend db.rs eval_* tables + gcloud.rs) ----

// gcloud.rs::GcloudStatus
interface GcloudStatus {
  installed: boolean;
  adc: boolean;
  project: string | null;
  region: string | null;
}

// db.rs::eval_suites
interface EvalSuite {
  id: number;
  name: string;
  scope: string;
  repo_id: number | null;
  config_json: string | null;
  created_at: string | null;
}

// db.rs::eval_runs
interface EvalRun {
  id: number;
  suite_id: number | null;
  model: string | null;
  score: number | null;
  passed: number | null;
  total: number | null;
  detail_json: string | null;
  created_at: string | null;
}

interface CreateSuiteBody {
  name: string;
  scope: string;
  repo_id: number | null;
  // The backend (evals_gcloud::CreateEvalBody) reads `config` (a JSON object)
  // and serializes it to the config_json column itself.
  config: unknown;
}

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

function qs(params: Record<string, string | number | undefined | null>): string {
  const sp = new URLSearchParams();
  for (const [k, v] of Object.entries(params)) {
    if (v !== undefined && v !== null && v !== "") sp.set(k, String(v));
  }
  const s = sp.toString();
  return s ? `?${s}` : "";
}

const getGcloud = () => request<GcloudStatus>("/api/gcloud");

const listSuites = (scope: string, repo_id?: number | null) =>
  request<EvalSuite[]>(`/api/evals${qs({ scope, repo_id })}`);

const createSuite = (body: CreateSuiteBody) =>
  request<EvalSuite>("/api/evals", {
    method: "POST",
    body: JSON.stringify(body),
  });

const runSuite = (id: number) =>
  request<EvalRun>(`/api/evals/${id}/run`, { method: "POST" });

const listRuns = (suite_id?: number | null) =>
  request<EvalRun[]>(`/api/evals/runs${qs({ suite_id })}`);

// ---------------------------------------------------------------------------

type Banner = { kind: "ok" | "err"; text: string } | null;

export default function AiOps({ scope, repoId }: AiOpsProps) {
  const repoScoped = scope === "repo";
  const effectiveRepoId = repoScoped ? (repoId ?? null) : null;

  // ---- gcloud readiness (gates live eval runs) ----
  const [gcloud, setGcloud] = useState<GcloudStatus | null>(null);
  const [gcloudError, setGcloudError] = useState<string | null>(null);

  // ---- eval suites ----
  const [suites, setSuites] = useState<EvalSuite[] | null>(null);
  const [suitesLoading, setSuitesLoading] = useState(true);
  const [suitesError, setSuitesError] = useState<string | null>(null);
  const [suiteName, setSuiteName] = useState("");
  const [suiteConfig, setSuiteConfig] = useState("");
  const [creating, setCreating] = useState(false);
  const [suiteBanner, setSuiteBanner] = useState<Banner>(null);
  const [running, setRunning] = useState<number | null>(null);

  // ---- run history ----
  const [runs, setRuns] = useState<EvalRun[] | null>(null);
  const [runsLoading, setRunsLoading] = useState(true);
  const [runsError, setRunsError] = useState<string | null>(null);

  const ready = !!gcloud && gcloud.installed && gcloud.adc;

  // ---- loaders ----
  const loadGcloud = useCallback(() => {
    setGcloudError(null);
    getGcloud()
      .then(setGcloud)
      .catch((e) => setGcloudError(e instanceof Error ? e.message : String(e)));
  }, []);

  const loadSuites = useCallback(() => {
    setSuitesLoading(true);
    setSuitesError(null);
    listSuites(scope, effectiveRepoId)
      .then(setSuites)
      .catch((e) => setSuitesError(e instanceof Error ? e.message : String(e)))
      .finally(() => setSuitesLoading(false));
  }, [scope, effectiveRepoId]);

  const loadRuns = useCallback(() => {
    setRunsLoading(true);
    setRunsError(null);
    listRuns()
      .then(setRuns)
      .catch((e) => setRunsError(e instanceof Error ? e.message : String(e)))
      .finally(() => setRunsLoading(false));
  }, []);

  useEffect(() => {
    loadGcloud();
    loadSuites();
    loadRuns();
  }, [loadGcloud, loadSuites, loadRuns]);

  // The suites visible in this scope reference these secret names (parsed from
  // each suite's config_json, e.g. { "secrets": ["ANTHROPIC_API_KEY", ...] }).
  // We surface the NAMES only — values never leave Secret Manager.
  const referencedSecrets = useMemo(() => {
    const names = new Set<string>();
    for (const s of suites ?? []) {
      if (!s.config_json) continue;
      try {
        const cfg = JSON.parse(s.config_json) as Record<string, unknown>;
        const raw = cfg.secrets ?? cfg.secret_names ?? cfg.api_keys;
        if (Array.isArray(raw)) {
          for (const n of raw) if (typeof n === "string" && n.trim()) names.add(n.trim());
        }
      } catch {
        // ignore malformed config_json
      }
    }
    return [...names].sort((a, b) => a.localeCompare(b));
  }, [suites]);

  const suiteName_byId = useMemo(() => {
    const m = new Map<number, string>();
    for (const s of suites ?? []) m.set(s.id, s.name);
    return m;
  }, [suites]);

  // ---- handlers ----
  const create = async (e: React.FormEvent) => {
    e.preventDefault();
    const name = suiteName.trim();
    if (!name) {
      setSuiteBanner({ kind: "err", text: "Suite name is required." });
      return;
    }
    let config: unknown = undefined;
    const cfgText = suiteConfig.trim();
    if (cfgText) {
      try {
        // Validate JSON before sending; the backend serializes it to config_json.
        config = JSON.parse(cfgText);
      } catch {
        setSuiteBanner({ kind: "err", text: "Config must be valid JSON (or left blank)." });
        return;
      }
    }
    setCreating(true);
    setSuiteBanner(null);
    try {
      const created = await createSuite({
        name,
        scope,
        repo_id: effectiveRepoId,
        config,
      });
      setSuites((list) => (list ? [created, ...list] : [created]));
      setSuiteName("");
      setSuiteConfig("");
      setSuiteBanner({ kind: "ok", text: `Created suite "${created.name}".` });
    } catch (err) {
      setSuiteBanner({ kind: "err", text: err instanceof Error ? err.message : String(err) });
    } finally {
      setCreating(false);
    }
  };

  const run = async (id: number) => {
    setRunning(id);
    setSuiteBanner(null);
    try {
      const result = await runSuite(id);
      setRuns((list) => (list ? [result, ...list] : [result]));
      setSuiteBanner({
        kind: "ok",
        text: `Ran "${suiteName_byId.get(id) ?? `suite ${id}`}".`,
      });
    } catch (err) {
      setSuiteBanner({ kind: "err", text: err instanceof Error ? err.message : String(err) });
    } finally {
      setRunning(null);
    }
  };

  return (
    <div className="space-y-6">
      <div>
        <h2 className="text-lg font-semibold text-slate-100">AI Ops</h2>
        <p className="mt-1 text-xs text-slate-500">
          Evaluations, fine-tuning, and the secrets your agents reference —{" "}
          {repoScoped ? (
            <>
              scoped to this repository. Suites you create here{" "}
              <span className="text-accent">override</span> the global defaults.
            </>
          ) : (
            <>the global defaults, inherited by every repo unless overridden.</>
          )}
        </p>
      </div>

      {/* ---- gcloud readiness banner ---- */}
      {!ready && (
        <ConfigureBanner status={gcloud} error={gcloudError} onRetry={loadGcloud} />
      )}

      {/* ---- Evals ---- */}
      <section className="rounded-xl border border-edge bg-panel p-6">
        <h3 className="text-base font-semibold text-slate-100">Evaluations</h3>
        <p className="mt-1 text-xs text-slate-500">
          Vertex AI Gen AI Evaluation suites. Define a suite, then run it to score a
          model against your criteria. Runs require Google Cloud to be configured.
        </p>

        {/* create suite */}
        <form onSubmit={create} className="mt-4 grid gap-3 sm:grid-cols-2">
          <Field label="Suite name">
            <input
              value={suiteName}
              onChange={(e) => setSuiteName(e.target.value)}
              placeholder="answer-quality"
              className={inputCls}
            />
          </Field>
          <Field label="Config (JSON, optional)">
            <textarea
              value={suiteConfig}
              onChange={(e) => setSuiteConfig(e.target.value)}
              rows={3}
              placeholder={`{\n  "metrics": ["coherence"],\n  "secrets": ["ANTHROPIC_API_KEY"]\n}`}
              className={`${inputCls} resize-y font-mono text-xs`}
            />
          </Field>
          <div className="sm:col-span-2">
            <button
              type="submit"
              disabled={creating}
              className="rounded-md bg-accent/20 px-4 py-2 text-sm text-accent transition hover:bg-accent/30 disabled:cursor-not-allowed disabled:opacity-40"
            >
              {creating ? "Creating…" : "Create suite"}
            </button>
          </div>
        </form>

        {suiteBanner && <BannerRow banner={suiteBanner} />}

        {/* suite list */}
        <div className="mt-5">
          {suitesLoading ? (
            <p className="text-sm text-slate-400">Loading suites…</p>
          ) : suitesError ? (
            <ErrorRow text={suitesError} onRetry={loadSuites} />
          ) : !suites || suites.length === 0 ? (
            <p className="rounded-lg border border-dashed border-edge px-4 py-6 text-center text-sm text-slate-500">
              No evaluation suites in this scope yet.
            </p>
          ) : (
            <ul className="space-y-2">
              {suites.map((s) => (
                <li
                  key={s.id}
                  className="rounded-lg border border-edge bg-slate-900/40 p-3"
                >
                  <div className="flex flex-wrap items-center gap-3">
                    <span className="font-medium text-slate-200">{s.name}</span>
                    <span className="rounded bg-edge px-1.5 py-0.5 text-[11px] uppercase tracking-wide text-slate-400">
                      {s.scope}
                      {s.repo_id != null ? ` · repo ${s.repo_id}` : ""}
                    </span>
                    {s.created_at && (
                      <span className="text-xs text-slate-500">
                        {fmtDate(s.created_at)}
                      </span>
                    )}
                    <div className="ml-auto">
                      <button
                        onClick={() => run(s.id)}
                        disabled={running !== null || !ready}
                        title={ready ? "Run this suite" : "Configure Google Cloud to run"}
                        className="rounded-md border border-edge px-2.5 py-1 text-xs text-slate-300 transition hover:bg-edge disabled:cursor-not-allowed disabled:opacity-40"
                      >
                        {running === s.id ? "Running…" : "Run"}
                      </button>
                    </div>
                  </div>
                  {s.config_json && (
                    <pre className="mt-2 max-h-32 overflow-auto whitespace-pre-wrap break-words rounded-md border border-edge bg-slate-950/60 px-3 py-2 font-mono text-[11px] leading-relaxed text-slate-400">
                      {prettyJson(s.config_json)}
                    </pre>
                  )}
                </li>
              ))}
            </ul>
          )}
        </div>

        {/* run history */}
        <div className="mt-6">
          <div className="flex items-center justify-between">
            <h4 className="text-xs font-medium uppercase tracking-wide text-slate-500">
              Run history
            </h4>
            <button
              onClick={loadRuns}
              disabled={runsLoading}
              className="rounded border border-edge px-2 py-0.5 text-[11px] text-slate-400 transition hover:bg-edge disabled:opacity-40"
            >
              {runsLoading ? "Refreshing…" : "Refresh"}
            </button>
          </div>
          <div className="mt-2">
            {runsLoading ? (
              <p className="text-sm text-slate-400">Loading runs…</p>
            ) : runsError ? (
              <ErrorRow text={runsError} onRetry={loadRuns} />
            ) : !runs || runs.length === 0 ? (
              <p className="rounded-lg border border-dashed border-edge px-4 py-6 text-center text-sm text-slate-500">
                No evaluation runs recorded yet.
              </p>
            ) : (
              <div className="overflow-hidden rounded-lg border border-edge">
                <table className="w-full text-sm">
                  <thead className="bg-slate-900/60 text-left text-xs uppercase tracking-wide text-slate-500">
                    <tr>
                      <th className="px-3 py-2 font-medium">Suite</th>
                      <th className="px-3 py-2 font-medium">Model</th>
                      <th className="px-3 py-2 font-medium">Score</th>
                      <th className="px-3 py-2 font-medium">Passed</th>
                      <th className="px-3 py-2 font-medium">When</th>
                    </tr>
                  </thead>
                  <tbody>
                    {runs.map((r) => (
                      <tr key={r.id} className="border-t border-edge text-slate-300">
                        <td className="px-3 py-2 text-slate-200">
                          {r.suite_id != null
                            ? suiteName_byId.get(r.suite_id) ?? `suite ${r.suite_id}`
                            : "—"}
                        </td>
                        <td className="px-3 py-2 font-mono text-xs">{r.model ?? "—"}</td>
                        <td className="px-3 py-2">
                          {r.score != null ? r.score.toFixed(3) : "—"}
                        </td>
                        <td className="px-3 py-2">
                          {r.passed != null && r.total != null
                            ? `${r.passed}/${r.total}`
                            : "—"}
                        </td>
                        <td className="px-3 py-2 text-xs text-slate-500">
                          {r.created_at ? fmtDate(r.created_at) : "—"}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}
          </div>
        </div>
      </section>

      {/* ---- Fine-tuning (placeholder) ---- */}
      <section className="rounded-xl border border-edge bg-panel p-6">
        <h3 className="text-base font-semibold text-slate-100">Fine-tuning</h3>
        <p className="mt-1 text-xs text-slate-500">
          Train a model on your repos' code and conventions.
        </p>
        <div className="mt-4 rounded-lg border border-dashed border-edge px-4 py-8 text-center">
          <span className="inline-block rounded-full bg-edge px-2.5 py-0.5 text-[11px] uppercase tracking-wide text-slate-400">
            Coming soon
          </span>
          <p className="mx-auto mt-3 max-w-md text-sm text-slate-500">
            Fine-tuning is not available yet. When it ships, you will be able to launch
            and track tuning jobs from here, per scope.
          </p>
        </div>
      </section>

      {/* ---- API keys / secrets ---- */}
      <section className="rounded-xl border border-edge bg-panel p-6">
        <h3 className="text-base font-semibold text-slate-100">API keys &amp; secrets</h3>
        <p className="mt-1 text-xs text-slate-500">
          Secrets are never stored in RepoHub. They live in{" "}
          <span className="text-slate-300">Google Cloud Secret Manager</span> and are
          fetched on demand by agents at run time. RepoHub only ever shows the secret{" "}
          <span className="text-slate-300">names</span> referenced by your eval suites —
          never any value.
        </p>

        <div className="mt-4">
          <h4 className="text-xs font-medium uppercase tracking-wide text-slate-500">
            Referenced secret names
          </h4>
          {referencedSecrets.length === 0 ? (
            <p className="mt-2 rounded-lg border border-dashed border-edge px-4 py-6 text-center text-sm text-slate-500">
              No secret names referenced by suites in this scope. Add a{" "}
              <code className="text-slate-400">"secrets"</code> array to a suite's config
              to reference one.
            </p>
          ) : (
            <ul className="mt-2 flex flex-wrap gap-2">
              {referencedSecrets.map((name) => (
                <li
                  key={name}
                  className="rounded-md border border-edge bg-slate-900/60 px-2.5 py-1 font-mono text-xs text-slate-300"
                >
                  {name}
                </li>
              ))}
            </ul>
          )}
        </div>

        <p className="mt-4 text-[11px] text-slate-600">
          Manage the values with{" "}
          <code className="text-slate-500">gcloud secrets</code> or the Secret Manager
          console
          {gcloud?.project ? (
            <>
              {" "}in project <span className="font-mono text-slate-500">{gcloud.project}</span>
            </>
          ) : null}
          .
        </p>
      </section>
    </div>
  );
}

// ---- gcloud configure banner ----

function ConfigureBanner({
  status,
  error,
  onRetry,
}: {
  status: GcloudStatus | null;
  error: string | null;
  onRetry: () => void;
}) {
  const missing: string[] = [];
  if (status) {
    if (!status.installed) missing.push("the gcloud CLI is not installed");
    else if (!status.adc)
      missing.push("Application Default Credentials are not set up");
    if (!status.project) missing.push("no project is configured");
    if (!status.region) missing.push("no region is configured");
  }
  return (
    <div className="rounded-xl border border-amber-500/40 bg-amber-500/10 p-4">
      <div className="flex items-start gap-3">
        <span className="mt-0.5 text-lg leading-none text-amber-400">⚠</span>
        <div className="text-sm text-amber-200">
          <p className="font-semibold text-amber-300">
            Google Cloud is not configured.
          </p>
          {error ? (
            <p className="mt-1 text-amber-200/90">
              Could not check status: {error}{" "}
              <button onClick={onRetry} className="underline underline-offset-2">
                retry
              </button>
              .
            </p>
          ) : (
            <p className="mt-1 text-amber-200/90">
              Live eval runs are disabled
              {missing.length > 0 ? <> because {missing.join(", ")}</> : null}. Install
              the CLI, then run{" "}
              <code className="rounded bg-amber-500/15 px-1 py-0.5 text-xs">
                gcloud auth application-default login
              </code>
              , and set the project &amp; region in the Google Cloud settings block. You
              can still create and review suites without it.
            </p>
          )}
        </div>
      </div>
    </div>
  );
}

// ---- small shared bits (mirror Settings.tsx) ----

const inputCls =
  "w-full rounded-md border border-edge bg-slate-900/60 px-3 py-2 text-sm text-slate-200 outline-none focus:border-accent";

function Field({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}) {
  return (
    <label className="flex min-w-[8rem] flex-col gap-1.5">
      <span className="text-xs font-medium text-slate-400">{label}</span>
      {children}
    </label>
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

function ErrorRow({ text, onRetry }: { text: string; onRetry: () => void }) {
  return (
    <div className="flex items-center justify-between rounded-md bg-rose-500/10 px-3 py-2 text-sm text-rose-400">
      <span>{text}</span>
      <button
        onClick={onRetry}
        className="rounded border border-rose-500/40 px-2 py-0.5 text-xs hover:bg-rose-500/10"
      >
        Retry
      </button>
    </div>
  );
}

function fmtDate(iso: string): string {
  const d = new Date(iso);
  return Number.isNaN(d.getTime()) ? iso : d.toLocaleString();
}

function prettyJson(raw: string): string {
  try {
    return JSON.stringify(JSON.parse(raw), null, 2);
  } catch {
    return raw;
  }
}
