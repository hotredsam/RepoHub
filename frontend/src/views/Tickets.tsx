import { useCallback, useEffect, useMemo, useState } from "react";
import { listRepos } from "../lib/api";
import type { Repo } from "../lib/types";

// Tickets.tsx (P17)
// A ticket board over REAL GitHub issues. The backend (tickets.rs) shells out to
// the `gh` CLI to list/create/comment/close/reopen issues for a tracked repo;
// this view is the UI over those endpoints. Listing is read-only; mutations go
// through explicit per-card actions or the New ticket form.
//
// API contract is kept local to this unit (mirrors backend tickets.rs), matching
// the pattern used by Connections.tsx — api.ts is owned elsewhere and untouched.

// ---- API contract (mirrors backend tickets.rs JSON shapes) ----

// One issue as normalized by the backend (tickets.rs `Ticket`) from
// `gh issue list/view --json ...`. Labels and assignees are plain strings
// (label names / assignee logins) — the backend does not surface label colors,
// author, comment counts, or created/closed timestamps.
interface Ticket {
  number: number;
  // owner/name the issue belongs to (so cards can show their repo on an
  // all-repos board and actions can target the right repo).
  repo: string;
  title: string;
  body: string | null;
  state: string; // "open" | "closed"
  labels: string[];
  assignees: string[];
  updated_at: string;
  url: string;
}

type StateFilter = "open" | "closed" | "all";

interface ListTicketsQuery {
  repo: string;
  state: StateFilter;
  label?: string;
  assignee?: string;
}

interface CreateTicketBody {
  repo: string;
  title: string;
  body: string;
  labels: string[];
}

// Comment/state target the issue via path params (owner/name/number); the JSON
// body carries only the payload, matching backend CommentBody / StateBody.
interface CommentTicketBody {
  body: string;
}

interface SetTicketStateBody {
  state: "open" | "closed";
  labels_add?: string[];
  labels_remove?: string[];
}

