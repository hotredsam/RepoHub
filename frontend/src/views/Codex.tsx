import { useCallback, useEffect, useState } from "react";
import {
  getCodex,
  putCodex,
  issueCodexToken,
  killCodex,
  listCodexCredentials,
  revokeCodexCredential,
} from "../lib/api";
import type {
  CodexState,
  CodexConfigBody,
  CodexCredential,
  IssuedToken,
} from "../lib/types";
import { Field, inputCls, ErrorRow } from "./Settings";

// Codex.tsx (P19)
// Control panel for the ChatGPT Codex bridge — an EXTERNAL agent that, when
// enabled, can call this app's API to execute code on this Mac. The whole
// surface is deliberately scary: a master enable toggle (OFF by default), a
// destructive-op confirmation toggle (ON by default), a token panel that shows
// a freshly minted bearer token exactly ONCE, a big red kill switch that
// disables access and revokes every credential, and a read-only list of the
// API surface Codex is allowed to reach. Access is tailnet-only.

// The read-only summary of what an authenticated Codex bearer may call. This is
// documentation for the operator — the backend gate is the real enforcement.
const API_SURFACE: { method: string; path: string; what: string }[] = [
  { method: "GET", path: "/api/repos", what: "List tracked repositories" },
  { method: "POST", path: "/api/repos/:id/pull", what: "Pull a repo (fast-forward)" },
  { method: "GET", path: "/api/prompts", what: "Search prompt / transcript history" },
  { method: "POST", path: "/api/bulk/prompt", what: "Run a prompt across repos" },
  { method: "POST", path: "/api/connections/integrate", what: "Scaffold a cross-repo integration" },
  { method: "POST", path: "/api/settings/apply-suggestion", what: "Write config to a staging branch" },
  { method: "GET", path: "/ws/claude", what: "Drive an interactive Claude session" },
];

