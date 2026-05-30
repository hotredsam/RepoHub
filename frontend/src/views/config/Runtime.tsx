import { useCallback, useEffect, useMemo, useState } from "react";

// Runtime.tsx — P16 unified Settings hub, "Runtime" knobs.
//
// Edits the runtime portion of Claude Code config in TWO layers:
//   - GLOBAL: applied LIVE to ~/.claude/settings.json. We GET the current
//     effective settings + a git history, and PUT a JSON *patch* that the
//     backend deep-merges (never blind-overwrites). A history list with Revert
//     buttons exposes the safety-net snapshots.
//   - REPO:  GET/PUT /api/config/repo/:id. Changes do NOT go live — they land
//     on the repohub-staging branch for review in the Merge tab. For a repo we
//     show inherit-vs-override: any field left unset INHERITS the global value.
//
// Every PUT is preceded by an explicit diff/confirm step so the user always
// sees exactly which keys change before anything is written.
//
// The API helpers + payload shapes are kept local to this unit (mirroring the
// convention in src/views/Merge.tsx and src/views/Connections.tsx) so this
// change touches only this file. They follow the same same-origin / JSON
// error-body contract as src/lib/api.ts.

// ---------------------------------------------------------------------------
// configApi — local same-origin fetch helpers (mirror src/lib/api.ts)
// ---------------------------------------------------------------------------

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

// One safety-net snapshot of ~/.claude (mirrors claude_fs::HistoryEntry).
interface HistoryEntry {
  hash: string;
  msg: string;
  date: string;
}

// GET/PUT /api/config/global — the live effective ~/.claude/settings.json.
// (History is a separate endpoint, /api/config/history.)
interface GlobalConfig {
  settings: Record<string, unknown>;
}

// GET/PUT /api/config/repo/:id — the per-repo override layer. `settings` holds
// only the keys this repo overrides (its .claude/settings.json on
// repohub-staging). The inherited global base is fetched separately so the UI
// can show inherit-vs-override.
interface RepoConfig {
  repo_id: number;
  branch: string;
  settings: Record<string, unknown>;
}

const configApi = {
  getGlobal: () => request<GlobalConfig>("/api/config/global"),
  // Git log of ~/.claude safety-net snapshots (newest first; [] when not a repo).
  getHistory: () => request<HistoryEntry[]>("/api/config/history"),
  // PUT a JSON patch; the backend deep-merges it into the live settings. The
  // backend's GlobalPatchBody requires the patch wrapped as { patch }.
  putGlobal: (patch: Record<string, unknown>) =>
    request<GlobalConfig>("/api/config/global", {
      method: "PUT",
      body: JSON.stringify({ patch }),
    }),
  revertGlobal: (hash: string) =>
    request<GlobalConfig>("/api/config/revert", {
      method: "POST",
      body: JSON.stringify({ hash }),
    }),
  getRepo: (id: number) => request<RepoConfig>(`/api/config/repo/${id}`),
  // PUT a JSON patch for the repo override layer (lands on repohub-staging).
  // RepoPatchBody also requires the wrapped { patch } shape.
  putRepo: (id: number, patch: Record<string, unknown>) =>
    request<RepoConfig>(`/api/config/repo/${id}`, {
      method: "PUT",
      body: JSON.stringify({ patch }),
    }),
};

// ---------------------------------------------------------------------------
// Runtime knob model
//
// We edit a curated subset of Claude Code's settings.json keys as a flat form,
// then assemble a deep JSON patch on save. Reading is the inverse: we pluck the
// same keys out of the effective settings object. Anything outside this subset
// is left untouched by our patches (deep-merge preserves it).
// ---------------------------------------------------------------------------

interface RuntimeForm {
  model: string; // blank = inherit (never sets a default model)
  systemPrompt: string;
  appendSystemPrompt: string;
  skills: string; // enabled skill names
  toolsAllow: string;
  toolsDeny: string;
  permAllow: string;
  permDeny: string;
  permAsk: string;
  loops: string; // JSON for the "loops" block
  workflows: string; // JSON for the "workflows" block
}