// Split a `owner/name` full_name into its two path segments. Splits on the
// FIRST '/' only (repo names cannot contain '/', owners cannot either).
function splitRepo(repo: string): { owner: string; name: string } {
  const idx = repo.indexOf("/");
  if (idx < 0) return { owner: repo, name: "" };
  return { owner: repo.slice(0, idx), name: repo.slice(idx + 1) };
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

function qs(params: Record<string, string | undefined | null>): string {
  const sp = new URLSearchParams();
  for (const [k, v] of Object.entries(params)) {
    if (v !== undefined && v !== null && v !== "") sp.set(k, v);
  }
  const s = sp.toString();
  return s ? `?${s}` : "";
}

const listTickets = (q: ListTicketsQuery) =>
  request<Ticket[]>(
    `/api/tickets${qs({
      repo: q.repo,
      state: q.state,
      label: q.label,
      assignee: q.assignee,
    })}`,
  );

const createTicket = (body: CreateTicketBody) =>
  request<Ticket>("/api/tickets", {
    method: "POST",
    body: JSON.stringify(body),
  });

// Target the real path-param routes: /api/tickets/:owner/:name/:number/comment.
// owner and name are emitted as two SEPARATE encoded segments (never one
// encoded "owner%2Fname"), matching the backend's :repo_owner/:repo_name route.
const commentTicket = (repo: string, number: number, body: CommentTicketBody) => {
  const { owner, name } = splitRepo(repo);
  return request<Ticket>(
    `/api/tickets/${encodeURIComponent(owner)}/${encodeURIComponent(name)}/${number}/comment`,
    { method: "POST", body: JSON.stringify(body) },
  );
};

const setTicketState = (
  repo: string,
  number: number,
  body: SetTicketStateBody,
) => {
  const { owner, name } = splitRepo(repo);
  return request<Ticket>(
    `/api/tickets/${encodeURIComponent(owner)}/${encodeURIComponent(name)}/${number}/state`,
    { method: "POST", body: JSON.stringify(body) },
  );
};

// ---- helpers ----

function fmtDate(iso: string | null): string {
  if (!iso) return "—";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  return d.toLocaleString();
}

function relTime(iso: string | null): string {
  if (!iso) return "—";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  const secs = Math.round((Date.now() - d.getTime()) / 1000);
  const abs = Math.abs(secs);
  const units: [number, string][] = [
    [60, "s"],
    [3600, "m"],
    [86400, "h"],
    [2592000, "d"],
    [31536000, "mo"],
  ];
  let value = secs;
  let suffix = "y";
  for (let i = 0; i < units.length; i++) {
    const [limit, label] = units[i];
    if (abs < limit) {
      const div = i === 0 ? 1 : units[i - 1][0];
      value = Math.round(secs / div);
      suffix = label;
      break;
    }
  }
  if (abs >= 31536000) value = Math.round(secs / 31536000);
  return value <= 0 ? `${-value}${suffix} ago` : `in ${value}${suffix}`;
}

function parseLabels(input: string): string[] {
  return input
    .split(",")
    .map((s) => s.trim())
    .filter(Boolean);
}

// ---- main view ----

export default function Tickets() {
  const [repos, setRepos] = useState<Repo[]>([]);
  const [reposError, setReposError] = useState<string | null>(null);

  // Filters.
  const [repoFilter, setRepoFilter] = useState<string>(""); // "" = pick a repo
  const [stateFilter, setStateFilter] = useState<StateFilter>("open");
  const [labelFilter, setLabelFilter] = useState("");
  const [assigneeFilter, setAssigneeFilter] = useState("");
  // Committed filter values that drive the active query.
  const [activeLabel, setActiveLabel] = useState("");
  const [activeAssignee, setActiveAssignee] = useState("");

  const [tickets, setTickets] = useState<Ticket[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const [showNew, setShowNew] = useState(false);

  // Tracked repos only — issues live on real GitHub repos we manage.
  const trackedRepos = useMemo(
    () => repos.filter((r) => r.tracked),
    [repos],
  );

  const loadRepos = useCallback(async () => {
    try {
      const rs = await listRepos();
      setRepos(rs);
      setReposError(null);
      // Default the repo filter to the first tracked repo so the board is useful
      // immediately, without mutating anything.
      setRepoFilter((cur) => {
        if (cur) return cur;
        const first = rs.find((r) => r.tracked);
        return first ? first.full_name : "";
      });
    } catch (e) {
      setReposError(e instanceof Error ? e.message : String(e));
    }
  }, []);

  const loadTickets = useCallback(
    async (q: ListTicketsQuery) => {
      if (!q.repo) {
        setTickets([]);
        return;
      }
      setLoading(true);
      setError(null);
      try {
        const data = await listTickets(q);
        setTickets(data);
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
        setTickets([]);
      } finally {
        setLoading(false);
      }
    },
    [],
  );

  useEffect(() => {
    void loadRepos();
  }, [loadRepos]);

  // Re-fetch whenever the repo/state/committed filters change.
  useEffect(() => {
    void loadTickets({
      repo: repoFilter,
      state: stateFilter,
      label: activeLabel || undefined,
      assignee: activeAssignee || undefined,
    });
  }, [repoFilter, stateFilter, activeLabel, activeAssignee, loadTickets]);

  const refresh = useCallback(() => {
    void loadTickets({
      repo: repoFilter,
      state: stateFilter,
      label: activeLabel || undefined,
      assignee: activeAssignee || undefined,
    });
  }, [repoFilter, stateFilter, activeLabel, activeAssignee, loadTickets]);

  const applyTextFilters = (e: React.FormEvent) => {
    e.preventDefault();
    setActiveLabel(labelFilter.trim());
    setActiveAssignee(assigneeFilter.trim());
  };

  const clearTextFilters = () => {
    setLabelFilter("");
    setAssigneeFilter("");
    setActiveLabel("");
    setActiveAssignee("");
  };

  // Optimistically replace a ticket after a mutation returns the updated issue.
  const upsertTicket = useCallback(
    (t: Ticket) => {
      // If the new state no longer matches the active filter, drop it from view;
      // otherwise replace/insert it.
      setTickets((cur) => {
        const matchesState =
          stateFilter === "all" || t.state === stateFilter;
        const without = cur.filter(
          (x) => !(x.number === t.number && x.repo === t.repo),
        );
        return matchesState
          ? [t, ...without].sort(
              (a, b) =>
                new Date(b.updated_at).getTime() -
                new Date(a.updated_at).getTime(),
            )
          : without;
      });
    },
    [stateFilter],
  );

  const open = useMemo(
    () => tickets.filter((t) => t.state === "open"),
    [tickets],
  );
  const closed = useMemo(
    () => tickets.filter((t) => t.state !== "open"),
    [tickets],
  );

  const hasRepoSelected = Boolean(repoFilter);

  return (
    <div className="mx-auto flex max-w-6xl flex-col gap-5">
      <div className="flex flex-wrap items-center gap-3">
        <h1 className="text-xl font-semibold">Tickets</h1>
        <span className="text-sm text-slate-500">
          Backed by real GitHub issues — also visible on github.com.
        </span>
        <div className="ml-auto flex items-center gap-3">
          <button
            onClick={refresh}
            disabled={!hasRepoSelected || loading}
            className="rounded-md border border-edge bg-panel px-3 py-1.5 text-sm text-slate-200 transition hover:border-accent/60 hover:text-accent disabled:cursor-not-allowed disabled:opacity-50"
          >
            {loading ? "Refreshing…" : "Refresh"}
          </button>
          <button
            onClick={() => setShowNew((s) => !s)}
            disabled={trackedRepos.length === 0}
            className="rounded-md bg-accent/20 px-4 py-1.5 text-sm text-accent transition hover:bg-accent/30 disabled:cursor-not-allowed disabled:opacity-50"
          >
            {showNew ? "Close form" : "New ticket"}
          </button>
        </div>
      </div>

      {reposError && (
        <div className="rounded-md border border-rose-500/40 bg-rose-500/10 px-4 py-2 text-sm text-rose-300">
          Failed to load repos: {reposError}
        </div>
      )}

      {/* Filters */}
      <form
        onSubmit={applyTextFilters}
        className="flex flex-wrap items-end gap-3 rounded-xl border border-edge bg-panel p-4"
      >
        <Field label="Repo">
          <select
            value={repoFilter}
            onChange={(e) => setRepoFilter(e.target.value)}
            className="rounded-md border border-edge bg-bg px-3 py-2 text-sm text-slate-200 focus:border-accent/60 focus:outline-none"
          >
            <option value="">Select a tracked repo…</option>
            {trackedRepos.map((r) => (
              <option key={r.id} value={r.full_name}>
                {r.full_name}
              </option>
            ))}
          </select>
        </Field>

        <Field label="State">
          <select
            value={stateFilter}
            onChange={(e) => setStateFilter(e.target.value as StateFilter)}
            className="rounded-md border border-edge bg-bg px-3 py-2 text-sm text-slate-200 focus:border-accent/60 focus:outline-none"
          >
            <option value="open">Open</option>
            <option value="closed">Closed</option>
            <option value="all">All</option>
          </select>
        </Field>

        <Field label="Label">
          <input
            type="text"
            value={labelFilter}
            onChange={(e) => setLabelFilter(e.target.value)}
            placeholder="bug"
            className="w-36 rounded-md border border-edge bg-bg px-3 py-2 text-sm text-slate-200 placeholder:text-slate-500 focus:border-accent/60 focus:outline-none"
          />
        </Field>

        <Field label="Assignee">
          <input
            type="text"
            value={assigneeFilter}
            onChange={(e) => setAssigneeFilter(e.target.value)}
            placeholder="login"
            className="w-36 rounded-md border border-edge bg-bg px-3 py-2 text-sm text-slate-200 placeholder:text-slate-500 focus:border-accent/60 focus:outline-none"
          />
        </Field>

        <div className="flex items-center gap-2">
          <button
            type="submit"
            className="rounded-md bg-accent/20 px-4 py-2 text-sm text-accent transition hover:bg-accent/30"
          >
            Apply
          </button>
          {(activeLabel || activeAssignee || labelFilter || assigneeFilter) && (
            <button
              type="button"
              onClick={clearTextFilters}
              className="rounded-md border border-edge px-3 py-2 text-sm text-slate-400 transition hover:text-slate-200"
            >
              Clear
            </button>
          )}
        </div>
      </form>

      {showNew && (
        <NewTicketForm
          repos={trackedRepos}
          defaultRepo={repoFilter}
          onCreated={(t) => {
            // Jump the board to the created ticket's repo + ensure it is visible.
            if (t.repo !== repoFilter) setRepoFilter(t.repo);
            if (stateFilter === "closed") setStateFilter("open");
            upsertTicket(t);
            setShowNew(false);
          }}
          onCancel={() => setShowNew(false)}
        />
      )}

      {/* Board */}
      {!hasRepoSelected ? (
        <div className="rounded-xl border border-edge bg-panel px-4 py-12 text-center text-sm text-slate-500">
          {trackedRepos.length === 0
            ? "No tracked repos yet. Track a repo to manage its tickets."
            : "Select a tracked repo to view its tickets."}
        </div>
      ) : error ? (
        <div className="rounded-xl border border-edge bg-panel px-4 py-10 text-center text-sm text-rose-400">
          {error}
          <div className="mt-3">
            <button
              onClick={refresh}
              className="rounded-md border border-edge px-3 py-1.5 text-xs text-slate-300 hover:text-slate-100"
            >
              Retry
            </button>
          </div>
        </div>
      ) : loading ? (
        <div className="rounded-xl border border-edge bg-panel px-4 py-12 text-center text-sm text-slate-500">
          Loading tickets…
        </div>
      ) : tickets.length === 0 ? (
        <div className="rounded-xl border border-edge bg-panel px-4 py-12 text-center text-sm text-slate-500">
          No tickets match these filters.
        </div>
      ) : (
        <div className="grid grid-cols-1 gap-4 lg:grid-cols-2">
          <Column
            title="Open"
            accent="emerald"
            tickets={open}
            onMutated={upsertTicket}
            onError={setError}
          />
          <Column
            title="Closed"
            accent="slate"
            tickets={closed}
            onMutated={upsertTicket}
            onError={setError}
          />
        </div>
      )}
    </div>
  );
}

function Field({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}) {
  return (
    <label className="flex flex-col gap-1">
      <span className="text-[11px] uppercase tracking-wide text-slate-500">
        {label}
      </span>
      {children}
    </label>
  );
}

function Column({
  title,
  accent,
  tickets,
  onMutated,
  onError,
}: {
  title: string;
  accent: "emerald" | "slate";
  tickets: Ticket[];
  onMutated: (t: Ticket) => void;
  onError: (msg: string) => void;
}) {
  const dot = accent === "emerald" ? "bg-emerald-400" : "bg-slate-500";
  return (
    <div className="flex flex-col gap-3 rounded-xl border border-edge bg-bg/40 p-3">
      <div className="flex items-center gap-2 px-1">
        <span className={`h-2 w-2 rounded-full ${dot}`} />
        <span className="text-sm font-medium text-slate-200">{title}</span>
        <span className="text-xs text-slate-500">{tickets.length}</span>
      </div>
      {tickets.length === 0 ? (
        <div className="rounded-lg border border-dashed border-edge px-3 py-8 text-center text-xs text-slate-600">
          None
        </div>
      ) : (
        tickets.map((t) => (
          <TicketCard
            key={`${t.repo}#${t.number}`}
            ticket={t}
            onMutated={onMutated}
            onError={onError}
          />
        ))
      )}
    </div>
  );
}

function TicketCard({
  ticket,
  onMutated,
  onError,
}: {
  ticket: Ticket;
  onMutated: (t: Ticket) => void;
  onError: (msg: string) => void;
}) {
  const [expanded, setExpanded] = useState(false);
  const [commenting, setCommenting] = useState(false);
  const [comment, setComment] = useState("");
  const [busy, setBusy] = useState<null | "comment" | "state">(null);

  const isOpen = ticket.state === "open";

  const submitComment = async (e: React.FormEvent) => {
    e.preventDefault();
    const body = comment.trim();
    if (!body) return;
    setBusy("comment");
    try {
      const updated = await commentTicket(ticket.repo, ticket.number, { body });
      onMutated(updated);
      setComment("");
      setCommenting(false);
    } catch (err) {
      onError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(null);
    }
  };

  const toggleState = async () => {
    setBusy("state");
    try {
      const updated = await setTicketState(ticket.repo, ticket.number, {
        state: isOpen ? "closed" : "open",
      });
      onMutated(updated);
    } catch (err) {
      onError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(null);
    }
  };

  return (
    <div className="rounded-lg border border-edge bg-panel p-3">
      <div className="flex items-start gap-2">
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-2 text-[11px] text-slate-500">
            <span className="text-slate-400">{ticket.repo}</span>
            <span>#{ticket.number}</span>
          </div>
          <button
            onClick={() => setExpanded((s) => !s)}
            className="mt-1 block w-full text-left text-sm font-medium text-slate-100 hover:text-accent"
          >
            {ticket.title}
          </button>
        </div>
        <a
          href={ticket.url}
          target="_blank"
          rel="noreferrer"
          title="Open on GitHub"
          className="shrink-0 rounded-md border border-edge px-2 py-1 text-[11px] text-slate-400 transition hover:border-accent/60 hover:text-accent"
        >
          GitHub ↗
        </a>
      </div>

      {(ticket.labels.length > 0 || ticket.assignees.length > 0) && (
        <div className="mt-2 flex flex-wrap items-center gap-1.5">
          {ticket.labels.map((l) => (
            <span
              key={l}
              className="rounded border border-edge bg-bg/50 px-2 py-0.5 text-[11px] font-medium text-slate-300"
            >
              {l}
            </span>
          ))}
          {ticket.assignees.map((a) => (
            <span
              key={a}
              className="rounded-full border border-edge px-2 py-0.5 text-[11px] text-slate-300"
            >
              @{a}
            </span>
          ))}
        </div>
      )}

      {expanded && ticket.body && (
        <pre className="mt-2 max-h-56 overflow-auto whitespace-pre-wrap break-words rounded-md border border-edge bg-bg/50 p-3 text-xs text-slate-300">
          {ticket.body}
        </pre>
      )}

      <div className="mt-2 flex flex-wrap items-center gap-3 text-[11px] text-slate-500">
        <span title={fmtDate(ticket.updated_at)}>
          updated {relTime(ticket.updated_at)}
        </span>
      </div>

      <div className="mt-3 flex flex-wrap items-center gap-2">
        <button
          onClick={() => setCommenting((s) => !s)}
          className="rounded-md border border-edge px-2.5 py-1 text-xs text-slate-300 transition hover:border-accent/60 hover:text-accent"
        >
          {commenting ? "Cancel comment" : "Comment"}
        </button>
        <button
          onClick={toggleState}
          disabled={busy === "state"}
          className="rounded-md border border-edge px-2.5 py-1 text-xs text-slate-300 transition hover:border-accent/60 hover:text-accent disabled:cursor-not-allowed disabled:opacity-50"
        >
          {busy === "state"
            ? isOpen
              ? "Closing…"
              : "Reopening…"
            : isOpen
              ? "Close"
              : "Reopen"}
        </button>
      </div>

      {commenting && (
        <form onSubmit={submitComment} className="mt-2 flex flex-col gap-2">
          <textarea
            value={comment}
            onChange={(e) => setComment(e.target.value)}
            placeholder="Add a comment…"
            rows={3}
            className="w-full resize-y rounded-md border border-edge bg-bg px-3 py-2 text-xs text-slate-200 placeholder:text-slate-500 focus:border-accent/60 focus:outline-none"
          />
          <div className="flex items-center gap-2">
            <button
              type="submit"
              disabled={busy === "comment" || !comment.trim()}
              className="rounded-md bg-accent/20 px-3 py-1 text-xs text-accent transition hover:bg-accent/30 disabled:cursor-not-allowed disabled:opacity-50"
            >
              {busy === "comment" ? "Posting…" : "Post comment"}
            </button>
          </div>
        </form>
      )}
    </div>
  );
}

function NewTicketForm({
  repos,
  defaultRepo,
  onCreated,
  onCancel,
}: {
  repos: Repo[];
  defaultRepo: string;
  onCreated: (t: Ticket) => void;
  onCancel: () => void;
}) {
  const [repo, setRepo] = useState(
    defaultRepo || (repos[0]?.full_name ?? ""),
  );
  const [title, setTitle] = useState("");
  const [body, setBody] = useState("");
  const [labels, setLabels] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    const trimmedTitle = title.trim();
    if (!repo || !trimmedTitle) {
      setError("Repo and title are required.");
      return;
    }
    setSubmitting(true);
    setError(null);
    try {
      const created = await createTicket({
        repo,
        title: trimmedTitle,
        body: body.trim(),
        labels: parseLabels(labels),
      });
      onCreated(created);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <form
      onSubmit={submit}
      className="flex flex-col gap-3 rounded-xl border border-accent/40 bg-panel p-4"
    >
      <div className="flex items-center gap-2">
        <h2 className="text-sm font-medium text-slate-200">New ticket</h2>
        <span className="text-xs text-slate-500">
          Creates a real GitHub issue.
        </span>
      </div>

      {error && (
        <div className="rounded-md border border-rose-500/40 bg-rose-500/10 px-3 py-2 text-xs text-rose-300">
          {error}
        </div>
      )}

      <div className="flex flex-wrap gap-3">
        <Field label="Repo">
          <select
            value={repo}
            onChange={(e) => setRepo(e.target.value)}
            className="rounded-md border border-edge bg-bg px-3 py-2 text-sm text-slate-200 focus:border-accent/60 focus:outline-none"
          >
            {repos.length === 0 && <option value="">No tracked repos</option>}
            {repos.map((r) => (
              <option key={r.id} value={r.full_name}>
                {r.full_name}
              </option>
            ))}
          </select>
        </Field>
        <Field label="Labels (comma-separated)">
          <input
            type="text"
            value={labels}
            onChange={(e) => setLabels(e.target.value)}
            placeholder="bug, p1"
            className="w-56 rounded-md border border-edge bg-bg px-3 py-2 text-sm text-slate-200 placeholder:text-slate-500 focus:border-accent/60 focus:outline-none"
          />
        </Field>
      </div>

      <label className="flex flex-col gap-1">
        <span className="text-[11px] uppercase tracking-wide text-slate-500">
          Title
        </span>
        <input
          type="text"
          value={title}
          onChange={(e) => setTitle(e.target.value)}
          placeholder="Short summary"
          className="rounded-md border border-edge bg-bg px-3 py-2 text-sm text-slate-200 placeholder:text-slate-500 focus:border-accent/60 focus:outline-none"
        />
      </label>

      <label className="flex flex-col gap-1">
        <span className="text-[11px] uppercase tracking-wide text-slate-500">
          Body
        </span>
        <textarea
          value={body}
          onChange={(e) => setBody(e.target.value)}
          placeholder="Describe the issue…"
          rows={5}
          className="w-full resize-y rounded-md border border-edge bg-bg px-3 py-2 text-sm text-slate-200 placeholder:text-slate-500 focus:border-accent/60 focus:outline-none"
        />
      </label>

      <div className="flex items-center gap-2">
        <button
          type="submit"
          disabled={submitting || !repo || !title.trim()}
          className="rounded-md bg-accent/20 px-4 py-2 text-sm text-accent transition hover:bg-accent/30 disabled:cursor-not-allowed disabled:opacity-50"
        >
          {submitting ? "Creating…" : "Create ticket"}
        </button>
        <button
          type="button"
          onClick={onCancel}
          className="rounded-md border border-edge px-3 py-2 text-sm text-slate-400 transition hover:text-slate-200"
        >
          Cancel
        </button>
      </div>
    </form>
  );
}
