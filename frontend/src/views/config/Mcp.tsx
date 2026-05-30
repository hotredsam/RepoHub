// P16 — MCP servers config unit.
//
// Lists / adds / removes MCP server entries for a scope via /api/config/mcp.
//   - scope = "global" -> ~/.claude/settings.json mcpServers (live).
//   - scope = "repo"   -> the repo's .mcp.json on the repohub-staging branch
//                         (committed for review, NOT live in the working tree).
//
// One server entry is free JSON; the common shape is { command, args, env }.
// We render a structured editor for that shape and fall back to a raw-JSON view
// for anything else (url-based servers, extra keys, …). Styling mirrors
// Settings.tsx (bg-panel / border-edge / text-accent / text-slate-200|400).

import { useCallback, useEffect, useMemo, useState } from "react";
import { getMcp, putMcp, deleteMcp } from "../../lib/configApi";
import type { McpServer, McpResponse } from "../../lib/configTypes";

interface McpProps {
  scope: string;
  repoId?: number | null;
}

type Banner = { kind: "ok" | "err"; text: string } | null;

// The conventional structured shape of a stdio MCP server entry.
interface ServerForm {
  command: string;
  // One CLI arg per line (joined back into an array on save).
  args: string;
  // KEY=VALUE per line.
  env: string;
}

const EMPTY_FORM: ServerForm = { command: "", args: "", env: "" };

// ---- (de)serialise the structured form <-> the free-JSON server value ----

function argsToText(value: unknown): string {
  if (Array.isArray(value)) return value.map((a) => String(a)).join("\n");
  return "";
}

function envToText(value: unknown): string {
  if (value && typeof value === "object" && !Array.isArray(value)) {
    return Object.entries(value as Record<string, unknown>)
      .map(([k, v]) => `${k}=${String(v)}`)
      .join("\n");
  }
  return "";
}

function textToArgs(text: string): string[] {
  return text
    .split("\n")
    .map((l) => l.trim())
    .filter((l) => l.length > 0);
}

function textToEnv(text: string): Record<string, string> {
  const out: Record<string, string> = {};
  for (const raw of text.split("\n")) {
    const line = raw.trim();
    if (!line) continue;
    const eq = line.indexOf("=");
    if (eq === -1) continue;
    const key = line.slice(0, eq).trim();
    if (key) out[key] = line.slice(eq + 1).trim();
  }
  return out;
}

// Build the flat MCP upsert payload (command/args/env) from the form.
function buildServerFields(form: ServerForm): {
  command: string;
  args: string[];
  env: Record<string, string>;
} {
  return {
    command: form.command.trim(),
    args: textToArgs(form.args),
    env: textToEnv(form.env),
  };
}

// A one-line summary of a server: its command + args.
function describeServer(server: McpServer): string {
  return [server.command, ...(server.args ?? [])].join(" ").trim() || "(no command)";
}