const EMPTY_FORM: RuntimeForm = {
  model: "",
  systemPrompt: "",
  appendSystemPrompt: "",
  skills: "",
  toolsAllow: "",
  toolsDeny: "",
  permAllow: "",
  permDeny: "",
  permAsk: "",
  loops: "",
  workflows: "",
};

// Parse a textarea of newline/comma-separated entries into a trimmed list.
function parseList(text: string): string[] {
  return text
    .split(/[\n,]/)
    .map((s) => s.trim())
    .filter((s) => s.length > 0);
}

function joinList(arr: unknown): string {
  if (!Array.isArray(arr)) return "";
  return arr.filter((x) => typeof x === "string").join("\n");
}

// Safe getters for nested settings shapes.
function asString(v: unknown): string {
  return typeof v === "string" ? v : "";
}
function getPath(obj: Record<string, unknown>, ...keys: string[]): unknown {
  let cur: unknown = obj;
  for (const k of keys) {
    if (cur && typeof cur === "object" && !Array.isArray(cur)) {
      cur = (cur as Record<string, unknown>)[k];
    } else {
      return undefined;
    }
  }
  return cur;
}
function prettyJson(v: unknown): string {
  if (v === undefined || v === null) return "";
  try {
    return JSON.stringify(v, null, 2);
  } catch {
    return "";
  }
}

// Read a settings object into the flat form. Keys map to Claude Code's
// settings.json shape: permissions.{allow,deny,ask}, tools.{allow,deny}, etc.
function formFromSettings(s: Record<string, unknown>): RuntimeForm {
  return {
    model: asString(s.model),
    systemPrompt: asString(s.systemPrompt),
    appendSystemPrompt: asString(s.appendSystemPrompt),
    skills: joinList(s.skills ?? s.enabledSkills),
    toolsAllow: joinList(getPath(s, "tools", "allow")),
    toolsDeny: joinList(getPath(s, "tools", "deny")),
    permAllow: joinList(getPath(s, "permissions", "allow")),
    permDeny: joinList(getPath(s, "permissions", "deny")),
    permAsk: joinList(getPath(s, "permissions", "ask")),
    loops: prettyJson(s.loops),
    workflows: prettyJson(s.workflows),
  };
}

// A parsed JSON block, or an error string (for inline validation).
type JsonResult =
  | { ok: true; value: unknown }
  | { ok: false; error: string };

function parseJsonField(text: string): JsonResult {
  const t = text.trim();
  if (!t) return { ok: true, value: undefined };
  try {
    return { ok: true, value: JSON.parse(t) };
  } catch (e) {
    return { ok: false, error: e instanceof Error ? e.message : String(e) };
  }
}

// Assemble a deep JSON patch from the form. Only includes keys the user
// actually set, so blanks INHERIT (global: Claude Code's default; repo: the
// global layer). Returns the patch and any JSON parse errors for loops/workflows.
function buildPatch(form: RuntimeForm): {
  patch: Record<string, unknown>;
  errors: Partial<Record<"loops" | "workflows", string>>;
} {
  const patch: Record<string, unknown> = {};
  const errors: Partial<Record<"loops" | "workflows", string>> = {};

  if (form.model.trim()) patch.model = form.model.trim();
  if (form.systemPrompt.trim()) patch.systemPrompt = form.systemPrompt;
  if (form.appendSystemPrompt.trim())
    patch.appendSystemPrompt = form.appendSystemPrompt;

  const skills = parseList(form.skills);
  if (skills.length > 0) patch.skills = skills;

  const toolsAllow = parseList(form.toolsAllow);
  const toolsDeny = parseList(form.toolsDeny);
  if (toolsAllow.length > 0 || toolsDeny.length > 0) {
    const tools: Record<string, unknown> = {};
    if (toolsAllow.length > 0) tools.allow = toolsAllow;
    if (toolsDeny.length > 0) tools.deny = toolsDeny;
    patch.tools = tools;
  }

  const permAllow = parseList(form.permAllow);
  const permDeny = parseList(form.permDeny);
  const permAsk = parseList(form.permAsk);
  if (permAllow.length > 0 || permDeny.length > 0 || permAsk.length > 0) {
    const permissions: Record<string, unknown> = {};
    if (permAllow.length > 0) permissions.allow = permAllow;
    if (permDeny.length > 0) permissions.deny = permDeny;
    if (permAsk.length > 0) permissions.ask = permAsk;
    patch.permissions = permissions;
  }

  const loops = parseJsonField(form.loops);
  if (!loops.ok) errors.loops = loops.error;
  else if (loops.value !== undefined) patch.loops = loops.value;

  const workflows = parseJsonField(form.workflows);
  if (!workflows.ok) errors.workflows = workflows.error;
  else if (workflows.value !== undefined) patch.workflows = workflows.value;

  return { patch, errors };
}

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

