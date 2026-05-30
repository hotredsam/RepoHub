import { useCallback, useEffect, useState } from "react";

// Permissions.tsx (P20)
//
// READ-ONLY view of the GitHub access this dashboard uses. It shows the active
// `gh` account, the token's OAuth scopes (as chips), any scopes already flagged
// as over-broad (in amber), and a per-repo role / default-branch-protection
// table for tracked repos. An "Analyze with Claude" button asks the local
// `claude` CLI for a plain-language summary + least-privilege recommendations.
//
// Nothing here changes a single permission — every call is a read. The UI makes
// that explicit so the user knows reviewing this page is always safe.

// ---- API contract (kept local to this unit; mirrors backend gh_perms.rs) ----
//
// The wire shapes below match `gh_perms::PermissionsReport` exactly. They are
// intentionally defined locally (rather than importing the broader
// lib/types.PermissionsReport) so this view tracks the real backend response.

// One scope that grants more authority than this local-first app needs.
interface OverBroad {
  scope: string;
  why: string;
}

// Per-repo permission summary (best-effort; null/false on probe failure).
interface RepoPerm {
  full_name: string;
  role: string | null;
  branch_protected: boolean;
}

// Body of GET /api/permissions (and the `raw` field of the analyze response).
interface PermissionsReport {
  account: string | null;
  token_scopes: string[];
  over_broad: OverBroad[];
  repos: RepoPerm[];
  // Populated only by POST /api/permissions/analyze.
  summary: string | null;
}

// Body of POST /api/permissions/analyze: the Claude summary (or null) plus a
// degradation note and the same raw report regardless of CLI availability.
interface AnalyzeResponse {
  summary: string | null;
  error?: string | null;
  raw: PermissionsReport;
}

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

const getPermissions = () => request<PermissionsReport>("/api/permissions");

const analyzePermissions = () =>
  request<AnalyzeResponse>("/api/permissions/analyze", { method: "POST" });

// Scopes flagged as over-broad are rendered in amber; everything else neutral.
function isFlagged(scope: string, overBroad: OverBroad[]): boolean {
  const s = scope.trim().toLowerCase();
  return overBroad.some((o) => o.scope.trim().toLowerCase() === s);
}

// ---- component ----

