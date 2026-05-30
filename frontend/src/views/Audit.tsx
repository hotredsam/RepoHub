import { useCallback, useEffect, useMemo, useState } from "react";
import { listAudit } from "../lib/api";
import type { AuditEntry } from "../lib/types";
import { Field, inputCls, ErrorRow } from "./Settings";

// ---------------------------------------------------------------------------
// Audit log viewer
//
// A filter bar (actor kind / method / path / since) over the audit_log table.
// The backend listAudit endpoint filters server-side by `actor` (free text)
// and pages with `limit`/`before`; the remaining facets (actor kind, method,
// path substring, since timestamp) are applied client-side over the fetched
// page so the bar stays responsive. Each row is expandable to reveal its
// body_excerpt and the full request context.
// ---------------------------------------------------------------------------

const DEFAULT_LIMIT = 200;

const ACTOR_KINDS = ["local", "user", "codex"];
const METHODS = ["GET", "POST", "PUT", "DELETE", "PATCH"];

function fmtDate(iso: string | null): string {
  if (!iso) return "—";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  return d.toLocaleString();
}

// Map an HTTP status code to a Tailwind text color.
function statusCls(status: number | null): string {
  if (status == null) return "text-slate-500";
  if (status >= 500) return "text-rose-400";
  if (status >= 400) return "text-amber-400";
  if (status >= 200 && status < 300) return "text-emerald-400";
  return "text-slate-300";
}

