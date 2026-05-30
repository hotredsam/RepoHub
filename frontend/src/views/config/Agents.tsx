import { useCallback, useEffect, useMemo, useState } from "react";
import { getAgents, putAgent, deleteAgent } from "../../lib/configApi";
import type { Agent } from "../../lib/configTypes";

// Props: which config layer this panel edits. `scope = "global"` writes
// ~/.claude/agents live; `scope = "repo"` (with repoId) writes the repo's
// .claude/agents on the repohub-staging branch for review.
interface AgentsProps {
  scope: string;
  repoId?: number | null;
}

type Banner = { kind: "ok" | "err"; text: string } | null;

// A blank agent for the "new agent" draft.
const EMPTY_AGENT: Agent = {
  name: "",
  description: "",
  model: "",
  tools: "",
  body: "",
};

const inputCls =
  "w-full rounded-md border border-edge bg-slate-900/60 px-3 py-2 text-sm text-slate-200 outline-none focus:border-accent";

export default function Agents({ scope, repoId }: AgentsProps) {
  const isRepo = scope === "repo";
  const repo_id = isRepo ? (repoId ?? null) : null;

  const [agents, setAgents] = useState<Agent[] | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [banner, setBanner] = useState<Banner>(null);

  // The agent currently open in the editor. `null` = nothing open;
  // a draft whose name is not in the list = creating a new one.
  const [editing, setEditing] = useState<Agent | null>(null);
  // The original name when editing an existing agent (so a rename can be
  // detected — we delete the old file after writing the renamed one).
  const [editingOriginalName, setEditingOriginalName] = useState<string | null>(
    null,
  );
  const [saving, setSaving] = useState(false);
  const [busy, setBusy] = useState<Record<string, "delete">>({});

  const load = useCallback(() => {
    setLoading(true);
    setError(null);
    getAgents(scope, repo_id ?? undefined)
      .then((res) => setAgents(res.agents))
      .catch((e) => setError(e instanceof Error ? e.message : String(e)))
      .finally(() => setLoading(false));
  }, [scope, repo_id]);

  useEffect(() => {
    // Reset transient editor state when the layer changes.
    setEditing(null);
    setEditingOriginalName(null);
    setBanner(null);
    load();
  }, [load]);

  const editingExistingNames = useMemo(
    () => new Set((agents ?? []).map((a) => a.name)),
    [agents],
  );

  const startNew = () => {
    setEditing({ ...EMPTY_AGENT });
    setEditingOriginalName(null);
    setBanner(null);
  };

  const startEdit = (agent: Agent) => {
    // Normalise nullable wire fields to strings for the controlled inputs.
    setEditing({
      name: agent.name,
      description: agent.description ?? "",
      model: agent.model ?? "",
      tools: agent.tools ?? "",
      body: agent.body,
    });
    setEditingOriginalName(agent.name);
    setBanner(null);
  };

  const cancelEdit = () => {
    setEditing(null);
    setEditingOriginalName(null);
  };

  const setField = <K extends keyof Agent>(key: K, value: Agent[K]) =>
    setEditing((a) => (a ? { ...a, [key]: value } : a));

  const save = async () => {
    if (!editing) return;
    const name = editing.name.trim();
    if (!name) {
      setBanner({ kind: "err", text: "Agent name is required." });
      return;
    }
    if (/[/\\]|\.\./.test(name)) {
      setBanner({
        kind: "err",
        text: "Name cannot contain '/', '\\', or '..'.",
      });
      return;
    }
    setSaving(true);
    setBanner(null);
    // Trim/normalise: empty optional fields become null so the backend omits
    // them from the frontmatter.
    const agent: Agent = {
      name,
      description: editing.description?.trim() ? editing.description.trim() : null,
      model: editing.model?.trim() ? editing.model.trim() : null,
      tools: editing.tools?.trim() ? editing.tools.trim() : null,
      body: editing.body,
    };
    try {
      // Flat wire shape: the backend deserializes name/description/model/tools/
      // system_prompt at the top level (system_prompt is the Markdown body).
      await putAgent({
        scope,
        repo_id,
        name: agent.name,
        description: agent.description,
        model: agent.model,
        tools: agent.tools,
        system_prompt: agent.body,
      });
      // If the user renamed an existing agent, remove the old file.
      const renamed =
        editingOriginalName !== null && editingOriginalName !== agent.name;
      if (renamed) {
        try {
          await deleteAgent({
            scope,
            repo_id,
            name: editingOriginalName as string,
          });
        } catch {
          // Non-fatal: the new agent was written; surface a soft note below.
        }
      }
      // The write response carries metadata only, not the full Agent — build
      // the list row from the local agent we just sent.
      setAgents((list) => {
        const base = (list ?? []).filter(
          (a) =>
            a.name !== agent.name &&
            (!renamed || a.name !== editingOriginalName),
        );
        return [...base, agent].sort((a, b) => a.name.localeCompare(b.name));
      });
      setEditing(null);
      setEditingOriginalName(null);
      setBanner({
        kind: "ok",
        text: isRepo
          ? `Saved "${agent.name}" to .claude/agents on repohub-staging.`
          : `Saved "${agent.name}".`,
      });
    } catch (e) {
      setBanner({ kind: "err", text: e instanceof Error ? e.message : String(e) });
    } finally {
      setSaving(false);
    }
  };

  const remove = async (name: string) => {
    setBusy((b) => ({ ...b, [name]: "delete" }));
    setBanner(null);
    try {
      await deleteAgent({ scope, repo_id, name });
      setAgents((list) => (list ? list.filter((a) => a.name !== name) : list));
      if (editingOriginalName === name) {
        setEditing(null);
        setEditingOriginalName(null);
      }
      setBanner({ kind: "ok", text: `Deleted "${name}".` });
    } catch (e) {
      setBanner({ kind: "err", text: e instanceof Error ? e.message : String(e) });
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
        <h2 className="text-lg font-semibold">Subagents</h2>
        <button
          onClick={startNew}
          disabled={loading || !!editing}
          className="rounded-md bg-accent/20 px-3 py-1.5 text-sm text-accent transition hover:bg-accent/30 disabled:cursor-not-allowed disabled:opacity-40"
        >
          New agent
        </button>
      </div>
      <p className="mt-1 text-xs text-slate-500">
        Claude Code subagents — Markdown with YAML frontmatter (name,
        description, model, tools) plus a body that becomes the system prompt.
        {isRepo ? (
          <>
            {" "}
            Per-repo agents land in{" "}
            <code className="text-slate-400">.claude/agents</code> on the{" "}
            <span className="text-accent">repohub-staging</span> branch for
            review — not the live working tree.
          </>
        ) : (
          <>
            {" "}
            Global agents are written live to{" "}
            <code className="text-slate-400">~/.claude/agents</code>.
          </>
        )}
      </p>

      {banner && <BannerRow banner={banner} />}

      {/* ---- editor ---- */}
      {editing && (
        <div className="mt-4 rounded-lg border border-edge bg-slate-900/40 p-4">
          <div className="flex items-center justify-between">
            <h3 className="text-sm font-medium text-slate-200">
              {editingOriginalName === null
                ? "New agent"
                : `Editing "${editingOriginalName}"`}
            </h3>
            <div className="flex gap-2">
              <button
                onClick={cancelEdit}
                disabled={saving}
                className="rounded-md border border-edge px-3 py-1.5 text-sm text-slate-300 transition hover:bg-edge disabled:opacity-40"
              >
                Cancel
              </button>
              <button
                onClick={save}
                disabled={saving}
                className="rounded-md bg-accent/20 px-3 py-1.5 text-sm text-accent transition hover:bg-accent/30 disabled:cursor-not-allowed disabled:opacity-40"
              >
                {saving ? "Saving…" : "Save agent"}
              </button>
            </div>
          </div>

          <div className="mt-4 grid gap-3 sm:grid-cols-2">
            <Field label="Name" hint="lowercase, no spaces (e.g. code-reviewer)">
              <input
                value={editing.name}
                onChange={(e) => setField("name", e.target.value)}
                placeholder="code-reviewer"
                className={inputCls}
                disabled={editingExistingNames.size > 0 && saving}
              />
            </Field>
            <Field label="Model" hint="optional — inherits Claude Code's default">
              <input
                value={editing.model ?? ""}
                onChange={(e) => setField("model", e.target.value)}
                placeholder="(inherit)"
                className={inputCls}
              />
            </Field>
            <div className="sm:col-span-2">
              <Field label="Description" hint="when this subagent should be used">
                <input
                  value={editing.description ?? ""}
                  onChange={(e) => setField("description", e.target.value)}
                  placeholder="Reviews diffs for correctness and style."
                  className={inputCls}
                />
              </Field>
            </div>
            <div className="sm:col-span-2">
              <Field
                label="Tools"
                hint="comma/space-separated; leave blank to inherit all tools"
              >
                <input
                  value={editing.tools ?? ""}
                  onChange={(e) => setField("tools", e.target.value)}
                  placeholder="Read, Grep, Bash"
                  className={inputCls}
                />
              </Field>
            </div>
            <div className="sm:col-span-2">
              <Field label="System prompt" hint="the Markdown body">
                <textarea
                  value={editing.body}
                  onChange={(e) => setField("body", e.target.value)}
                  placeholder="You are a meticulous code reviewer…"
                  rows={10}
                  className={`${inputCls} resize-y font-mono leading-relaxed`}
                />
              </Field>
            </div>
          </div>
        </div>
      )}

      {/* ---- list ---- */}
      <div className="mt-5">
        {loading ? (
          <p className="text-sm text-slate-400">Loading agents…</p>
        ) : error ? (
          <ErrorRow text={error} onRetry={load} />
        ) : !agents || agents.length === 0 ? (
          <p className="rounded-lg border border-dashed border-edge px-4 py-6 text-center text-sm text-slate-500">
            No subagents defined for this scope yet.
          </p>
        ) : (
          <ul className="space-y-2">
            {agents.map((a) => {
              const b = busy[a.name];
              const isEditing = editingOriginalName === a.name;
              return (
                <li
                  key={a.name}
                  className={`rounded-lg border bg-slate-900/40 p-3 ${
                    isEditing ? "border-accent/60" : "border-edge"
                  }`}
                >
                  <div className="flex flex-wrap items-center gap-3">
                    <span className="font-mono text-sm font-medium text-slate-200">
                      {a.name}
                    </span>
                    {a.model ? (
                      <span className="rounded bg-edge px-1.5 py-0.5 text-[11px] uppercase tracking-wide text-slate-400">
                        {a.model}
                      </span>
                    ) : (
                      <span className="rounded bg-edge px-1.5 py-0.5 text-[11px] uppercase tracking-wide text-slate-500">
                        inherit model
                      </span>
                    )}
                    {a.tools ? (
                      <span className="text-xs text-slate-500">
                        tools: {a.tools}
                      </span>
                    ) : null}
                    <div className="ml-auto flex gap-2">
                      <button
                        onClick={() => startEdit(a)}
                        disabled={!!b || (!!editing && !isEditing)}
                        className="rounded-md border border-edge px-2.5 py-1 text-xs text-slate-300 transition hover:bg-edge disabled:opacity-40"
                      >
                        Edit
                      </button>
                      <button
                        onClick={() => remove(a.name)}
                        disabled={!!b}
                        className="rounded-md border border-rose-500/40 px-2.5 py-1 text-xs text-rose-400 transition hover:bg-rose-500/10 disabled:opacity-40"
                      >
                        {b === "delete" ? "Deleting…" : "Delete"}
                      </button>
                    </div>
                  </div>
                  {a.description ? (
                    <p className="mt-1.5 text-xs text-slate-400">
                      {a.description}
                    </p>
                  ) : null}
                </li>
              );
            })}
          </ul>
        )}
      </div>
    </section>
  );
}

function Field({
  label,
  hint,
  children,
}: {
  label: string;
  hint?: string;
  children: React.ReactNode;
}) {
  return (
    <label className="flex min-w-[8rem] flex-1 flex-col gap-1.5">
      <span className="text-xs font-medium text-slate-400">
        {label}
        {hint && <span className="ml-2 font-normal text-slate-600">{hint}</span>}
      </span>
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
    <div className="mt-4 flex items-center justify-between rounded-md bg-rose-500/10 px-3 py-2 text-sm text-rose-400">
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
