import { useCallback, useEffect, useState } from "react";
import {
  getPreferences,
  putPreferences,
  listInfra,
  createInfra,
  deleteInfra,
  testInfra,
  getSettings,
  putSetting,
} from "../lib/api";
import type {
  Preferences,
  InfraResource,
  CreateInfraBody,
  InfraTestResult,
  Setting,
} from "../lib/types";

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

type Banner = { kind: "ok" | "err"; text: string } | null;

export default function Settings() {
  // ---- Preferences ----
  const [prefs, setPrefs] = useState<Preferences | null>(null);
  const [prefsLoading, setPrefsLoading] = useState(true);
  const [prefsError, setPrefsError] = useState<string | null>(null);
  const [prefsSaving, setPrefsSaving] = useState(false);
  const [prefsBanner, setPrefsBanner] = useState<Banner>(null);

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

  // ---- loaders ----
  const loadPrefs = useCallback(() => {
    setPrefsLoading(true);
    setPrefsError(null);
    getPreferences()
      .then(setPrefs)
      .catch((e) => setPrefsError(String(e instanceof Error ? e.message : e)))
      .finally(() => setPrefsLoading(false));
  }, []);

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
    loadPrefs();
    loadInfra();
    loadSettings();
  }, [loadPrefs, loadInfra, loadSettings]);

  // ---- prefs handlers ----
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

  // ---- infra handlers ----
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

  // ---- general setting handler ----
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
    <div className="mx-auto max-w-4xl space-y-6">
      <div>
        <h1 className="text-2xl font-semibold tracking-tight">Settings</h1>
        <p className="mt-1 text-sm text-slate-400">
          Preferences, infrastructure registry, and general configuration.
        </p>
      </div>

      {/* ---- Preferences ---- */}
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

      {/* ---- Infra registry ---- */}
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
    </div>
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
