import { useCallback, useEffect, useMemo, useState } from "react";
import {
  getPreferences,
  putPreferences,
  listInfra,
  createInfra,
  deleteInfra,
  testInfra,
  getSettings,
  putSetting,
  listRepos,
  suggestSettings,
  applySuggestion,
} from "../lib/api";
import type {
  Preferences,
  InfraResource,
  CreateInfraBody,
  InfraTestResult,
  Setting,
  Repo,
  SuggestResponse,
  SuggestedFile,
} from "../lib/types";

// New P16 config sections (separate units; default exports).
import Runtime from "./config/Runtime";
import Agents from "./config/Agents";
import Mcp from "./config/Mcp";
import Knowledge from "./config/Knowledge";
import AiOps from "./config/AiOps";
import GoogleCloud from "./config/GoogleCloud";

// ---------------------------------------------------------------------------
// Settings shell (P16)
//
// A unified, layered Settings hub. A left sub-nav switches between sections; a
// scope toggle at the top selects the layer the section operates on:
//   - "global": the user's real ~/.claude config (applied live)
//   - "repo":   a per-repo override that inherits global when unset
//
// The new config sections (Runtime/Agents/MCP/Knowledge/AI Ops/Google Cloud)
// are separate components that read the current scope via {scope, repoId} props.
// The original sections (Preferences, Infrastructure, Suggest) are preserved
// verbatim as local sub-tab components below.
// ---------------------------------------------------------------------------

// Layer the active section operates on. Shared with the section components so
// sibling units import a single source of truth for the props contract.
export type Scope = "global" | "repo";

// Props every config section receives. When scope === "repo", repoId is the
// selected repository (or null if none chosen yet); for "global" it is null.
export interface SectionProps {
  scope: Scope;
  repoId: number | null;
}

type SectionKey =
  | "preferences"
  | "infrastructure"
  | "suggest"
  | "runtime"
  | "agents"
  | "mcp"
  | "knowledge"
  | "aiops"
  | "googlecloud";

type SectionDef = {
  key: SectionKey;
  label: string;
  // Sections that operate on a config layer honor the scope toggle; the legacy
  // sections are scope-agnostic and hide the toggle when active.
  scoped: boolean;
};

const SECTIONS: SectionDef[] = [
  { key: "runtime", label: "Runtime", scoped: true },
  { key: "agents", label: "Agents", scoped: true },
  { key: "mcp", label: "MCP", scoped: true },
  { key: "knowledge", label: "Knowledge", scoped: true },
  { key: "aiops", label: "AI Ops", scoped: true },
  { key: "googlecloud", label: "Google Cloud", scoped: false },
  { key: "preferences", label: "Preferences", scoped: false },
  { key: "infrastructure", label: "Infrastructure", scoped: false },
  { key: "suggest", label: "Suggest", scoped: false },
];