export default function Permissions() {
  const [report, setReport] = useState<PermissionsReport | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  // Analyze (Claude) state.
  const [analyzing, setAnalyzing] = useState(false);
  const [summary, setSummary] = useState<string | null>(null);
  const [analyzeNote, setAnalyzeNote] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const data = await getPermissions();
      setReport(data);
      // A fresh load supersedes any prior analysis.
      setSummary(null);
      setAnalyzeNote(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const runAnalyze = useCallback(async () => {
    setAnalyzing(true);
    setSummary(null);
    setAnalyzeNote(null);
    try {
      const res = await analyzePermissions();
      // The analyze endpoint also returns the freshest raw report — adopt it so
      // the chips/table reflect exactly what Claude reasoned over.
      if (res.raw) setReport(res.raw);
      setSummary(res.summary ?? null);
      if (res.error) setAnalyzeNote(res.error);
      else if (!res.summary)
        setAnalyzeNote("Claude returned no summary; showing raw data only.");
    } catch (e) {
      setAnalyzeNote(e instanceof Error ? e.message : String(e));
    } finally {
      setAnalyzing(false);
    }
  }, []);

  return (
    <div className="mx-auto flex max-w-5xl flex-col gap-6">
      {/* ---- header ---- */}
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h1 className="text-2xl font-semibold tracking-tight text-slate-100">
            GitHub permissions
          </h1>
          <p className="mt-1 max-w-2xl text-sm text-slate-400">
            A read-only look at the GitHub access this dashboard uses — the
            signed-in <code className="text-slate-400">gh</code> account, its
            token scopes, and your role on each tracked repo.
          </p>
        </div>
        <button
          onClick={() => void load()}
          disabled={loading}
          className="rounded-md border border-edge px-3 py-1.5 text-sm text-slate-300 transition hover:bg-edge hover:text-slate-100 disabled:opacity-50"
        >
          {loading ? "Loading…" : "Refresh"}
        </button>
      </div>

      {/* ---- read-only assurance ---- */}
      <div className="rounded-lg border border-edge bg-panel px-4 py-3 text-sm text-slate-400">
        <span className="font-medium text-slate-200">Read-only.</span> Nothing on
        this page changes any permission. Every check is a GitHub read; revoking
        or narrowing scopes is done by you in your GitHub settings.
      </div>

      {error ? (
        <div className="rounded-lg border border-rose-500/40 bg-rose-500/10 p-4 text-sm text-rose-300">
          <p className="font-medium">Could not load the permissions report.</p>
          <p className="mt-1 text-rose-300/80">{error}</p>
          <button
            onClick={() => void load()}
            className="mt-3 rounded-md border border-rose-500/40 px-3 py-1 text-xs text-rose-200 transition hover:bg-rose-500/20"
          >
            Try again
          </button>
        </div>
      ) : loading ? (
        <div className="h-64 animate-pulse rounded-xl border border-edge bg-edge/20" />
      ) : report ? (
        <>
          {/* ---- account + token scopes ---- */}
          <section className="rounded-xl border border-edge bg-panel p-6">
            <h2 className="text-lg font-semibold text-slate-100">
              Account &amp; token scopes
            </h2>
            <p className="mt-1 text-xs text-slate-500">
              Reported by <code className="text-slate-400">gh auth status</code>.
              The token value itself is never read or shown — only its declared
              scopes.
            </p>

            <div className="mt-4 flex flex-wrap items-center gap-2 text-sm">
              <span className="text-slate-400">Signed in as</span>
              {report.account ? (
                <span className="rounded-md bg-edge px-2 py-0.5 font-medium text-slate-100">
                  {report.account}
                </span>
              ) : (
                <span className="text-slate-500">
                  not signed in (no <code>gh</code> session found)
                </span>
              )}
            </div>

            <div className="mt-4">
              <p className="text-xs font-medium uppercase tracking-wide text-slate-500">
                Token scopes
              </p>
              {report.token_scopes.length === 0 ? (
                <p className="mt-2 text-sm text-slate-500">
                  No scopes reported.
                </p>
              ) : (
                <div className="mt-2 flex flex-wrap gap-2">
                  {report.token_scopes.map((scope) => {
                    const flagged = isFlagged(scope, report.over_broad);
                    return (
                      <span
                        key={scope}
                        title={
                          flagged
                            ? "Flagged as over-broad — see below"
                            : undefined
                        }
                        className={
                          flagged
                            ? "inline-flex items-center gap-1 rounded-full border border-amber-500/40 bg-amber-500/10 px-2.5 py-0.5 font-mono text-xs text-amber-300"
                            : "inline-flex items-center rounded-full border border-edge bg-slate-900/60 px-2.5 py-0.5 font-mono text-xs text-slate-300"
                        }
                      >
                        {flagged && <span aria-hidden>⚠</span>}
                        {scope}
                      </span>
                    );
                  })}
                </div>
              )}
            </div>

            {/* ---- over-broad flags (amber) ---- */}
            {report.over_broad.length > 0 && (
              <div className="mt-5 rounded-lg border border-amber-500/40 bg-amber-500/10 p-4">
                <p className="text-sm font-medium text-amber-300">
                  {report.over_broad.length} scope
                  {report.over_broad.length === 1 ? "" : "s"} broader than this
                  app needs
                </p>
                <ul className="mt-2 space-y-2">
                  {report.over_broad.map((o) => (
                    <li key={o.scope} className="text-sm text-amber-200/90">
                      <code className="rounded bg-amber-500/15 px-1.5 py-0.5 font-mono text-xs text-amber-200">
                        {o.scope}
                      </code>{" "}
                      <span className="text-amber-200/80">{o.why}</span>
                    </li>
                  ))}
                </ul>
              </div>
            )}
          </section>

          {/* ---- per-repo role + branch protection ---- */}
          <section className="rounded-xl border border-edge bg-panel p-6">
            <h2 className="text-lg font-semibold text-slate-100">
              Per-repo access
            </h2>
            <p className="mt-1 text-xs text-slate-500">
              Your role and default-branch protection on each tracked repo.
              Probed best-effort via read-only <code>gh api</code> calls — a
              missing role means the repo could not be read with this token.
            </p>

            <div className="mt-4">
              {report.repos.length === 0 ? (
                <p className="rounded-lg border border-dashed border-edge px-4 py-6 text-center text-sm text-slate-500">
                  No tracked repos to report. Track repositories on the Dashboard
                  first.
                </p>
              ) : (
                <div className="overflow-hidden rounded-lg border border-edge">
                  <table className="w-full text-sm">
                    <thead className="bg-slate-900/60 text-left text-xs uppercase tracking-wide text-slate-500">
                      <tr>
                        <th className="px-3 py-2 font-medium">Repository</th>
                        <th className="px-3 py-2 font-medium">Your role</th>
                        <th className="px-3 py-2 font-medium">
                          Default branch
                        </th>
                      </tr>
                    </thead>
                    <tbody>
                      {report.repos.map((r) => (
                        <tr
                          key={r.full_name}
                          className="border-t border-edge text-slate-300"
                        >
                          <td className="px-3 py-2 font-mono text-xs text-slate-200">
                            {r.full_name}
                          </td>
                          <td className="px-3 py-2">
                            {r.role ? (
                              <span className="rounded bg-edge px-1.5 py-0.5 text-[11px] uppercase tracking-wide text-slate-300">
                                {r.role}
                              </span>
                            ) : (
                              <span className="text-slate-500">unknown</span>
                            )}
                          </td>
                          <td className="px-3 py-2">
                            {r.branch_protected ? (
                              <span className="text-emerald-400">
                                protected
                              </span>
                            ) : (
                              <span className="text-amber-300">
                                not protected
                              </span>
                            )}
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              )}
            </div>
          </section>

          {/* ---- analyze with Claude ---- */}
          <section className="rounded-xl border border-edge bg-panel p-6">
            <div className="flex flex-wrap items-center justify-between gap-3">
              <div>
                <h2 className="text-lg font-semibold text-slate-100">
                  Analyze with Claude
                </h2>
                <p className="mt-1 text-xs text-slate-500">
                  Ask the local <code className="text-slate-400">claude</code>{" "}
                  CLI to explain, in plain language, what this token can do and
                  where you could safely narrow it. Advisory only — it never
                  changes anything.
                </p>
              </div>
              <button
                onClick={() => void runAnalyze()}
                disabled={analyzing}
                className="shrink-0 rounded-md bg-accent/20 px-4 py-2 text-sm text-accent transition hover:bg-accent/30 disabled:cursor-not-allowed disabled:opacity-40"
              >
                {analyzing ? "Asking Claude…" : "Analyze with Claude"}
              </button>
            </div>

            {analyzing && (
              <p className="mt-3 text-xs text-slate-500">
                This calls the local Claude CLI and may take a moment.
              </p>
            )}

            {analyzeNote && (
              <p className="mt-4 rounded-md bg-amber-500/10 px-3 py-2 text-sm text-amber-300">
                {analyzeNote}
              </p>
            )}

            {summary && (
              <div className="mt-4 rounded-lg border border-edge bg-slate-900/40 p-4">
                <p className="text-xs font-medium uppercase tracking-wide text-slate-500">
                  Summary &amp; recommendations
                </p>
                <p className="mt-2 whitespace-pre-wrap text-sm leading-relaxed text-slate-200">
                  {summary}
                </p>
                <p className="mt-3 text-[11px] text-slate-500">
                  Generated by Claude from the read-only data above. Apply any
                  changes yourself in GitHub — RepoHub will not.
                </p>
              </div>
            )}
          </section>
        </>
      ) : null}
    </div>
  );
}