export default function Codex() {
  const [state, setState] = useState<CodexState | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  // Per-toggle save state so each switch flips independently.
  const [savingEnabled, setSavingEnabled] = useState(false);
  const [savingConfirm, setSavingConfirm] = useState(false);
  const [banner, setBanner] = useState<
    { kind: "ok" | "err"; text: string } | null
  >(null);

  // Credentials list + token issuance.
  const [creds, setCreds] = useState<CodexCredential[] | null>(null);
  const [credsLoading, setCredsLoading] = useState(true);
  const [credsError, setCredsError] = useState<string | null>(null);
  const [label, setLabel] = useState("");
  const [issuing, setIssuing] = useState(false);
  // The raw token is only ever held here, transiently, until dismissed.
  const [issued, setIssued] = useState<IssuedToken | null>(null);
  const [copied, setCopied] = useState(false);
  const [busyCred, setBusyCred] = useState<Record<number, boolean>>({});

  // Kill switch.
  const [killConfirm, setKillConfirm] = useState(false);
  const [killing, setKilling] = useState(false);

  const loadState = useCallback(() => {
    setLoading(true);
    setError(null);
    getCodex()
      .then(setState)
      .catch((e) => setError(String(e instanceof Error ? e.message : e)))
      .finally(() => setLoading(false));
  }, []);

  const loadCreds = useCallback(() => {
    setCredsLoading(true);
    setCredsError(null);
    listCodexCredentials()
      .then(setCreds)
      .catch((e) => setCredsError(String(e instanceof Error ? e.message : e)))
      .finally(() => setCredsLoading(false));
  }, []);

  useEffect(() => {
    loadState();
    loadCreds();
  }, [loadState, loadCreds]);

  const saveConfig = async (
    body: CodexConfigBody,
    setSaving: (b: boolean) => void,
  ) => {
    setSaving(true);
    setBanner(null);
    try {
      const next = await putCodex(body);
      setState(next);
    } catch (e) {
      setBanner({ kind: "err", text: e instanceof Error ? e.message : String(e) });
    } finally {
      setSaving(false);
    }
  };

  const toggleEnabled = () => {
    if (!state) return;
    void saveConfig({ enabled: !state.config.enabled }, setSavingEnabled);
  };

  const toggleConfirm = () => {
    if (!state) return;
    void saveConfig(
      { destructive_confirm: !state.config.destructive_confirm },
      setSavingConfirm,
    );
  };

  const issue = async () => {
    setIssuing(true);
    setBanner(null);
    setIssued(null);
    setCopied(false);
    try {
      const token = await issueCodexToken(label.trim() || undefined);
      setIssued(token);
      setLabel("");
      // Refresh the metadata list so the new credential shows up.
      loadCreds();
      loadState();
    } catch (e) {
      setBanner({ kind: "err", text: e instanceof Error ? e.message : String(e) });
    } finally {
      setIssuing(false);
    }
  };

  const copyToken = async () => {
    if (!issued) return;
    try {
      await navigator.clipboard.writeText(issued.token);
      setCopied(true);
    } catch {
      setCopied(false);
    }
  };

  const revoke = async (id: number) => {
    setBusyCred((b) => ({ ...b, [id]: true }));
    setBanner(null);
    try {
      await revokeCodexCredential(id);
      loadCreds();
      loadState();
    } catch (e) {
      setBanner({ kind: "err", text: e instanceof Error ? e.message : String(e) });
    } finally {
      setBusyCred((b) => {
        const next = { ...b };
        delete next[id];
        return next;
      });
    }
  };

  const doKill = async () => {
    setKilling(true);
    setBanner(null);
    try {
      await killCodex();
      setIssued(null);
      setKillConfirm(false);
      loadState();
      loadCreds();
      setBanner({
        kind: "ok",
        text: "Kill switch engaged. Codex access disabled and all credentials revoked.",
      });
    } catch (e) {
      setBanner({ kind: "err", text: e instanceof Error ? e.message : String(e) });
    } finally {
      setKilling(false);
    }
  };

  const activeCreds = (creds ?? []).filter((c) => !c.revoked);

  return (
    <div className="mx-auto max-w-4xl space-y-6">
      <div>
        <h1 className="text-2xl font-semibold tracking-tight">Codex</h1>
        <p className="mt-1 text-sm text-slate-400">
          Bridge for ChatGPT Codex — an external agent that can call this app's
          API to act on your repos.
        </p>
      </div>

      {/* ---- danger preamble ---- */}
      <div className="rounded-xl border border-rose-500/40 bg-rose-500/10 p-4 text-sm text-rose-200">
        <p className="font-semibold text-rose-300">
          Full access means external-agent code execution on this Mac.
        </p>
        <p className="mt-1.5 text-rose-200/90">
          Enabling Codex lets an outside agent run prompts, scaffold code, and
          push changes to staging branches through your account. Only enable
          this over the tailnet ­— never expose it to the public internet — and
          revoke tokens the moment you are done. The kill switch below cuts off
          access instantly.
        </p>
      </div>

      {banner && (
        <p
          className={`rounded-md px-3 py-2 text-sm ${
            banner.kind === "ok"
              ? "bg-emerald-500/10 text-emerald-400"
              : "bg-rose-500/10 text-rose-400"
          }`}
        >
          {banner.text}
        </p>
      )}

      {loading ? (
        <p className="text-sm text-slate-400">Loading Codex status…</p>
      ) : error ? (
        <ErrorRow text={error} onRetry={loadState} />
      ) : state ? (
        <>
          {/* ---- master toggles ---- */}
          <section className="rounded-xl border border-edge bg-panel p-6">
            <h2 className="text-lg font-semibold">Access controls</h2>
            <p className="mt-1 text-xs text-slate-500">
              Master switch for the Codex bridge and the destructive-op
              confirmation guard.
            </p>

            <div className="mt-4 space-y-3">
              <ToggleRow
                title="Enable Codex access"
                detail="When on, valid Codex bearer tokens may call the API surface listed below. Off by default."
                checked={state.config.enabled}
                saving={savingEnabled}
                onToggle={toggleEnabled}
                danger
              />
              <ToggleRow
                title="Destructive-op confirmation required"
                detail="Require Codex to present a short-lived confirm token for mutating operations. On by default — turning this off lets Codex mutate without a second step."
                checked={state.config.destructive_confirm}
                saving={savingConfirm}
                onToggle={toggleConfirm}
              />
            </div>

            <div className="mt-4 flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-slate-500">
              <span>
                Bridge:{" "}
                <span
                  className={
                    state.config.enabled ? "text-emerald-400" : "text-slate-400"
                  }
                >
                  {state.config.enabled ? "enabled" : "disabled"}
                </span>
              </span>
              <span>
                Token prefix:{" "}
                <span className="font-mono text-slate-300">
                  {state.config.token_prefix}
                </span>
              </span>
              <span>
                Active credentials:{" "}
                <span className="text-slate-300">
                  {state.status.active_credentials}
                </span>
              </span>
            </div>
          </section>

          {/* ---- token panel ---- */}
          <section className="rounded-xl border border-edge bg-panel p-6">
            <div className="flex items-center justify-between">
              <h2 className="text-lg font-semibold">Bearer tokens</h2>
              <button
                onClick={() => void issue()}
                disabled={issuing}
                className="rounded-md bg-accent/20 px-3 py-1.5 text-sm text-accent transition hover:bg-accent/30 disabled:cursor-not-allowed disabled:opacity-40"
              >
                {issuing
                  ? "Issuing…"
                  : activeCreds.length > 0
                    ? "Issue / rotate token"
                    : "Issue token"}
              </button>
            </div>
            <p className="mt-1 text-xs text-slate-500">
              Issue a bearer token for Codex to authenticate. The raw token is
              shown <span className="text-slate-300">only once</span> — copy it
              now or rotate to mint a new one.
            </p>

            <div className="mt-4 flex flex-wrap items-end gap-3">
              <Field label="Label (optional)">
                <input
                  value={label}
                  onChange={(e) => setLabel(e.target.value)}
                  placeholder="laptop-codex"
                  className={inputCls}
                />
              </Field>
            </div>

            {/* one-time raw token reveal */}
            {issued && (
              <div className="mt-4 rounded-lg border border-amber-500/40 bg-amber-500/10 p-4">
                <div className="flex items-center justify-between gap-3">
                  <p className="text-sm font-medium text-amber-300">
                    New token
                    {issued.credential.label
                      ? ` "${issued.credential.label}"`
                      : ""}{" "}
                    — copy it now
                  </p>
                  <button
                    onClick={() => setIssued(null)}
                    className="rounded border border-amber-500/40 px-2 py-0.5 text-xs text-amber-200 transition hover:bg-amber-500/20"
                  >
                    Dismiss
                  </button>
                </div>
                <p className="mt-1 text-xs text-amber-200/80">
                  This is the only time the full token is displayed. Store it
                  somewhere safe — it will never be shown again.
                </p>
                <div className="mt-3 flex items-center gap-2">
                  <code className="flex-1 select-all break-all rounded-md border border-edge bg-slate-950/70 px-3 py-2 font-mono text-xs text-slate-200">
                    {issued.token}
                  </code>
                  <button
                    onClick={() => void copyToken()}
                    className="shrink-0 rounded-md bg-amber-500/20 px-3 py-2 text-xs text-amber-200 transition hover:bg-amber-500/30"
                  >
                    {copied ? "Copied" : "Copy"}
                  </button>
                </div>
              </div>
            )}

            {/* credential metadata list */}
            <div className="mt-5">
              {credsLoading ? (
                <p className="text-sm text-slate-400">Loading credentials…</p>
              ) : credsError ? (
                <ErrorRow text={credsError} onRetry={loadCreds} />
              ) : !creds || creds.length === 0 ? (
                <p className="rounded-lg border border-dashed border-edge px-4 py-6 text-center text-sm text-slate-500">
                  No tokens issued yet.
                </p>
              ) : (
                <ul className="space-y-2">
                  {creds.map((c) => {
                    const revoked = c.revoked;
                    const busy = !!busyCred[c.id];
                    return (
                      <li
                        key={c.id}
                        className="flex flex-wrap items-center gap-3 rounded-lg border border-edge bg-slate-900/40 p-3"
                      >
                        <span className="font-medium text-slate-200">
                          {c.label || `token #${c.id}`}
                        </span>
                        {revoked ? (
                          <span className="rounded bg-edge px-1.5 py-0.5 text-[11px] uppercase tracking-wide text-slate-500">
                            revoked
                          </span>
                        ) : (
                          <span className="rounded bg-emerald-500/15 px-1.5 py-0.5 text-[11px] uppercase tracking-wide text-emerald-400">
                            active
                          </span>
                        )}
                        <span className="font-mono text-xs text-slate-500">
                          #{c.id}
                        </span>
                        <span className="text-xs text-slate-500">
                          issued {c.created_at}
                          {c.last_used_at
                            ? ` · last used ${c.last_used_at}`
                            : " · never used"}
                          {revoked && c.revoked_at
                            ? ` · revoked ${c.revoked_at}`
                            : ""}
                        </span>
                        {!revoked && (
                          <button
                            onClick={() => void revoke(c.id)}
                            disabled={busy}
                            className="ml-auto rounded-md border border-rose-500/40 px-2.5 py-1 text-xs text-rose-400 transition hover:bg-rose-500/10 disabled:opacity-40"
                          >
                            {busy ? "Revoking…" : "Revoke"}
                          </button>
                        )}
                      </li>
                    );
                  })}
                </ul>
              )}
            </div>
          </section>

          {/* ---- API surface ---- */}
          <section className="rounded-xl border border-edge bg-panel p-6">
            <h2 className="text-lg font-semibold">API surface Codex can call</h2>
            <p className="mt-1 text-xs text-slate-500">
              Read-only reference. When access is enabled, an authenticated
              Codex bearer can reach these endpoints. Destructive routes are
              gated by the confirmation toggle above.
            </p>
            <div className="mt-4 overflow-hidden rounded-lg border border-edge">
              <table className="w-full text-sm">
                <thead className="bg-slate-900/60 text-left text-xs uppercase tracking-wide text-slate-500">
                  <tr>
                    <th className="px-3 py-2 font-medium">Method</th>
                    <th className="px-3 py-2 font-medium">Path</th>
                    <th className="px-3 py-2 font-medium">What it does</th>
                  </tr>
                </thead>
                <tbody>
                  {API_SURFACE.map((r) => (
                    <tr
                      key={`${r.method} ${r.path}`}
                      className="border-t border-edge text-slate-300"
                    >
                      <td className="px-3 py-2 font-mono text-xs text-accent">
                        {r.method}
                      </td>
                      <td className="px-3 py-2 font-mono text-xs text-slate-200">
                        {r.path}
                      </td>
                      <td className="px-3 py-2 text-slate-400">{r.what}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          </section>

          {/* ---- kill switch ---- */}
          <section className="rounded-xl border border-rose-500/40 bg-rose-500/5 p-6">
            <h2 className="text-lg font-semibold text-rose-300">Kill switch</h2>
            <p className="mt-1 text-xs text-rose-200/80">
              Immediately disables Codex access, stops any running bridge
              process, and revokes every credential. Use this if a token leaks
              or anything looks wrong.
            </p>
            <div className="mt-4">
              <button
                onClick={() => setKillConfirm(true)}
                disabled={killing}
                className="rounded-md bg-rose-600 px-5 py-2.5 text-sm font-semibold text-white transition hover:bg-rose-500 disabled:cursor-not-allowed disabled:opacity-50"
              >
                KILL SWITCH — disable & revoke everything
              </button>
            </div>
          </section>
        </>
      ) : null}

      {/* ---- kill confirmation modal ---- */}
      {killConfirm && (
        <div
          className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-4"
          onClick={() => !killing && setKillConfirm(false)}
        >
          <div
            className="w-full max-w-md rounded-xl border border-rose-500/40 bg-panel p-6 shadow-xl"
            onClick={(e) => e.stopPropagation()}
          >
            <h2 className="text-base font-semibold text-rose-300">
              Engage the kill switch?
            </h2>
            <p className="mt-2 text-sm text-slate-300">
              This disables Codex access and revokes{" "}
              <span className="font-medium text-slate-100">
                all {activeCreds.length} active credential
                {activeCreds.length === 1 ? "" : "s"}
              </span>
              . Codex will be cut off immediately. This cannot be undone — you
              will have to issue new tokens to re-enable access.
            </p>
            <div className="mt-5 flex justify-end gap-2">
              <button
                onClick={() => setKillConfirm(false)}
                disabled={killing}
                className="rounded-md border border-edge px-4 py-2 text-sm text-slate-300 transition hover:bg-edge disabled:opacity-50"
              >
                Cancel
              </button>
              <button
                onClick={() => void doKill()}
                disabled={killing}
                className="rounded-md bg-rose-600 px-4 py-2 text-sm font-semibold text-white transition hover:bg-rose-500 disabled:cursor-not-allowed disabled:opacity-50"
              >
                {killing ? "Killing…" : "Yes, kill it"}
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

// ---- toggle switch row ----

function ToggleRow({
  title,
  detail,
  checked,
  saving,
  onToggle,
  danger = false,
}: {
  title: string;
  detail: string;
  checked: boolean;
  saving: boolean;
  onToggle: () => void;
  danger?: boolean;
}) {
  // On color depends on whether "on" is the dangerous state (master enable) or
  // the safe state (confirmation guard).
  const onColor = danger ? "bg-rose-600" : "bg-emerald-600";
  return (
    <div className="flex items-start justify-between gap-4 rounded-lg border border-edge bg-slate-900/40 p-3">
      <div className="min-w-0">
        <p className="text-sm font-medium text-slate-200">{title}</p>
        <p className="mt-0.5 text-xs text-slate-500">{detail}</p>
      </div>
      <button
        role="switch"
        aria-checked={checked}
        aria-label={title}
        onClick={onToggle}
        disabled={saving}
        className={`relative mt-0.5 inline-flex h-6 w-11 shrink-0 items-center rounded-full transition disabled:opacity-50 ${
          checked ? onColor : "bg-edge"
        }`}
      >
        <span
          className={`inline-block h-4 w-4 transform rounded-full bg-white transition ${
            checked ? "translate-x-6" : "translate-x-1"
          }`}
        />
      </button>
    </div>
  );
}