export default function Settings() {
  const [active, setActive] = useState<SectionKey>("runtime");

  // ---- scope toggle (shared across scoped sections) ----
  const [scope, setScope] = useState<Scope>("global");
  const [scopeRepoId, setScopeRepoId] = useState<number | "">("");

  // Repo list backs both the scope picker and the Suggest section.
  const [repos, setRepos] = useState<Repo[] | null>(null);
  const [reposLoading, setReposLoading] = useState(true);
  const [reposError, setReposError] = useState<string | null>(null);

  const loadRepos = useCallback(() => {
    setReposLoading(true);
    setReposError(null);
    listRepos()
      .then(setRepos)
      .catch((e) => setReposError(String(e instanceof Error ? e.message : e)))
      .finally(() => setReposLoading(false));
  }, []);

  useEffect(() => {
    loadRepos();
  }, [loadRepos]);

  const activeDef = SECTIONS.find((s) => s.key === active)!;
  const repoId =
    scope === "repo" && scopeRepoId !== "" ? Number(scopeRepoId) : null;
  const sectionProps: SectionProps = { scope, repoId };

  return (
    <div className="mx-auto max-w-5xl space-y-6">
      <div>
        <h1 className="text-2xl font-semibold tracking-tight">Settings</h1>
        <p className="mt-1 text-sm text-slate-400">
          A unified hub for Claude Code configuration. Global defaults apply
          live to your <code className="text-slate-400">~/.claude</code> config;
          per-repo overrides inherit global when unset and land on the{" "}
          <span className="text-accent">repohub-staging</span> branch for review.
        </p>
      </div>

      {/* ---- scope toggle (only for scoped sections) ---- */}
      {activeDef.scoped && (
        <div className="flex flex-wrap items-center gap-3 rounded-xl border border-edge bg-panel px-4 py-3">
          <span className="text-xs font-medium uppercase tracking-wide text-slate-500">
            Layer
          </span>
          <div className="inline-flex overflow-hidden rounded-md border border-edge">
            <button
              onClick={() => setScope("global")}
              className={`px-3 py-1.5 text-sm transition ${
                scope === "global"
                  ? "bg-accent/20 text-accent"
                  : "text-slate-400 hover:bg-edge hover:text-slate-200"
              }`}
            >
              Global default
            </button>
            <button
              onClick={() => setScope("repo")}
              className={`border-l border-edge px-3 py-1.5 text-sm transition ${
                scope === "repo"
                  ? "bg-accent/20 text-accent"
                  : "text-slate-400 hover:bg-edge hover:text-slate-200"
              }`}
            >
              Per-repo override
            </button>
          </div>

          {scope === "repo" &&
            (reposLoading ? (
              <span className="text-sm text-slate-400">Loading repos…</span>
            ) : reposError ? (
              <span className="text-sm text-rose-400">{reposError}</span>
            ) : (
              <select
                value={scopeRepoId}
                onChange={(e) =>
                  setScopeRepoId(
                    e.target.value === "" ? "" : Number(e.target.value),
                  )
                }
                className="rounded-md border border-edge bg-slate-900/60 px-3 py-1.5 text-sm text-slate-200 outline-none focus:border-accent"
              >
                <option value="">Select a repo…</option>
                {(repos ?? []).map((r) => (
                  <option key={r.id} value={r.id}>
                    {r.full_name}
                    {r.local_path ? "" : " (not cloned)"}
                  </option>
                ))}
              </select>
            ))}

          <span className="ml-auto text-xs text-slate-500">
            {scope === "global"
              ? "Applies live to ~/.claude"
              : repoId !== null
                ? "Overrides land on repohub-staging"
                : "Pick a repo to edit overrides"}
          </span>
        </div>
      )}

      {/* ---- shell: left sub-nav + active section ---- */}
      <div className="flex flex-col gap-6 md:flex-row">
        <nav className="flex shrink-0 flex-row flex-wrap gap-1 md:w-48 md:flex-col">
          {SECTIONS.map((s) => (
            <button
              key={s.key}
              onClick={() => setActive(s.key)}
              className={`rounded-md px-3 py-2 text-left text-sm transition ${
                active === s.key
                  ? "bg-accent/20 text-accent"
                  : "text-slate-400 hover:bg-edge hover:text-slate-200"
              }`}
            >
              {s.label}
            </button>
          ))}
        </nav>

        <div className="min-w-0 flex-1 space-y-6">
          {active === "runtime" && <Runtime {...sectionProps} />}
          {active === "agents" && <Agents {...sectionProps} />}
          {active === "mcp" && <Mcp {...sectionProps} />}
          {active === "knowledge" && <Knowledge {...sectionProps} />}
          {active === "aiops" && <AiOps {...sectionProps} />}
          {active === "googlecloud" && <GoogleCloud />}

          {active === "preferences" && <PreferencesSection />}
          {active === "infrastructure" && <InfrastructureSection />}
          {active === "suggest" && (
            <SuggestSection
              repos={repos}
              reposLoading={reposLoading}
              reposError={reposError}
            />
          )}
        </div>
      </div>
    </div>
  );
}

// ===========================================================================
// Preserved section: Preferences
// ===========================================================================

const LANGUAGE_OPTIONS = [
  "Rust",
  "TypeScript",
  "Python",
  "Go",
  "C",
  "C++",
  "Assembly",
  "JavaScript",
  "Java",
  "Swift",
];

const CLOUD_OPTIONS = [
  "Google Cloud",
  "AWS",
  "Azure",
  "Cloudflare",
  "Fly.io",
  "DigitalOcean",
  "Self-hosted",
];