export default function Mcp({ scope, repoId }: McpProps) {
  const isRepo = scope === "repo";

  const [servers, setServers] = useState<McpServer[] | null>(null);
  const [respScope, setRespScope] = useState<string>(scope);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [banner, setBanner] = useState<Banner>(null);

  // ---- add form ----
  const [newName, setNewName] = useState("");
  const [form, setForm] = useState<ServerForm>(EMPTY_FORM);
  const [saving, setSaving] = useState(false);

  // ---- per-entry busy / edit state ----
  const [busy, setBusy] = useState<Record<string, "save" | "delete">>({});
  const [editing, setEditing] = useState<Record<string, ServerForm>>({});

  const load = useCallback(() => {
    setLoading(true);
    setError(null);
    getMcp(scope, repoId ?? undefined)
      .then((res: McpResponse) => {
        setServers(res.servers ?? []);
        setRespScope(res.scope);
      })
      .catch((e) => setError(e instanceof Error ? e.message : String(e)))
      .finally(() => setLoading(false));
  }, [scope, repoId]);

  useEffect(() => {
    load();
  }, [load]);

  const entries = useMemo(
    () => [...(servers ?? [])].sort((a, b) => a.name.localeCompare(b.name)),
    [servers],
  );

  const setField = (key: keyof ServerForm, value: string) =>
    setForm((f) => ({ ...f, [key]: value }));

  // ---- add a new server ----
  const addServer = async (e: React.FormEvent) => {
    e.preventDefault();
    const name = newName.trim();
    if (!name) {
      setBanner({ kind: "err", text: "Server name is required." });
      return;
    }
    if (!form.command.trim()) {
      setBanner({ kind: "err", text: "Command is required." });
      return;
    }
    if (servers && servers.some((s) => s.name === name)) {
      setBanner({ kind: "err", text: `A server named "${name}" already exists.` });
      return;
    }
    setSaving(true);
    setBanner(null);
    try {
      const res = await putMcp({
        scope,
        repo_id: repoId ?? undefined,
        name,
        ...buildServerFields(form),
      });
      setServers(res.servers ?? []);
      setNewName("");
      setForm(EMPTY_FORM);
      setBanner({
        kind: "ok",
        text: isRepo
          ? `Added "${name}" to .mcp.json on the staging branch.`
          : `Added "${name}".`,
      });
    } catch (err) {
      setBanner({ kind: "err", text: err instanceof Error ? err.message : String(err) });
    } finally {
      setSaving(false);
    }
  };

  // ---- begin / cancel / commit editing an existing server ----
  const beginEdit = (server: McpServer) => {
    setBanner(null);
    setEditing((m) => ({
      ...m,
      [server.name]: {
        command: server.command,
        args: argsToText(server.args),
        env: envToText(server.env),
      },
    }));
  };

  const cancelEdit = (name: string) =>
    setEditing((m) => {
      const next = { ...m };
      delete next[name];
      return next;
    });

  const setEditField = (name: string, key: keyof ServerForm, value: string) =>
    setEditing((m) => ({ ...m, [name]: { ...m[name], [key]: value } }));

  const saveEdit = async (name: string) => {
    const draft = editing[name];
    if (!draft) return;
    if (!draft.command.trim()) {
      setBanner({ kind: "err", text: "Command is required." });
      return;
    }
    setBusy((b) => ({ ...b, [name]: "save" }));
    setBanner(null);
    try {
      const res = await putMcp({
        scope,
        repo_id: repoId ?? undefined,
        name,
        ...buildServerFields(draft),
      });
      setServers(res.servers ?? []);
      cancelEdit(name);
      setBanner({ kind: "ok", text: `Updated "${name}".` });
    } catch (err) {
      setBanner({ kind: "err", text: err instanceof Error ? err.message : String(err) });
    } finally {
      setBusy((b) => {
        const next = { ...b };
        delete next[name];
        return next;
      });
    }
  };

  // ---- remove a server ----
  const removeServer = async (name: string) => {
    setBusy((b) => ({ ...b, [name]: "delete" }));
    setBanner(null);
    try {
      const res = await deleteMcp({ scope, repo_id: repoId ?? undefined, name });
      setServers(res.servers ?? []);
      cancelEdit(name);
      setBanner({ kind: "ok", text: `Removed "${name}".` });
    } catch (err) {
      setBanner({ kind: "err", text: err instanceof Error ? err.message : String(err) });
    } finally {
      setBusy((b) => {
        const next = { ...b };
        delete next[name];
        return next;
      });
    }
  };

  return (
    <section className="rounded-xl border border-edge bg-panel p-6">
      <div className="flex items-center justify-between">
        <h2 className="text-lg font-semibold">MCP servers</h2>
        <span className="rounded bg-edge px-1.5 py-0.5 text-[11px] uppercase tracking-wide text-slate-400">
          {respScope}
          {isRepo && repoId != null ? ` · repo ${repoId}` : ""}
        </span>
      </div>
      <p className="mt-1 text-xs text-slate-500">
        {isRepo ? (
          <>
            Servers for this repo land in{" "}
            <code className="text-slate-400">.mcp.json</code> on the{" "}
            <span className="text-accent">repohub-staging</span> branch for review
            — not in the live working tree.
          </>
        ) : (
          <>
            Global servers in{" "}
            <code className="text-slate-400">~/.claude/settings.json</code>. Changes
            are snapshotted to git, then merged — applied live.
          </>
        )}
      </p>

      {/* ---- add form ---- */}
      <form
        onSubmit={addServer}
        className="mt-4 grid gap-3 sm:grid-cols-2"
      >
        <Field label="Name">
          <input
            value={newName}
            onChange={(e) => setNewName(e.target.value)}
            placeholder="filesystem"
            className={inputCls}
          />
        </Field>
        <Field label="Command">
          <input
            value={form.command}
            onChange={(e) => setField("command", e.target.value)}
            placeholder="npx"
            className={inputCls}
          />
        </Field>
        <Field label="Args (one per line)">
          <textarea
            value={form.args}
            onChange={(e) => setField("args", e.target.value)}
            placeholder={"-y\n@modelcontextprotocol/server-filesystem\n/path"}
            rows={3}
            className={`${inputCls} font-mono`}
          />
        </Field>
        <Field label="Env (KEY=VALUE per line)">
          <textarea
            value={form.env}
            onChange={(e) => setField("env", e.target.value)}
            placeholder={"API_BASE=https://example.com"}
            rows={3}
            className={`${inputCls} font-mono`}
          />
        </Field>
        <div className="sm:col-span-2">
          <button
            type="submit"
            disabled={saving}
            className="rounded-md bg-accent/20 px-4 py-2 text-sm text-accent transition hover:bg-accent/30 disabled:cursor-not-allowed disabled:opacity-40"
          >
            {saving ? "Adding…" : "Add server"}
          </button>
        </div>
      </form>

      {banner && <BannerRow banner={banner} />}

      {/* ---- list ---- */}
      <div className="mt-5">
        {loading ? (
          <p className="text-sm text-slate-400">Loading MCP servers…</p>
        ) : error ? (
          <ErrorRow text={error} onRetry={load} />
        ) : entries.length === 0 ? (
          <p className="rounded-lg border border-dashed border-edge px-4 py-6 text-center text-sm text-slate-500">
            No MCP servers configured for this scope.
          </p>
        ) : (
          <ul className="space-y-2">
            {entries.map((server) => {
              const name = server.name;
              const entryBusy = busy[name];
              const draft = editing[name];
              return (
                <li
                  key={name}
                  className="rounded-lg border border-edge bg-slate-900/40 p-3"
                >
                  <div className="flex flex-wrap items-center gap-3">
                    <span className="font-medium text-slate-200">{name}</span>
                    <span className="truncate text-sm text-slate-400">
                      {describeServer(server)}
                    </span>
                    <div className="ml-auto flex gap-2">
                      {!draft && (
                        <button
                          onClick={() => beginEdit(server)}
                          disabled={!!entryBusy}
                          className="rounded-md border border-edge px-2.5 py-1 text-xs text-slate-300 transition hover:bg-edge disabled:opacity-40"
                        >
                          Edit
                        </button>
                      )}
                      <button
                        onClick={() => removeServer(name)}
                        disabled={!!entryBusy}
                        className="rounded-md border border-rose-500/40 px-2.5 py-1 text-xs text-rose-400 transition hover:bg-rose-500/10 disabled:opacity-40"
                      >
                        {entryBusy === "delete" ? "Removing…" : "Remove"}
                      </button>
                    </div>
                  </div>

                  {/* inline edit form */}
                  {draft && (
                    <div className="mt-3 grid gap-3 border-t border-edge pt-3 sm:grid-cols-2">
                      <Field label="Command">
                        <input
                          value={draft.command}
                          onChange={(e) =>
                            setEditField(name, "command", e.target.value)
                          }
                          className={inputCls}
                        />
                      </Field>
                      <div className="hidden sm:block" />
                      <Field label="Args (one per line)">
                        <textarea
                          value={draft.args}
                          onChange={(e) =>
                            setEditField(name, "args", e.target.value)
                          }
                          rows={3}
                          className={`${inputCls} font-mono`}
                        />
                      </Field>
                      <Field label="Env (KEY=VALUE per line)">
                        <textarea
                          value={draft.env}
                          onChange={(e) =>
                            setEditField(name, "env", e.target.value)
                          }
                          rows={3}
                          className={`${inputCls} font-mono`}
                        />
                      </Field>
                      <div className="flex gap-2 sm:col-span-2">
                        <button
                          onClick={() => saveEdit(name)}
                          disabled={entryBusy === "save"}
                          className="rounded-md bg-accent/20 px-3 py-1.5 text-sm text-accent transition hover:bg-accent/30 disabled:cursor-not-allowed disabled:opacity-40"
                        >
                          {entryBusy === "save" ? "Saving…" : "Save"}
                        </button>
                        <button
                          onClick={() => cancelEdit(name)}
                          disabled={entryBusy === "save"}
                          className="rounded-md border border-edge px-3 py-1.5 text-sm text-slate-300 transition hover:bg-edge disabled:opacity-40"
                        >
                          Cancel
                        </button>
                      </div>
                    </div>
                  )}
                </li>
              );
            })}
          </ul>
        )}
      </div>
    </section>
  );
}

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
    <label className="flex min-w-[8rem] flex-1 flex-col gap-1.5">
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