type Banner = { kind: "ok" | "err"; text: string } | null;

export interface RuntimeProps {
  scope: "global" | "repo";
  // null when no repo is selected (scope === "global", or "repo" before a pick).
  repoId?: number | null;
}

export default function Runtime({ scope, repoId }: RuntimeProps) {
  const isRepo = scope === "repo";

  const [form, setForm] = useState<RuntimeForm>(EMPTY_FORM);
  // The effective settings as last loaded — the baseline we diff against.
  const [baseline, setBaseline] = useState<Record<string, unknown>>({});
  // The inherited global layer (repo scope only), for inherit-vs-override hints.
  const [inherited, setInherited] = useState<Record<string, unknown>>({});
  const [history, setHistory] = useState<HistoryEntry[]>([]);
  const [branch, setBranch] = useState<string>("repohub-staging");

  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [banner, setBanner] = useState<Banner>(null);

  // Diff/confirm modal state. `null` = closed.
  const [pendingPatch, setPendingPatch] = useState<Record<
    string,
    unknown
  > | null>(null);
  const [saving, setSaving] = useState(false);

  // Revert-in-flight hash (global scope), for per-row button disabling.
  const [revertingHash, setRevertingHash] = useState<string | null>(null);

  const applyLoaded = useCallback(
    (settings: Record<string, unknown>, global: Record<string, unknown>) => {
      setBaseline(settings);
      setInherited(global);
      setForm(formFromSettings(settings));
    },
    [],
  );

  const load = useCallback(() => {
    setLoading(true);
    setLoadError(null);
    setBanner(null);
    if (isRepo) {
      if (repoId === undefined || repoId === null) {
        setLoadError("Select a repository to edit its overrides.");
        setLoading(false);
        return;
      }
      // The repo override layer plus the inherited global settings (fetched
      // separately) so we can render inherit-vs-override.
      Promise.all([configApi.getRepo(repoId), configApi.getGlobal()])
        .then(([cfg, global]) => {
          setBranch(cfg.branch || "repohub-staging");
          setHistory([]);
          applyLoaded(cfg.settings ?? {}, global.settings ?? {});
        })
        .catch((e) => setLoadError(e instanceof Error ? e.message : String(e)))
        .finally(() => setLoading(false));
    } else {
      // History comes from the dedicated /api/config/history endpoint.
      Promise.all([configApi.getGlobal(), configApi.getHistory()])
        .then(([cfg, hist]) => {
          setHistory(hist ?? []);
          applyLoaded(cfg.settings ?? {}, {});
        })
        .catch((e) => setLoadError(e instanceof Error ? e.message : String(e)))
        .finally(() => setLoading(false));
    }
  }, [isRepo, repoId, applyLoaded]);

  useEffect(() => {
    load();
  }, [load]);

  const setField = (key: keyof RuntimeForm, value: string) =>
    setForm((f) => ({ ...f, [key]: value }));

  // Build the patch live so we can validate JSON fields and gate the diff step.
  const { patch, errors } = useMemo(() => buildPatch(form), [form]);
  const jsonError = errors.loops ?? errors.workflows ?? null;

  // For repo scope, which top-level form keys are currently overriding (i.e.
  // present in the repo's own settings, distinct from the inherited global).
  const overriddenKeys = useMemo(() => {
    const set = new Set<string>();
    if (!isRepo) return set;
    const check = (k: string) =>
      Object.prototype.hasOwnProperty.call(baseline, k);
    for (const k of [
      "model",
      "systemPrompt",
      "appendSystemPrompt",
      "skills",
      "tools",
      "permissions",
      "loops",
      "workflows",
    ]) {
      if (check(k)) set.add(k);
    }
    return set;
  }, [isRepo, baseline]);

  const openConfirm = () => {
    setBanner(null);
    if (jsonError) {
      setBanner({ kind: "err", text: `Fix JSON before saving: ${jsonError}` });
      return;
    }
    if (Object.keys(patch).length === 0) {
      setBanner({
        kind: "err",
        text: "Nothing to save — set at least one field.",
      });
      return;
    }
    setPendingPatch(patch);
  };

  const confirmSave = async () => {
    if (!pendingPatch) return;
    setSaving(true);
    setBanner(null);
    try {
      if (isRepo) {
        const cfg = await configApi.putRepo(repoId as number, pendingPatch);
        setBranch(cfg.branch || "repohub-staging");
        // Re-fetch the inherited global layer so override hints stay accurate.
        const global = await configApi.getGlobal();
        applyLoaded(cfg.settings ?? {}, global.settings ?? {});
        setBanner({
          kind: "ok",
          text: `Saved to branch "${cfg.branch || branch}". Review and merge it in the Merge tab.`,
        });
      } else {
        const cfg = await configApi.putGlobal(pendingPatch);
        // The write created a new safety-net snapshot — refresh the history.
        const hist = await configApi.getHistory();
        setHistory(hist ?? []);
        applyLoaded(cfg.settings ?? {}, {});
        setBanner({
          kind: "ok",
          text: "Saved live to ~/.claude/settings.json. A snapshot was committed first.",
        });
      }
      setPendingPatch(null);
    } catch (e) {
      setBanner({ kind: "err", text: e instanceof Error ? e.message : String(e) });
    } finally {
      setSaving(false);
    }
  };

  const revert = async (hash: string) => {
    setRevertingHash(hash);
    setBanner(null);
    try {
      const cfg = await configApi.revertGlobal(hash);
      const hist = await configApi.getHistory();
      setHistory(hist ?? []);
      applyLoaded(cfg.settings ?? {}, {});
      setBanner({
        kind: "ok",
        text: `Reverted settings.json to ${hash.slice(0, 8)}. A snapshot was committed first.`,
      });
    } catch (e) {
      setBanner({ kind: "err", text: e instanceof Error ? e.message : String(e) });
    } finally {
      setRevertingHash(null);
    }
  };

  if (loading) {
    return (
      <p className="text-sm text-slate-400">Loading runtime configuration…</p>
    );
  }

  if (loadError) {
    return (
      <div className="flex items-center justify-between rounded-md bg-rose-500/10 px-3 py-2 text-sm text-rose-400">
        <span>{loadError}</span>
        <button
          onClick={load}
          className="rounded border border-rose-500/40 px-2 py-0.5 text-xs hover:bg-rose-500/10"
        >
          Retry
        </button>
      </div>
    );
  }

  return (
    <div className="space-y-6">
      {/* ---- scope note ---- */}
      {isRepo ? (
        <div className="rounded-lg border border-edge bg-slate-900/40 px-4 py-3 text-sm text-slate-300">
          Repo overrides land on{" "}
          <code className="rounded bg-edge px-1 py-0.5 text-xs text-slate-200">
            {branch}
          </code>{" "}
          — they are <span className="font-medium">not</span> applied live.
          Review and merge them in the{" "}
          <span className="text-accent">Merge</span> tab. Any field you leave
          blank <span className="font-medium">inherits</span> the global value.
        </div>
      ) : (
        <div className="rounded-lg border border-amber-500/40 bg-amber-500/10 px-4 py-3 text-sm text-amber-200">
          Global settings apply{" "}
          <span className="font-semibold">live</span> to{" "}
          <code className="rounded bg-amber-500/15 px-1 py-0.5 text-xs">
            ~/.claude/settings.json
          </code>
          . A git snapshot is committed before every change — use the history
          below to revert. Blank fields inherit Claude Code's own defaults.
        </div>
      )}

      {/* ---- form ---- */}
      <div className="space-y-5">
        <Knob
          label="Model"
          hint="Leave blank to inherit Claude Code's default (no default model is forced)."
          inherit={inheritState(isRepo, overriddenKeys, "model", inherited, "model")}
        >
          <input
            value={form.model}
            onChange={(e) => setField("model", e.target.value)}
            placeholder="inherit (blank)"
            className={inputCls}
          />
        </Knob>

        <Knob
          label="System prompt (replace)"
          hint="Replaces the system prompt entirely. Usually leave blank and use append."
          inherit={inheritState(
            isRepo,
            overriddenKeys,
            "systemPrompt",
            inherited,
            "systemPrompt",
          )}
        >
          <textarea
            value={form.systemPrompt}
            onChange={(e) => setField("systemPrompt", e.target.value)}
            rows={3}
            placeholder="inherit (blank)"
            className={areaCls}
          />
        </Knob>

        <Knob
          label="Append to system prompt"
          hint="Appended to the existing system prompt."
          inherit={inheritState(
            isRepo,
            overriddenKeys,
            "appendSystemPrompt",
            inherited,
            "appendSystemPrompt",
          )}
        >
          <textarea
            value={form.appendSystemPrompt}
            onChange={(e) => setField("appendSystemPrompt", e.target.value)}
            rows={3}
            placeholder="inherit (blank)"
            className={areaCls}
          />
        </Knob>

        <Knob
          label="Enabled skills"
          hint="One per line (or comma-separated). The skills to enable."
          inherit={inheritState(isRepo, overriddenKeys, "skills", inherited, "skills")}
        >
          <textarea
            value={form.skills}
            onChange={(e) => setField("skills", e.target.value)}
            rows={3}
            placeholder={"deep-research\ncode-review"}
            className={areaCls}
          />
        </Knob>

        <div className="grid gap-5 sm:grid-cols-2">
          <Knob
            label="Tools — allow"
            hint="One per line. Tools explicitly allowed."
            inherit={inheritState(
              isRepo,
              overriddenKeys,
              "tools",
              inherited,
              "tools",
              "allow",
            )}
          >
            <textarea
              value={form.toolsAllow}
              onChange={(e) => setField("toolsAllow", e.target.value)}
              rows={3}
              placeholder={"Bash\nEdit"}
              className={areaCls}
            />
          </Knob>
          <Knob
            label="Tools — deny"
            hint="One per line. Tools explicitly denied."
            inherit={inheritState(
              isRepo,
              overriddenKeys,
              "tools",
              inherited,
              "tools",
              "deny",
            )}
          >
            <textarea
              value={form.toolsDeny}
              onChange={(e) => setField("toolsDeny", e.target.value)}
              rows={3}
              placeholder={"WebFetch"}
              className={areaCls}
            />
          </Knob>
        </div>

        <div className="grid gap-5 sm:grid-cols-3">
          <Knob
            label="Permissions — allow"
            hint="Auto-allowed without a prompt."
            inherit={inheritState(
              isRepo,
              overriddenKeys,
              "permissions",
              inherited,
              "permissions",
              "allow",
            )}
          >
            <textarea
              value={form.permAllow}
              onChange={(e) => setField("permAllow", e.target.value)}
              rows={4}
              placeholder={"Bash(git status)\nRead(./**)"}
              className={areaCls}
            />
          </Knob>
          <Knob
            label="Permissions — deny"
            hint="Always denied."
            inherit={inheritState(
              isRepo,
              overriddenKeys,
              "permissions",
              inherited,
              "permissions",
              "deny",
            )}
          >
            <textarea
              value={form.permDeny}
              onChange={(e) => setField("permDeny", e.target.value)}
              rows={4}
              placeholder={"Bash(rm -rf*)"}
              className={areaCls}
            />
          </Knob>
          <Knob
            label="Permissions — ask"
            hint="Always prompt before use."
            inherit={inheritState(
              isRepo,
              overriddenKeys,
              "permissions",
              inherited,
              "permissions",
              "ask",
            )}
          >
            <textarea
              value={form.permAsk}
              onChange={(e) => setField("permAsk", e.target.value)}
              rows={4}
              placeholder={"Bash(git push*)"}
              className={areaCls}
            />
          </Knob>
        </div>

        <Knob
          label="Loops (JSON)"
          hint="Raw JSON for the loops block. Leave blank to inherit."
          inherit={inheritState(isRepo, overriddenKeys, "loops", inherited, "loops")}
          error={errors.loops}
        >
          <textarea
            value={form.loops}
            onChange={(e) => setField("loops", e.target.value)}
            rows={4}
            spellCheck={false}
            placeholder={"{ }"}
            className={`${areaCls} font-mono`}
          />
        </Knob>

        <Knob
          label="Workflows (JSON)"
          hint="Raw JSON for the workflows block. Leave blank to inherit."
          inherit={inheritState(
            isRepo,
            overriddenKeys,
            "workflows",
            inherited,
            "workflows",
          )}
          error={errors.workflows}
        >
          <textarea
            value={form.workflows}
            onChange={(e) => setField("workflows", e.target.value)}
            rows={4}
            spellCheck={false}
            placeholder={"{ }"}
            className={`${areaCls} font-mono`}
          />
        </Knob>
      </div>

      {banner && <BannerRow banner={banner} />}

      <div className="flex items-center gap-3">
        <button
          onClick={openConfirm}
          disabled={saving || !!jsonError}
          className="rounded-md bg-accent/20 px-4 py-2 text-sm text-accent transition hover:bg-accent/30 disabled:cursor-not-allowed disabled:opacity-40"
        >
          {isRepo ? "Review & save to staging" : "Review & apply live"}
        </button>
        <button
          onClick={load}
          disabled={saving}
          className="rounded-md border border-edge px-4 py-2 text-sm text-slate-300 transition hover:bg-edge disabled:opacity-40"
        >
          Reset
        </button>
      </div>

      {/* ---- history + revert (global only) ---- */}
      {!isRepo && (
        <section className="rounded-xl border border-edge bg-panel p-5">
          <h3 className="text-sm font-semibold text-slate-200">
            Change history
          </h3>
          <p className="mt-1 text-xs text-slate-500">
            Safety-net snapshots of <code>~/.claude</code>. Revert restores{" "}
            <code>settings.json</code> from a snapshot (a new snapshot is
            committed first).
          </p>
          {history.length === 0 ? (
            <p className="mt-3 rounded-lg border border-dashed border-edge px-4 py-5 text-center text-xs text-slate-500">
              No snapshots yet. The first one is created when you save a change.
            </p>
          ) : (
            <ul className="mt-3 space-y-1.5">
              {history.map((h) => (
                <li
                  key={h.hash}
                  className="flex flex-wrap items-center gap-3 rounded-lg border border-edge bg-slate-900/40 px-3 py-2"
                >
                  <code className="font-mono text-xs text-accent">
                    {h.hash.slice(0, 8)}
                  </code>
                  <span className="text-sm text-slate-300">{h.msg}</span>
                  <span className="text-xs text-slate-500">
                    {formatDate(h.date)}
                  </span>
                  <button
                    onClick={() => revert(h.hash)}
                    disabled={revertingHash !== null}
                    className="ml-auto rounded-md border border-edge px-2.5 py-1 text-xs text-slate-300 transition hover:bg-edge disabled:opacity-40"
                  >
                    {revertingHash === h.hash ? "Reverting…" : "Revert"}
                  </button>
                </li>
              ))}
            </ul>
          )}
        </section>
      )}

      {/* ---- diff / confirm modal ---- */}
      {pendingPatch && (
        <ConfirmModal
          isRepo={isRepo}
          branch={branch}
          patch={pendingPatch}
          baseline={baseline}
          saving={saving}
          onCancel={() => setPendingPatch(null)}
          onConfirm={confirmSave}
        />
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// inherit-vs-override indicator
// ---------------------------------------------------------------------------

type InheritState =
  | { kind: "n/a" } // global scope — no inheritance to show
  | { kind: "override" } // repo overrides this key
  | { kind: "inherit"; value: string }; // repo inherits the global value

// Decide whether a repo field is overriding or inheriting, and what the
// inherited value looks like (for the hint). For global scope this is "n/a".
function inheritState(
  isRepo: boolean,
  overriddenKeys: Set<string>,
  topKey: string,
  inherited: Record<string, unknown>,
  ...path: string[]
): InheritState {
  if (!isRepo) return { kind: "n/a" };
  if (overriddenKeys.has(topKey)) return { kind: "override" };
  const value = getPath(inherited, ...path);
  let display = "";
  if (Array.isArray(value)) display = joinList(value).replace(/\n/g, ", ");
  else if (typeof value === "string") display = value;
  else if (value !== undefined && value !== null) display = prettyJson(value);
  return { kind: "inherit", value: display };
}

function InheritBadge({ state }: { state: InheritState }) {
  if (state.kind === "n/a") return null;
  if (state.kind === "override") {
    return (
      <span className="rounded bg-accent/15 px-1.5 py-0.5 text-[10px] font-medium uppercase tracking-wide text-accent">
        override
      </span>
    );
  }
  return (
    <span
      className="rounded bg-edge px-1.5 py-0.5 text-[10px] font-medium uppercase tracking-wide text-slate-400"
      title={state.value ? `Inherited: ${state.value}` : "Inherited from global"}
    >
      inherit{state.value ? " ✓" : ""}
    </span>
  );
}

// ---------------------------------------------------------------------------
// presentational helpers
// ---------------------------------------------------------------------------

const inputCls =
  "w-full rounded-md border border-edge bg-slate-900/60 px-3 py-2 text-sm text-slate-200 outline-none focus:border-accent placeholder:text-slate-600";
const areaCls = `${inputCls} resize-y`;

function Knob({
  label,
  hint,
  inherit,
  error,
  children,
}: {
  label: string;
  hint?: string;
  inherit?: InheritState;
  error?: string;
  children: React.ReactNode;
}) {
  return (
    <label className="flex flex-col gap-1.5">
      <span className="flex items-center gap-2">
        <span className="text-xs font-medium text-slate-400">{label}</span>
        {inherit && <InheritBadge state={inherit} />}
      </span>
      {children}
      {hint && <span className="text-[11px] text-slate-500">{hint}</span>}
      {error && (
        <span className="text-[11px] text-rose-400">Invalid JSON: {error}</span>
      )}
    </label>
  );
}

function BannerRow({ banner }: { banner: NonNullable<Banner> }) {
  return (
    <p
      className={`rounded-md px-3 py-2 text-sm ${
        banner.kind === "ok"
          ? "bg-emerald-500/10 text-emerald-400"
          : "bg-rose-500/10 text-rose-400"
      }`}
    >
      {banner.text}
    </p>
  );
}

function formatDate(iso: string): string {
  if (!iso) return "";
  const d = new Date(iso);
  return Number.isNaN(d.getTime()) ? iso : d.toLocaleString();
}

// ---------------------------------------------------------------------------
// diff / confirm modal
//
// Shows, per top-level key in the patch, the BEFORE (current effective value)
// and AFTER (deep-merge result) so the user sees exactly what will change
// before anything is written.
// ---------------------------------------------------------------------------

// Local deep-merge mirroring claude_fs::deep_merge so the preview matches what
// the backend will write (objects merged key-by-key; non-objects replace).
function deepMerge(target: unknown, patch: unknown): unknown {
  if (
    target &&
    typeof target === "object" &&
    !Array.isArray(target) &&
    patch &&
    typeof patch === "object" &&
    !Array.isArray(patch)
  ) {
    const out: Record<string, unknown> = {
      ...(target as Record<string, unknown>),
    };
    for (const [k, v] of Object.entries(patch as Record<string, unknown>)) {
      out[k] = deepMerge(out[k], v);
    }
    return out;
  }
  return patch;
}

interface DiffRow {
  key: string;
  before: string;
  after: string;
  changed: boolean;
}

function buildDiff(
  baseline: Record<string, unknown>,
  patch: Record<string, unknown>,
): DiffRow[] {
  return Object.keys(patch).map((key) => {
    const before = baseline[key];
    const after = deepMerge(before, patch[key]);
    const beforeStr = before === undefined ? "(inherit / unset)" : prettyJson(before);
    const afterStr = prettyJson(after);
    return {
      key,
      before: beforeStr,
      after: afterStr,
      changed: beforeStr !== afterStr,
    };
  });
}

function ConfirmModal({
  isRepo,
  branch,
  patch,
  baseline,
  saving,
  onCancel,
  onConfirm,
}: {
  isRepo: boolean;
  branch: string;
  patch: Record<string, unknown>;
  baseline: Record<string, unknown>;
  saving: boolean;
  onCancel: () => void;
  onConfirm: () => void;
}) {
  const rows = useMemo(() => buildDiff(baseline, patch), [baseline, patch]);
  const changedCount = rows.filter((r) => r.changed).length;

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-4"
      onClick={saving ? undefined : onCancel}
    >
      <div
        className="flex max-h-[85vh] w-full max-w-2xl flex-col rounded-xl border border-edge bg-panel shadow-xl"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="border-b border-edge px-6 py-4">
          <h2 className="text-base font-semibold text-slate-100">
            Confirm changes
          </h2>
          <p className="mt-1 text-xs text-slate-500">
            {isRepo ? (
              <>
                These keys will be written to{" "}
                <code className="text-slate-300">.claude/settings.json</code> on
                branch{" "}
                <code className="rounded bg-edge px-1 py-0.5 text-slate-200">
                  {branch}
                </code>{" "}
                (not applied live) — merged into the existing file.
              </>
            ) : (
              <>
                These keys will be deep-merged LIVE into{" "}
                <code className="text-slate-300">~/.claude/settings.json</code>.
                A snapshot is committed first.
              </>
            )}{" "}
            {changedCount} of {rows.length} change
            {rows.length === 1 ? "" : "s"} differ from the current value.
          </p>
        </div>

        <div className="flex-1 overflow-auto px-6 py-4">
          {rows.length === 0 ? (
            <p className="text-sm text-slate-400">No changes.</p>
          ) : (
            <ul className="space-y-3">
              {rows.map((r) => (
                <li
                  key={r.key}
                  className="overflow-hidden rounded-lg border border-edge"
                >
                  <div className="flex items-center gap-2 bg-slate-900/60 px-3 py-1.5">
                    <code className="font-mono text-xs text-slate-200">
                      {r.key}
                    </code>
                    {!r.changed && (
                      <span className="text-[10px] uppercase tracking-wide text-slate-500">
                        unchanged
                      </span>
                    )}
                  </div>
                  <div className="grid grid-cols-1 gap-px bg-edge sm:grid-cols-2">
                    <DiffPane title="Before" body={r.before} tone="before" />
                    <DiffPane title="After" body={r.after} tone="after" />
                  </div>
                </li>
              ))}
            </ul>
          )}
        </div>

        <div className="flex justify-end gap-2 border-t border-edge px-6 py-4">
          <button
            onClick={onCancel}
            disabled={saving}
            className="rounded-md border border-edge px-4 py-2 text-sm text-slate-300 transition hover:bg-edge disabled:opacity-50"
          >
            Cancel
          </button>
          <button
            onClick={onConfirm}
            disabled={saving}
            className="rounded-md bg-accent/20 px-4 py-2 text-sm text-accent transition hover:bg-accent/30 disabled:cursor-not-allowed disabled:opacity-40"
          >
            {saving
              ? "Saving…"
              : isRepo
                ? "Commit to staging"
                : "Apply live"}
          </button>
        </div>
      </div>
    </div>
  );
}

function DiffPane({
  title,
  body,
  tone,
}: {
  title: string;
  body: string;
  tone: "before" | "after";
}) {
  return (
    <div className="bg-slate-950/60">
      <div className="px-3 py-1 text-[10px] font-medium uppercase tracking-wide text-slate-500">
        {title}
      </div>
      <pre
        className={`max-h-48 overflow-auto px-3 pb-2 text-xs leading-relaxed ${
          tone === "after" ? "text-emerald-300" : "text-slate-400"
        }`}
      >
        <code>{body || "—"}</code>
      </pre>
    </div>
  );
}