function PreferencesSection() {
  const [prefs, setPrefs] = useState<Preferences | null>(null);
  const [prefsLoading, setPrefsLoading] = useState(true);
  const [prefsError, setPrefsError] = useState<string | null>(null);
  const [prefsSaving, setPrefsSaving] = useState(false);
  const [prefsBanner, setPrefsBanner] = useState<Banner>(null);

  const loadPrefs = useCallback(() => {
    setPrefsLoading(true);
    setPrefsError(null);
    getPreferences()
      .then(setPrefs)
      .catch((e) => setPrefsError(String(e instanceof Error ? e.message : e)))
      .finally(() => setPrefsLoading(false));
  }, []);

  useEffect(() => {
    loadPrefs();
  }, [loadPrefs]);

  const setPref = (key: string, value: string) =>
    setPrefs((p) => (p ? { ...p, [key]: value } : p));

  const savePrefs = async () => {
    if (!prefs) return;
    setPrefsSaving(true);
    setPrefsBanner(null);
    try {
      const saved = await putPreferences({
        pref_languages: prefs.pref_languages,
        pref_cloud: prefs.pref_cloud,
      });
      setPrefs(saved);
      setPrefsBanner({ kind: "ok", text: "Preferences saved." });
    } catch (e) {
      setPrefsBanner({
        kind: "err",
        text: e instanceof Error ? e.message : String(e),
      });
    } finally {
      setPrefsSaving(false);
    }
  };

  return (
    <section className="rounded-xl border border-edge bg-panel p-6">
      <div className="flex items-center justify-between">
        <h2 className="text-lg font-semibold">Preferences</h2>
        <button
          onClick={savePrefs}
          disabled={prefsSaving || prefsLoading || !prefs}
          className="rounded-md bg-accent/20 px-3 py-1.5 text-sm text-accent transition hover:bg-accent/30 disabled:cursor-not-allowed disabled:opacity-40"
        >
          {prefsSaving ? "Saving…" : "Save preferences"}
        </button>
      </div>
      <p className="mt-1 text-xs text-slate-500">
        These suggest defaults for Claude features and migrations — they never
        force a choice.
      </p>

      {prefsLoading ? (
        <p className="mt-4 text-sm text-slate-400">Loading preferences…</p>
      ) : prefsError ? (
        <ErrorRow text={prefsError} onRetry={loadPrefs} />
      ) : prefs ? (
        <div className="mt-4 grid gap-4 sm:grid-cols-2">
          <label className="flex flex-col gap-1.5">
            <span className="text-xs font-medium text-slate-400">
              Preferred language
            </span>
            <input
              list="pref-languages"
              value={prefs.pref_languages}
              onChange={(e) => setPref("pref_languages", e.target.value)}
              className="rounded-md border border-edge bg-slate-900/60 px-3 py-2 text-sm text-slate-200 outline-none focus:border-accent"
            />
            <datalist id="pref-languages">
              {LANGUAGE_OPTIONS.map((l) => (
                <option key={l} value={l} />
              ))}
            </datalist>
          </label>

          <label className="flex flex-col gap-1.5">
            <span className="text-xs font-medium text-slate-400">
              Preferred cloud
            </span>
            <input
              list="pref-clouds"
              value={prefs.pref_cloud}
              onChange={(e) => setPref("pref_cloud", e.target.value)}
              className="rounded-md border border-edge bg-slate-900/60 px-3 py-2 text-sm text-slate-200 outline-none focus:border-accent"
            />
            <datalist id="pref-clouds">
              {CLOUD_OPTIONS.map((c) => (
                <option key={c} value={c} />
              ))}
            </datalist>
          </label>
        </div>
      ) : null}

      {prefsBanner && <BannerRow banner={prefsBanner} />}
    </section>
  );
}

// ===========================================================================
// Preserved section: Infrastructure registry + General settings
// ===========================================================================

const INFRA_KINDS = ["ssh", "vps", "database", "storage", "k8s", "other"];

const EMPTY_INFRA: CreateInfraBody = {
  name: "",
  kind: "ssh",
  host: "",
  port: 22,
  username: "",
  base_path: "",
  notes: "",
};