export default function Audit() {
  // ---- filters ----
  const [actor, setActor] = useState(""); // free-text actor (server-side)
  const [actorKind, setActorKind] = useState(""); // client-side facet
  const [method, setMethod] = useState(""); // client-side facet
  const [path, setPath] = useState(""); // client-side substring
  const [since, setSince] = useState(""); // datetime-local; client-side

  // committed actor term used for the server request
  const [actorQuery, setActorQuery] = useState("");

  const [rows, setRows] = useState<AuditEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const [expanded, setExpanded] = useState<number | null>(null);

  const load = useCallback(async (actorTerm: string) => {
    setLoading(true);
    setError(null);
    try {
      const data = await listAudit({
        actor: actorTerm || undefined,
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

  useEffect(() => {
    void load("");
  }, [load]);

  const applyActor = (e: React.FormEvent) => {
    e.preventDefault();
    setActorQuery(actor);
    void load(actor);
  };

  const clearFilters = () => {
    setActor("");
    setActorQuery("");
    setActorKind("");
    setMethod("");
    setPath("");
    setSince("");
    void load("");
  };

  // Client-side facets over the fetched page.
  const filtered = useMemo(() => {
    const pathNeedle = path.trim().toLowerCase();
    const sinceMs = since ? new Date(since).getTime() : null;
    return rows.filter((r) => {
      if (actorKind && (r.actor_kind ?? "") !== actorKind) return false;
      if (method && (r.method ?? "") !== method) return false;
      if (pathNeedle && !(r.path ?? "").toLowerCase().includes(pathNeedle))
        return false;
      if (sinceMs != null) {
        const t = new Date(r.ts).getTime();
        if (Number.isNaN(t) || t < sinceMs) return false;
      }
      return true;
    });
  }, [rows, actorKind, method, path, since]);

  const hasClientFilters =
    !!actorKind || !!method || !!path.trim() || !!since;
  const anyFilter = !!actorQuery || hasClientFilters;

  return (
    <div className="mx-auto flex max-w-6xl flex-col gap-5">
      <div className="flex flex-wrap items-center gap-3">
        <h1 className="text-xl font-semibold">Audit log</h1>
        <span className="text-sm text-slate-500">
          Every authenticated and remote action, with request context.
        </span>
      </div>

      {/* ---- filter bar ---- */}
      <form
        onSubmit={applyActor}
        className="grid gap-3 rounded-xl border border-edge bg-panel p-4 sm:grid-cols-2 lg:grid-cols-5"
      >
        <Field label="Actor">
          <input
            value={actor}
            onChange={(e) => setActor(e.target.value)}
            placeholder="email or token id"
            className={inputCls}
          />
        </Field>
        <Field label="Actor kind">
          <select
            value={actorKind}
            onChange={(e) => setActorKind(e.target.value)}
            className={inputCls}
          >
            <option value="">Any</option>
            {ACTOR_KINDS.map((k) => (
              <option key={k} value={k}>
                {k}
              </option>
            ))}
          </select>
        </Field>
        <Field label="Method">
          <select
            value={method}
            onChange={(e) => setMethod(e.target.value)}
            className={inputCls}
          >
            <option value="">Any</option>
            {METHODS.map((m) => (
              <option key={m} value={m}>
                {m}
              </option>
            ))}
          </select>
        </Field>
        <Field label="Path">
          <input
            value={path}
            onChange={(e) => setPath(e.target.value)}
            placeholder="/api/…"
            className={inputCls}
          />
        </Field>
        <Field label="Since">
          <input
            type="datetime-local"
            value={since}
            onChange={(e) => setSince(e.target.value)}
            className={inputCls}
          />
        </Field>
        <div className="flex items-end gap-2 sm:col-span-2 lg:col-span-5">
          <button
            type="submit"
            className="rounded-md bg-accent/20 px-4 py-2 text-sm text-accent transition hover:bg-accent/30"
          >
            Apply
          </button>
          {anyFilter && (
            <button
              type="button"
              onClick={clearFilters}
              className="rounded-md border border-edge px-3 py-2 text-sm text-slate-400 transition hover:text-slate-200"
            >
              Clear
            </button>
          )}
        </div>
      </form>

      <div className="rounded-xl border border-edge bg-panel">
        <div className="flex items-center justify-between border-b border-edge px-4 py-2.5">
          <span className="text-sm font-medium text-slate-200">
            Entries
            {!loading && (
              <span className="ml-2 text-xs text-slate-500">
                {filtered.length}
                {hasClientFilters ? ` of ${rows.length}` : ""}
                {rows.length >= DEFAULT_LIMIT ? " (page)" : ""} shown
              </span>
            )}
          </span>
        </div>

        {error ? (
          <div className="px-4 py-2">
            <ErrorRow text={error} onRetry={() => load(actorQuery)} />
          </div>
        ) : loading ? (
          <div className="px-4 py-10 text-center text-sm text-slate-500">
            Loading audit log…
          </div>
        ) : filtered.length === 0 ? (
          <div className="px-4 py-10 text-center text-sm text-slate-500">
            {anyFilter
              ? "No audit entries match these filters."
              : "No audit entries recorded yet."}
          </div>
        ) : (
          <div className="overflow-x-auto">
            <table className="w-full border-collapse text-sm">
              <thead>
                <tr className="text-left text-xs uppercase tracking-wide text-slate-500">
                  <th className="px-4 py-2 font-medium">When</th>
                  <th className="px-4 py-2 font-medium">Actor</th>
                  <th className="px-4 py-2 font-medium">Method</th>
                  <th className="px-4 py-2 font-medium">Path</th>
                  <th className="px-4 py-2 text-right font-medium">Status</th>
                  <th className="px-4 py-2 font-medium">Summary</th>
                </tr>
              </thead>
              <tbody>
                {filtered.map((e) => (
                  <AuditRow
                    key={e.id}
                    e={e}
                    isOpen={expanded === e.id}
                    onToggle={() =>
                      setExpanded((cur) => (cur === e.id ? null : e.id))
                    }
                  />
                ))}
              </tbody>
            </table>
          </div>
        )}
      </div>
    </div>
  );
}

function AuditRow({
  e,
  isOpen,
  onToggle,
}: {
  e: AuditEntry;
  isOpen: boolean;
  onToggle: () => void;
}) {
  return (
    <>
      <tr
        onClick={onToggle}
        className="cursor-pointer border-t border-edge align-top transition hover:bg-edge/40"
      >
        <td className="whitespace-nowrap px-4 py-2.5 text-xs text-slate-400">
          {fmtDate(e.ts)}
        </td>
        <td className="whitespace-nowrap px-4 py-2.5 text-xs text-slate-300">
          {e.actor ?? <span className="text-slate-500">—</span>}
          {e.actor_kind && (
            <span className="ml-2 rounded bg-edge px-1.5 py-0.5 text-[10px] uppercase tracking-wide text-slate-400">
              {e.actor_kind}
            </span>
          )}
        </td>
        <td className="whitespace-nowrap px-4 py-2.5">
          {e.method ? (
            <span className="rounded bg-edge px-2 py-0.5 text-[11px] text-slate-300">
              {e.method}
            </span>
          ) : (
            <span className="text-slate-500">—</span>
          )}
        </td>
        <td className="px-4 py-2.5 font-mono text-xs text-slate-200">
          {e.path ?? "—"}
        </td>
        <td
          className={`whitespace-nowrap px-4 py-2.5 text-right text-xs ${statusCls(
            e.status,
          )}`}
        >
          {e.status ?? "—"}
        </td>
        <td className="px-4 py-2.5 text-slate-300">
          {e.summary ?? <span className="text-slate-500">—</span>}
        </td>
      </tr>
      {isOpen && (
        <tr className="border-t border-edge bg-bg/40">
          <td colSpan={6} className="px-4 py-3">
            <div className="flex flex-col gap-3">
              <div className="flex flex-wrap gap-4 text-[11px] text-slate-500">
                <span>id: {e.id}</span>
                {e.query && <span>query: {e.query}</span>}
                {e.credential_id != null && (
                  <span>credential: {e.credential_id}</span>
                )}
              </div>
              <div>
                <div className="mb-1 text-[11px] uppercase tracking-wide text-slate-500">
                  Body excerpt
                </div>
                <pre className="max-h-64 overflow-auto whitespace-pre-wrap break-words rounded-md border border-edge bg-panel p-3 text-xs text-slate-200">
                  {e.body_excerpt ?? "(no body recorded)"}
                </pre>
              </div>
            </div>
          </td>
        </tr>
      )}
    </>
  );
}