function InfrastructureSection() {
  // ---- Infra ----
  const [infra, setInfra] = useState<InfraResource[] | null>(null);
  const [infraLoading, setInfraLoading] = useState(true);
  const [infraError, setInfraError] = useState<string | null>(null);
  const [draft, setDraft] = useState<CreateInfraBody>(EMPTY_INFRA);
  const [creating, setCreating] = useState(false);
  const [infraBanner, setInfraBanner] = useState<Banner>(null);
  const [busyInfra, setBusyInfra] = useState<Record<number, "test" | "delete">>(
    {},
  );
  const [testResults, setTestResults] = useState<
    Record<number, InfraTestResult>
  >({});

  // ---- General settings (free-form global key/value rows) ----
  const [settings, setSettings] = useState<Setting[] | null>(null);
  const [settingsLoading, setSettingsLoading] = useState(true);
  const [settingsError, setSettingsError] = useState<string | null>(null);
  const [newKey, setNewKey] = useState("");
  const [newVal, setNewVal] = useState("");
  const [settingSaving, setSettingSaving] = useState(false);
  const [settingsBanner, setSettingsBanner] = useState<Banner>(null);

  const loadInfra = useCallback(() => {
    setInfraLoading(true);
    setInfraError(null);
    listInfra()
      .then(setInfra)
      .catch((e) => setInfraError(String(e instanceof Error ? e.message : e)))
      .finally(() => setInfraLoading(false));
  }, []);

  const loadSettings = useCallback(() => {
    setSettingsLoading(true);
    setSettingsError(null);
    getSettings("global")
      .then(setSettings)
      .catch((e) =>
        setSettingsError(String(e instanceof Error ? e.message : e)),
      )
      .finally(() => setSettingsLoading(false));
  }, []);

  useEffect(() => {
    loadInfra();
    loadSettings();
  }, [loadInfra, loadSettings]);

  const createResource = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!draft.name.trim() || !draft.host.trim()) {
      setInfraBanner({ kind: "err", text: "Name and host are required." });
      return;
    }
    setCreating(true);
    setInfraBanner(null);
    try {
      const created = await createInfra({
        ...draft,
        name: draft.name.trim(),
        host: draft.host.trim(),
        username: draft.username?.trim() || null,
        base_path: draft.base_path?.trim() || null,
        notes: draft.notes?.trim() || null,
      });
      setInfra((list) => (list ? [created, ...list] : [created]));
      setDraft(EMPTY_INFRA);
      setInfraBanner({ kind: "ok", text: `Added "${created.name}".` });
    } catch (err) {
      setInfraBanner({
        kind: "err",
        text: err instanceof Error ? err.message : String(err),
      });
    } finally {
      setCreating(false);
    }
  };

  const removeResource = async (id: number) => {
    setBusyInfra((b) => ({ ...b, [id]: "delete" }));
    setInfraBanner(null);
    try {
      await deleteInfra(id);
      setInfra((list) => (list ? list.filter((r) => r.id !== id) : list));
      setTestResults((t) => {
        const next = { ...t };
        delete next[id];
        return next;
      });
    } catch (err) {
      setInfraBanner({
        kind: "err",
        text: err instanceof Error ? err.message : String(err),
      });
    } finally {
      setBusyInfra((b) => {
        const next = { ...b };
        delete next[id];
        return next;
      });
    }
  };

  const runTest = async (id: number) => {
    setBusyInfra((b) => ({ ...b, [id]: "test" }));
    try {
      const result = await testInfra(id);
      setTestResults((t) => ({ ...t, [id]: result }));
    } catch (err) {
      setTestResults((t) => ({
        ...t,
        [id]: { ok: false, error: err instanceof Error ? err.message : String(err) },
      }));
    } finally {
      setBusyInfra((b) => {
        const next = { ...b };
        delete next[id];
        return next;
      });
    }
  };

  const addSetting = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!newKey.trim()) {
      setSettingsBanner({ kind: "err", text: "Key is required." });
      return;
    }
    setSettingSaving(true);
    setSettingsBanner(null);
    const row: Setting = {
      scope: "global",
      repo_id: null,
      key: newKey.trim(),
      value: newVal,
    };
    try {
      const saved = await putSetting(row);
      setSettings((list) => {
        const base = list ?? [];
        const without = base.filter((s) => s.key !== saved.key);
        return [...without, saved].sort((a, b) => a.key.localeCompare(b.key));
      });
      setNewKey("");
      setNewVal("");
      setSettingsBanner({ kind: "ok", text: `Saved "${saved.key}".` });
    } catch (err) {
      setSettingsBanner({
        kind: "err",
        text: err instanceof Error ? err.message : String(err),
      });
    } finally {
      setSettingSaving(false);
    }
  };

  return (
    <>
      <section className="rounded-xl border border-edge bg-panel p-6">
        <h2 className="text-lg font-semibold">Infrastructure registry</h2>
        <p className="mt-1 text-xs text-slate-500">
          SSH hosts, VPSes, and other resources. Use Test to verify
          connectivity.
        </p>

        {/* create form */}
        <form
          onSubmit={createResource}
          className="mt-4 grid gap-3 sm:grid-cols-2 lg:grid-cols-3"
        >
          <Field label="Name">
            <input
              value={draft.name}
              onChange={(e) => setDraft({ ...draft, name: e.target.value })}
              placeholder="prod-vps"
              className={inputCls}
            />
          </Field>
          <Field label="Kind">
            <select
              value={draft.kind}
              onChange={(e) => setDraft({ ...draft, kind: e.target.value })}
              className={inputCls}
            >
              {INFRA_KINDS.map((k) => (
                <option key={k} value={k}>
                  {k}
                </option>
              ))}
            </select>
          </Field>
          <Field label="Host">
            <input
              value={draft.host}
              onChange={(e) => setDraft({ ...draft, host: e.target.value })}
              placeholder="example.com"
              className={inputCls}
            />
          </Field>
          <Field label="Port">
            <input
              type="number"
              value={draft.port}
              onChange={(e) =>
                setDraft({ ...draft, port: Number(e.target.value) || 0 })
              }
              className={inputCls}
            />
          </Field>
          <Field label="Username">
            <input
              value={draft.username ?? ""}
              onChange={(e) => setDraft({ ...draft, username: e.target.value })}
              placeholder="root"
              className={inputCls}
            />
          </Field>
          <Field label="Base path">
            <input
              value={draft.base_path ?? ""}
              onChange={(e) =>
                setDraft({ ...draft, base_path: e.target.value })
              }
              placeholder="/srv/app"
              className={inputCls}
            />
          </Field>
          <div className="sm:col-span-2 lg:col-span-3">
            <Field label="Notes">
              <input
                value={draft.notes ?? ""}
                onChange={(e) => setDraft({ ...draft, notes: e.target.value })}
                placeholder="optional"
                className={inputCls}
              />
            </Field>
          </div>
          <div className="sm:col-span-2 lg:col-span-3">
            <button
              type="submit"
              disabled={creating}
              className="rounded-md bg-accent/20 px-4 py-2 text-sm text-accent transition hover:bg-accent/30 disabled:cursor-not-allowed disabled:opacity-40"
            >
              {creating ? "Adding…" : "Add resource"}
            </button>
          </div>
        </form>

        {infraBanner && <BannerRow banner={infraBanner} />}

        {/* list */}
        <div className="mt-5">
          {infraLoading ? (
            <p className="text-sm text-slate-400">Loading resources…</p>
          ) : infraError ? (
            <ErrorRow text={infraError} onRetry={loadInfra} />
          ) : !infra || infra.length === 0 ? (
            <p className="rounded-lg border border-dashed border-edge px-4 py-6 text-center text-sm text-slate-500">
              No infrastructure resources yet.
            </p>
          ) : (
            <ul className="space-y-2">
              {infra.map((r) => {
                const busy = busyInfra[r.id];
                const result = testResults[r.id];
                return (
                  <li
                    key={r.id}
                    className="rounded-lg border border-edge bg-slate-900/40 p-3"
                  >
                    <div className="flex flex-wrap items-center gap-3">
                      <span className="font-medium text-slate-200">
                        {r.name}
                      </span>
                      <span className="rounded bg-edge px-1.5 py-0.5 text-[11px] uppercase tracking-wide text-slate-400">
                        {r.kind}
                      </span>
                      <span className="text-sm text-slate-400">
                        {r.username ? `${r.username}@` : ""}
                        {r.host}:{r.port}
                      </span>
                      {result && (
                        <span
                          className={`text-xs ${
                            result.ok ? "text-emerald-400" : "text-rose-400"
                          }`}
                        >
                          {result.ok
                            ? "✓ reachable"
                            : `✕ ${result.error ?? "failed"}`}
                        </span>
                      )}
                      <div className="ml-auto flex gap-2">
                        <button
                          onClick={() => runTest(r.id)}
                          disabled={!!busy}
                          className="rounded-md border border-edge px-2.5 py-1 text-xs text-slate-300 transition hover:bg-edge disabled:opacity-40"
                        >
                          {busy === "test" ? "Testing…" : "Test"}
                        </button>
                        <button
                          onClick={() => removeResource(r.id)}
                          disabled={!!busy}
                          className="rounded-md border border-rose-500/40 px-2.5 py-1 text-xs text-rose-400 transition hover:bg-rose-500/10 disabled:opacity-40"
                        >
                          {busy === "delete" ? "Deleting…" : "Delete"}
                        </button>
                      </div>
                    </div>
                    {(r.base_path || r.notes) && (
                      <div className="mt-1.5 text-xs text-slate-500">
                        {r.base_path && <span>path: {r.base_path}</span>}
                        {r.base_path && r.notes && <span> · </span>}
                        {r.notes && <span>{r.notes}</span>}
                      </div>
                    )}
                  </li>
                );
              })}
            </ul>
          )}
        </div>
      </section>

      {/* ---- General settings ---- */}
      <section className="rounded-xl border border-edge bg-panel p-6">
        <h2 className="text-lg font-semibold">General settings</h2>
        <p className="mt-1 text-xs text-slate-500">
          Free-form global key/value configuration rows.
        </p>

        <form
          onSubmit={addSetting}
          className="mt-4 flex flex-wrap items-end gap-3"
        >
          <Field label="Key">
            <input
              value={newKey}
              onChange={(e) => setNewKey(e.target.value)}
              placeholder="some_key"
              className={inputCls}
            />
          </Field>
          <Field label="Value">
            <input
              value={newVal}
              onChange={(e) => setNewVal(e.target.value)}
              placeholder="value"
              className={inputCls}
            />
          </Field>
          <button
            type="submit"
            disabled={settingSaving}
            className="rounded-md bg-accent/20 px-4 py-2 text-sm text-accent transition hover:bg-accent/30 disabled:cursor-not-allowed disabled:opacity-40"
          >
            {settingSaving ? "Saving…" : "Save"}
          </button>
        </form>

        {settingsBanner && <BannerRow banner={settingsBanner} />}

        <div className="mt-5">
          {settingsLoading ? (
            <p className="text-sm text-slate-400">Loading settings…</p>
          ) : settingsError ? (
            <ErrorRow text={settingsError} onRetry={loadSettings} />
          ) : !settings || settings.length === 0 ? (
            <p className="rounded-lg border border-dashed border-edge px-4 py-6 text-center text-sm text-slate-500">
              No general settings stored.
            </p>
          ) : (
            <div className="overflow-hidden rounded-lg border border-edge">
              <table className="w-full text-sm">
                <thead className="bg-slate-900/60 text-left text-xs uppercase tracking-wide text-slate-500">
                  <tr>
                    <th className="px-3 py-2 font-medium">Key</th>
                    <th className="px-3 py-2 font-medium">Value</th>
                  </tr>
                </thead>
                <tbody>
                  {settings.map((s) => (
                    <tr
                      key={s.key}
                      className="border-t border-edge text-slate-300"
                    >
                      <td className="px-3 py-2 font-mono text-xs text-slate-200">
                        {s.key}
                      </td>
                      <td className="px-3 py-2 break-all">{s.value}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </div>
      </section>
    </>
  );
}

// ===========================================================================
// Preserved section: Suggest settings (AI)
// ===========================================================================

function SuggestSection({
  repos,
  reposLoading,
  reposError,
}: {
  repos: Repo[] | null;
  reposLoading: boolean;
  reposError: string | null;
}) {
  const [suggestRepoId, setSuggestRepoId] = useState<number | "">("");
  const [suggesting, setSuggesting] = useState(false);
  const [suggestion, setSuggestion] = useState<SuggestResponse | null>(null);
  const [suggestRepoName, setSuggestRepoName] = useState<string>("");
  const [checked, setChecked] = useState<Record<string, boolean>>({});
  const [applying, setApplying] = useState(false);
  const [suggestBanner, setSuggestBanner] = useState<Banner>(null);

  // Flatten a suggestion into the concrete files we can apply, keyed by path.
  const suggestedFiles: SuggestedFile[] = useMemo(() => {
    if (!suggestion) return [];
    const files: SuggestedFile[] = [];
    if (suggestion.claude_md.trim())
      files.push({ path: "CLAUDE.md", content: suggestion.claude_md });
    if (suggestion.settings_json.trim())
      files.push({
        path: ".claude/settings.json",
        content: suggestion.settings_json,
      });
    for (const dc of suggestion.dev_configs) {
      if (dc.path.trim()) files.push(dc);
    }
    return files;
  }, [suggestion]);

  const runSuggest = async () => {
    if (suggestRepoId === "") {
      setSuggestBanner({ kind: "err", text: "Pick a repository first." });
      return;
    }
    setSuggesting(true);
    setSuggestBanner(null);
    setSuggestion(null);
    setChecked({});
    const repoId = Number(suggestRepoId);
    const repoName =
      repos?.find((r) => r.id === repoId)?.full_name ?? `repo ${repoId}`;
    setSuggestRepoName(repoName);
    try {
      const res = await suggestSettings({ repo_id: repoId });
      setSuggestion(res);
      // Default every proposed file to checked.
      const next: Record<string, boolean> = {};
      if (res.claude_md.trim()) next["CLAUDE.md"] = true;
      if (res.settings_json.trim()) next[".claude/settings.json"] = true;
      for (const dc of res.dev_configs) {
        if (dc.path.trim()) next[dc.path] = true;
      }
      setChecked(next);
      setSuggestBanner({
        kind: "ok",
        text: `Suggestion ready for "${repoName}". Review the files below.`,
      });
    } catch (err) {
      setSuggestBanner({
        kind: "err",
        text: err instanceof Error ? err.message : String(err),
      });
    } finally {
      setSuggesting(false);
    }
  };

  const applySelected = async () => {
    if (suggestRepoId === "" || !suggestion) return;
    const files = suggestedFiles.filter((f) => checked[f.path]);
    if (files.length === 0) {
      setSuggestBanner({ kind: "err", text: "Select at least one file." });
      return;
    }
    setApplying(true);
    setSuggestBanner(null);
    try {
      const res = await applySuggestion({
        repo_id: Number(suggestRepoId),
        files,
      });
      setSuggestBanner({
        kind: "ok",
        text: res.ok
          ? `Applied ${files.length} file${files.length === 1 ? "" : "s"} to branch "${res.branch}". Review and open a PR to merge.`
          : "Apply reported failure.",
      });
    } catch (err) {
      setSuggestBanner({
        kind: "err",
        text: err instanceof Error ? err.message : String(err),
      });
    } finally {
      setApplying(false);
    }
  };

  const checkedCount = suggestedFiles.filter((f) => checked[f.path]).length;

  return (
    <section className="rounded-xl border border-edge bg-panel p-6">
      <h2 className="text-lg font-semibold">Suggest settings (AI)</h2>
      <p className="mt-1 text-xs text-slate-500">
        Ask Claude to propose a{" "}
        <code className="text-slate-400">CLAUDE.md</code>,{" "}
        <code className="text-slate-400">.claude/settings.json</code>, and dev
        config for a repo. Nothing is written until you apply — selected files
        land on the <span className="text-accent">repohub-staging</span> branch
        for you to review and merge.
      </p>

      <div className="mt-4 flex flex-wrap items-end gap-3">
        <Field label="Repository">
          {reposLoading ? (
            <span className="px-1 py-2 text-sm text-slate-400">
              Loading repos…
            </span>
          ) : reposError ? (
            <span className="px-1 py-2 text-sm text-rose-400">
              {reposError}
            </span>
          ) : (
            <select
              value={suggestRepoId}
              onChange={(e) =>
                setSuggestRepoId(
                  e.target.value === "" ? "" : Number(e.target.value),
                )
              }
              className={inputCls}
            >
              <option value="">Select a repo…</option>
              {(repos ?? []).map((r) => (
                <option key={r.id} value={r.id}>
                  {r.full_name}
                  {r.local_path ? "" : " (not cloned)"}
                </option>
              ))}
            </select>
          )}
        </Field>
        <button
          onClick={runSuggest}
          disabled={suggesting || suggestRepoId === ""}
          className="rounded-md bg-accent/20 px-4 py-2 text-sm text-accent transition hover:bg-accent/30 disabled:cursor-not-allowed disabled:opacity-40"
        >
          {suggesting ? "Asking Claude…" : "Suggest"}
        </button>
        {suggesting && (
          <span className="text-xs text-slate-500">
            This calls Claude and may take a while.
          </span>
        )}
      </div>

      {suggestBanner && <BannerRow banner={suggestBanner} />}

      {suggestion && (
        <div className="mt-5 space-y-3">
          <div className="flex items-center justify-between">
            <p className="text-sm text-slate-400">
              Proposed for{" "}
              <span className="font-medium text-slate-200">
                {suggestRepoName}
              </span>{" "}
              · {suggestedFiles.length} file
              {suggestedFiles.length === 1 ? "" : "s"} · {checkedCount} selected
            </p>
            <button
              onClick={applySelected}
              disabled={applying || checkedCount === 0}
              className="rounded-md bg-accent/20 px-3 py-1.5 text-sm text-accent transition hover:bg-accent/30 disabled:cursor-not-allowed disabled:opacity-40"
            >
              {applying ? "Applying…" : "Apply selected to staging"}
            </button>
          </div>

          {suggestion.rationale.trim() && (
            <div className="rounded-lg border border-edge bg-slate-900/40 p-3">
              <h3 className="text-xs font-medium uppercase tracking-wide text-slate-500">
                Rationale
              </h3>
              <p className="mt-1.5 whitespace-pre-wrap text-sm text-slate-300">
                {suggestion.rationale}
              </p>
            </div>
          )}

          {suggestedFiles.length === 0 ? (
            <p className="rounded-lg border border-dashed border-edge px-4 py-6 text-center text-sm text-slate-500">
              Claude did not propose any files.
            </p>
          ) : (
            suggestedFiles.map((f) => (
              <SuggestFile
                key={f.path}
                file={f}
                checked={!!checked[f.path]}
                onToggle={() =>
                  setChecked((c) => ({ ...c, [f.path]: !c[f.path] }))
                }
              />
            ))
          )}
        </div>
      )}
    </section>
  );
}

// Maps a proposed file path to a label and (best-effort) language hint.
function langOf(path: string): string {
  if (path.endsWith(".json")) return "json";
  if (path.endsWith(".md")) return "markdown";
  if (path.endsWith(".toml")) return "toml";
  if (path.endsWith(".yml") || path.endsWith(".yaml")) return "yaml";
  return "text";
}

function SuggestFile({
  file,
  checked,
  onToggle,
}: {
  file: SuggestedFile;
  checked: boolean;
  onToggle: () => void;
}) {
  const [open, setOpen] = useState(true);
  return (
    <div className="overflow-hidden rounded-lg border border-edge bg-slate-900/40">
      <div className="flex items-center gap-3 px-3 py-2">
        <input
          type="checkbox"
          checked={checked}
          onChange={onToggle}
          className="h-4 w-4 accent-accent"
          aria-label={`Apply ${file.path}`}
        />
        <button
          onClick={() => setOpen((o) => !o)}
          className="flex flex-1 items-center gap-2 text-left"
        >
          <span className="text-slate-400">{open ? "▾" : "▸"}</span>
          <span className="font-mono text-xs text-slate-200">{file.path}</span>
          <span className="rounded bg-edge px-1.5 py-0.5 text-[10px] uppercase tracking-wide text-slate-500">
            {langOf(file.path)}
          </span>
          <span className="ml-auto text-[11px] text-slate-500">
            {file.content.split("\n").length} lines
          </span>
        </button>
      </div>
      {open && (
        <pre className="max-h-80 overflow-auto border-t border-edge bg-slate-950/60 px-3 py-2 text-xs leading-relaxed text-slate-300">
          <code>{file.content}</code>
        </pre>
      )}
    </div>
  );
}

// ===========================================================================
// Shared primitives (used by preserved sections; exported for new sections)
// ===========================================================================

export type Banner = { kind: "ok" | "err"; text: string } | null;

export const inputCls =
  "w-full rounded-md border border-edge bg-slate-900/60 px-3 py-2 text-sm text-slate-200 outline-none focus:border-accent";

export function Field({
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

export function BannerRow({ banner }: { banner: NonNullable<Banner> }) {
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

export function ErrorRow({
  text,
  onRetry,
}: {
  text: string;
  onRetry: () => void;
}) {
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
