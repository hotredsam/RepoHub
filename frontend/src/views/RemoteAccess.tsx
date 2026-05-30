import { useCallback, useEffect, useState } from "react";
import { inputCls } from "./Settings";
import {
  getTailscale,
  enableTailscaleServe,
  disableTailscaleServe,
  getAuthStatus,
  putAuthConfig,
  listSessions,
  revokeSession,
} from "../lib/api";
import type {
  TsStatus,
  AuthStatus,
  AuthConfigBody,
  SessionRow,
} from "../lib/types";

// ===========================================================================
// RemoteAccess.tsx (P18) — expose RepoHub beyond loopback, safely.
//
// Three stacked sections:
//   1. Tailscale serve — local install / login / serve status, the published
//      HTTPS URL (with copy), Enable / Disable, and a clear "Funnel is never
//      enabled" note (RepoHub is tailnet-only on purpose).
//   2. Authorized accounts — the email allowlist for Tailscale identity logins
//      plus the two gate toggles (loopback-allowed, remote-required).
//   3. Active sessions — the session table with per-row Revoke.
//
// All endpoint helpers + payload types come from the shared lib/api.ts +
// lib/types.ts (the authoritative contract against the Rust backend).
//
// Graceful degradation: with NO Tailscale installed the status endpoint reports
// installed=false and the section walks the user through setup; nothing throws
// the page into an error state on its own.
// ===========================================================================

// The fixed tailnet name for this Mac (from the build contract). The published
// URL never changes shape — only whether serve is currently running does.
const PUBLISHED_URL = "https://samuels-mac-mini.tail97ef37.ts.net";

// ---------------------------------------------------------------------------

export default function RemoteAccess() {
  return (
    <div className="mx-auto max-w-4xl space-y-6">
      <div>
        <h1 className="text-2xl font-semibold tracking-tight">Remote access</h1>
        <p className="mt-1 text-sm text-slate-400">
          RepoHub binds to <code className="text-slate-400">127.0.0.1</code> and
          is reached from your other devices over your private{" "}
          <span className="text-accent">Tailscale</span> network. Publish it with
          Tailscale serve, allowlist the Google accounts that may sign in, and
          review or revoke active sessions below.
        </p>
      </div>

      <TailscaleSection />
      <AuthorizedAccountsSection />
      <SessionsSection />
    </div>
  );
}

// ===========================================================================
// 1) Tailscale serve
// ===========================================================================

function TailscaleSection() {
  const [status, setStatus] = useState<TsStatus | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [banner, setBanner] = useState<Banner>(null);
  const [busy, setBusy] = useState<null | "enable" | "disable">(null);
  const [copied, setCopied] = useState(false);

  const load = useCallback(() => {
    setLoading(true);
    setError(null);
    getTailscale()
      .then(setStatus)
      .catch((e) => setError(e instanceof Error ? e.message : String(e)))
      .finally(() => setLoading(false));
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  const url = status?.published_url ?? PUBLISHED_URL;

  const copyUrl = async () => {
    try {
      await navigator.clipboard.writeText(url);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      setBanner({ kind: "err", text: "Could not copy to clipboard." });
    }
  };

  const enable = async () => {
    setBusy("enable");
    setBanner(null);
    try {
      const res = await enableTailscaleServe();
      setStatus(res.status);
      setBanner(
        res.funnel_warning
          ? { kind: "err", text: res.funnel_warning }
          : {
              kind: "ok",
              text: "Tailscale serve enabled. RepoHub is reachable on your tailnet over HTTPS.",
            },
      );
    } catch (e) {
      setBanner({ kind: "err", text: e instanceof Error ? e.message : String(e) });
    } finally {
      setBusy(null);
    }
  };

  const disable = async () => {
    setBusy("disable");
    setBanner(null);
    try {
      const res = await disableTailscaleServe();
      setStatus(res.status);
      setBanner({ kind: "ok", text: "Tailscale serve disabled." });
    } catch (e) {
      setBanner({ kind: "err", text: e instanceof Error ? e.message : String(e) });
    } finally {
      setBusy(null);
    }
  };

  const ready = !!status && status.installed && status.logged_in;
  const serving = !!status && status.serve_enabled;

  return (
    <section className="rounded-xl border border-edge bg-panel p-6">
      <div className="flex items-center justify-between">
        <h2 className="text-lg font-semibold">Tailscale serve</h2>
        <button
          onClick={load}
          disabled={loading}
          className="rounded-md border border-edge px-3 py-1.5 text-sm text-slate-300 transition hover:bg-edge hover:text-slate-100 disabled:cursor-not-allowed disabled:opacity-40"
        >
          {loading ? "Checking…" : "Refresh"}
        </button>
      </div>
      <p className="mt-1 text-xs text-slate-500">
        Serve proxies HTTPS on your tailnet to RepoHub on loopback. It is
        reachable only by devices signed in to your Tailscale network.
      </p>

      {banner && <BannerRow banner={banner} />}

      <div className="mt-4">
        {loading ? (
          <p className="text-sm text-slate-400">Checking Tailscale status…</p>
        ) : error ? (
          <ErrorRow text={error} onRetry={load} />
        ) : status ? (
          <>
            <div className="grid gap-2 sm:grid-cols-3">
              <StatusRow
                label="Tailscale CLI"
                ok={status.installed}
                okText="installed"
                badText="not found on PATH"
              />
              <StatusRow
                label="Logged in"
                ok={status.logged_in}
                okText={status.dns_name ?? "signed in"}
                badText="not signed in"
              />
              <StatusRow
                label="Serve"
                ok={status.serve_enabled}
                okText="serving over HTTPS"
                badText="not serving"
              />
            </div>

            {/* published URL */}
            <div className="mt-4 rounded-lg border border-edge bg-slate-900/40 p-4">
              <h3 className="text-sm font-medium text-slate-200">
                Published URL
              </h3>
              <div className="mt-2 flex flex-wrap items-center gap-2">
                <code className="flex-1 break-all rounded-md border border-edge bg-slate-950/60 px-3 py-2 font-mono text-xs text-slate-200">
                  {url}
                </code>
                <button
                  onClick={copyUrl}
                  className="rounded-md border border-edge px-3 py-2 text-xs text-slate-300 transition hover:bg-edge hover:text-slate-100"
                >
                  {copied ? "Copied" : "Copy"}
                </button>
                {serving && (
                  <a
                    href={url}
                    target="_blank"
                    rel="noreferrer"
                    className="rounded-md border border-edge px-3 py-2 text-xs text-slate-300 transition hover:bg-edge hover:text-slate-100"
                  >
                    Open
                  </a>
                )}
              </div>
              <p className="mt-2 text-xs text-slate-500">
                {serving
                  ? "Live now for devices on your tailnet."
                  : "This is where RepoHub will be published once serve is enabled."}
              </p>
            </div>

            {/* enable / disable */}
            <div className="mt-4 flex flex-wrap items-center gap-3">
              {serving ? (
                <button
                  onClick={disable}
                  disabled={busy !== null}
                  className="rounded-md border border-rose-500/40 px-4 py-2 text-sm text-rose-300 transition hover:bg-rose-500/10 disabled:cursor-not-allowed disabled:opacity-40"
                >
                  {busy === "disable" ? "Disabling…" : "Disable serve"}
                </button>
              ) : (
                <button
                  onClick={enable}
                  disabled={busy !== null || !ready}
                  className="rounded-md bg-accent/20 px-4 py-2 text-sm text-accent transition hover:bg-accent/30 disabled:cursor-not-allowed disabled:opacity-40"
                  title={
                    ready
                      ? undefined
                      : "Install and sign in to Tailscale first."
                  }
                >
                  {busy === "enable" ? "Enabling…" : "Enable serve"}
                </button>
              )}
              {!ready && (
                <span className="text-xs text-slate-500">
                  Install and sign in to Tailscale to enable serve.
                </span>
              )}
            </div>

            {/* Funnel note — always shown */}
            <div className="mt-4 rounded-md border border-amber-500/30 bg-amber-500/5 px-3 py-2 text-xs text-amber-200">
              Funnel is never enabled. RepoHub is published to your private
              tailnet only — it is never exposed to the public internet.
            </div>

            {!ready && <TailscaleSetup status={status} />}
          </>
        ) : null}
      </div>
    </section>
  );
}

// Step-by-step setup, shown when the CLI is missing or not signed in.
function TailscaleSetup({ status }: { status: TsStatus }) {
  const steps: { title: string; cmd?: string; note: string; done: boolean }[] = [
    {
      title: "Install Tailscale",
      note: status.installed
        ? "Detected on PATH."
        : "Install Tailscale, then make sure the `tailscale` CLI is on your PATH. See tailscale.com/download.",
      done: status.installed,
    },
    {
      title: "Sign in to your tailnet",
      cmd: "tailscale up",
      note: status.logged_in
        ? "Signed in."
        : "Authenticate this Mac to your Tailscale network.",
      done: status.logged_in,
    },
    {
      title: "Enable serve",
      note: "Use the Enable serve button above once the CLI is installed and signed in.",
      done: status.serve_enabled,
    },
  ];

  return (
    <div className="mt-5 rounded-lg border border-amber-500/30 bg-amber-500/5 p-4">
      <h3 className="text-sm font-medium text-amber-200">Set up Tailscale</h3>
      <p className="mt-1 text-xs text-slate-400">
        Tailscale is not ready in this environment yet, so remote access stays
        off and RepoHub keeps running on loopback. Complete these steps on the
        machine running RepoHub, then press Refresh.
      </p>
      <ol className="mt-4 space-y-3">
        {steps.map((s, i) => (
          <li
            key={s.title}
            className="rounded-lg border border-edge bg-slate-900/40 p-3"
          >
            <div className="flex items-center gap-2">
              <span
                className={`flex h-5 w-5 shrink-0 items-center justify-center rounded-full text-[11px] font-medium ${
                  s.done
                    ? "bg-emerald-500/20 text-emerald-300"
                    : "bg-edge text-slate-400"
                }`}
              >
                {s.done ? "✓" : i + 1}
              </span>
              <span className="text-sm font-medium text-slate-200">
                {s.title}
              </span>
            </div>
            {s.cmd && (
              <pre className="mt-2 overflow-auto rounded-md border border-edge bg-slate-950/60 px-3 py-2 font-mono text-xs text-slate-300">
                <code>{s.cmd}</code>
              </pre>
            )}
            <p className="mt-1.5 text-xs text-slate-500">{s.note}</p>
          </li>
        ))}
      </ol>
    </div>
  );
}

// ===========================================================================
// 2) Authorized accounts (email allowlist + gate toggles)
// ===========================================================================

function AuthorizedAccountsSection() {
  const [auth, setAuth] = useState<AuthStatus | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [banner, setBanner] = useState<Banner>(null);

  const [newEmail, setNewEmail] = useState("");
  const [adding, setAdding] = useState(false);
  // Tracks the email currently being removed (for a per-row busy state).
  const [removing, setRemoving] = useState<string | null>(null);
  // Tracks which toggle is mid-flight.
  const [toggling, setToggling] = useState<null | "loopback" | "remote">(null);

  const load = useCallback(() => {
    setLoading(true);
    setError(null);
    getAuthStatus()
      .then(setAuth)
      .catch((e) => setError(e instanceof Error ? e.message : String(e)))
      .finally(() => setLoading(false));
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  const addEmail = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!auth) return;
    const email = newEmail.trim().toLowerCase();
    if (!email) {
      setBanner({ kind: "err", text: "Enter an email address." });
      return;
    }
    if (auth.allowed_emails.some((x) => x.toLowerCase() === email)) {
      setBanner({ kind: "err", text: `"${email}" is already allowed.` });
      return;
    }
    setAdding(true);
    setBanner(null);
    try {
      // ConfigBody.allowed_emails is a comma/space-separated string on the
      // backend; join the array before sending.
      await putAuthConfig({
        allowed_emails: [...auth.allowed_emails, email].join(","),
      });
      await load();
      setNewEmail("");
      setBanner({ kind: "ok", text: `Added "${email}".` });
    } catch (err) {
      setBanner({ kind: "err", text: err instanceof Error ? err.message : String(err) });
    } finally {
      setAdding(false);
    }
  };

  const removeEmail = async (email: string) => {
    if (!auth) return;
    setRemoving(email);
    setBanner(null);
    try {
      await putAuthConfig({
        allowed_emails: auth.allowed_emails
          .filter((x) => x !== email)
          .join(","),
      });
      await load();
      setBanner({ kind: "ok", text: `Removed "${email}".` });
    } catch (err) {
      setBanner({ kind: "err", text: err instanceof Error ? err.message : String(err) });
    } finally {
      setRemoving(null);
    }
  };

  const toggle = async (which: "loopback" | "remote", value: boolean) => {
    if (!auth) return;
    setToggling(which);
    setBanner(null);
    try {
      const patch: AuthConfigBody =
        which === "loopback"
          ? { loopback_allowed: value }
          : { remote_required: value };
      await putAuthConfig(patch);
      await load();
    } catch (err) {
      setBanner({ kind: "err", text: err instanceof Error ? err.message : String(err) });
    } finally {
      setToggling(null);
    }
  };

  return (
    <section className="rounded-xl border border-edge bg-panel p-6">
      <div className="flex items-center justify-between">
        <h2 className="text-lg font-semibold">Authorized accounts</h2>
        <button
          onClick={load}
          disabled={loading}
          className="rounded-md border border-edge px-3 py-1.5 text-sm text-slate-300 transition hover:bg-edge hover:text-slate-100 disabled:cursor-not-allowed disabled:opacity-40"
        >
          {loading ? "Loading…" : "Refresh"}
        </button>
      </div>
      <p className="mt-1 text-xs text-slate-500">
        Only these Google accounts may sign in remotely (matched against the
        Tailscale identity). An empty allowlist denies every remote caller.
      </p>

      {banner && <BannerRow banner={banner} />}

      <div className="mt-4">
        {loading ? (
          <p className="text-sm text-slate-400">Loading authorized accounts…</p>
        ) : error ? (
          <ErrorRow text={error} onRetry={load} />
        ) : auth ? (
          <>
            {/* add email */}
            <form onSubmit={addEmail} className="flex flex-wrap items-end gap-3">
              <label className="flex min-w-[14rem] flex-1 flex-col gap-1.5">
                <span className="text-xs font-medium text-slate-400">
                  Add account email
                </span>
                <input
                  type="email"
                  value={newEmail}
                  onChange={(e) => setNewEmail(e.target.value)}
                  placeholder="person@gmail.com"
                  className={inputCls}
                />
              </label>
              <button
                type="submit"
                disabled={adding}
                className="rounded-md bg-accent/20 px-4 py-2 text-sm text-accent transition hover:bg-accent/30 disabled:cursor-not-allowed disabled:opacity-40"
              >
                {adding ? "Adding…" : "Add"}
              </button>
            </form>

            {/* email list */}
            <div className="mt-4">
              {auth.allowed_emails.length === 0 ? (
                <p className="rounded-lg border border-dashed border-edge px-4 py-6 text-center text-sm text-slate-500">
                  No accounts allowlisted — remote access is denied for everyone.
                </p>
              ) : (
                <ul className="space-y-2">
                  {auth.allowed_emails.map((email) => (
                    <li
                      key={email}
                      className="flex items-center gap-3 rounded-lg border border-edge bg-slate-900/40 px-3 py-2"
                    >
                      <span className="break-all text-sm text-slate-200">
                        {email}
                      </span>
                      <button
                        onClick={() => removeEmail(email)}
                        disabled={removing === email}
                        className="ml-auto rounded-md border border-rose-500/40 px-2.5 py-1 text-xs text-rose-400 transition hover:bg-rose-500/10 disabled:opacity-40"
                      >
                        {removing === email ? "Removing…" : "Remove"}
                      </button>
                    </li>
                  ))}
                </ul>
              )}
            </div>

            {/* gate toggles */}
            <div className="mt-5 space-y-2">
              <ToggleRow
                label="Allow unauthenticated loopback"
                hint="When on, requests from 127.0.0.1 stay unauthenticated (local-first default)."
                checked={auth.loopback_allowed}
                disabled={toggling === "loopback"}
                onChange={(v) => toggle("loopback", v)}
              />
              <ToggleRow
                label="Require auth for remote callers"
                hint="When on, anyone reaching RepoHub over the tailnet must be an allowlisted account."
                checked={auth.remote_required}
                disabled={toggling === "remote"}
                onChange={(v) => toggle("remote", v)}
              />
            </div>
          </>
        ) : null}
      </div>
    </section>
  );
}

// ===========================================================================
// 3) Active sessions
// ===========================================================================

function SessionsSection() {
  const [sessions, setSessions] = useState<SessionRow[] | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [banner, setBanner] = useState<Banner>(null);
  const [revoking, setRevoking] = useState<number | null>(null);

  const load = useCallback(() => {
    setLoading(true);
    setError(null);
    listSessions()
      .then(setSessions)
      .catch((e) => setError(e instanceof Error ? e.message : String(e)))
      .finally(() => setLoading(false));
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  const revoke = async (id: number) => {
    setRevoking(id);
    setBanner(null);
    try {
      await revokeSession(id);
      setSessions((list) => (list ? list.filter((s) => s.id !== id) : list));
      setBanner({ kind: "ok", text: "Session revoked." });
    } catch (err) {
      setBanner({ kind: "err", text: err instanceof Error ? err.message : String(err) });
    } finally {
      setRevoking(null);
    }
  };

  // Only show live sessions (revoked rows are filtered server-side too, but be
  // defensive in case the endpoint returns the full history).
  const active = (sessions ?? []).filter((s) => !s.revoked);

  return (
    <section className="rounded-xl border border-edge bg-panel p-6">
      <div className="flex items-center justify-between">
        <h2 className="text-lg font-semibold">Active sessions</h2>
        <button
          onClick={load}
          disabled={loading}
          className="rounded-md border border-edge px-3 py-1.5 text-sm text-slate-300 transition hover:bg-edge hover:text-slate-100 disabled:cursor-not-allowed disabled:opacity-40"
        >
          {loading ? "Loading…" : "Refresh"}
        </button>
      </div>
      <p className="mt-1 text-xs text-slate-500">
        Browser sessions created by remote sign-in. Revoke any session to sign
        that device out immediately.
      </p>

      {banner && <BannerRow banner={banner} />}

      <div className="mt-4">
        {loading ? (
          <p className="text-sm text-slate-400">Loading sessions…</p>
        ) : error ? (
          <ErrorRow text={error} onRetry={load} />
        ) : active.length === 0 ? (
          <p className="rounded-lg border border-dashed border-edge px-4 py-6 text-center text-sm text-slate-500">
            No active sessions.
          </p>
        ) : (
          <div className="overflow-x-auto rounded-lg border border-edge">
            <table className="w-full text-sm">
              <thead className="bg-slate-900/60 text-left text-xs uppercase tracking-wide text-slate-500">
                <tr>
                  <th className="px-3 py-2 font-medium">Account</th>
                  <th className="px-3 py-2 font-medium">IP</th>
                  <th className="px-3 py-2 font-medium">Created</th>
                  <th className="px-3 py-2 font-medium">Last seen</th>
                  <th className="px-3 py-2 font-medium">Expires</th>
                  <th className="px-3 py-2" />
                </tr>
              </thead>
              <tbody>
                {active.map((s) => (
                  <tr
                    key={s.id}
                    className="border-t border-edge align-top text-slate-300"
                  >
                    <td className="px-3 py-2">
                      <div className="break-all text-slate-200">{s.email}</div>
                      {s.ua && (
                        <div
                          className="mt-0.5 max-w-[18rem] truncate text-[11px] text-slate-500"
                          title={s.ua}
                        >
                          {s.ua}
                        </div>
                      )}
                    </td>
                    <td className="px-3 py-2 font-mono text-xs text-slate-400">
                      {s.ip ?? "—"}
                    </td>
                    <td className="px-3 py-2 text-xs text-slate-400">
                      {fmtTime(s.created_at)}
                    </td>
                    <td className="px-3 py-2 text-xs text-slate-400">
                      {fmtTime(s.last_seen_at)}
                    </td>
                    <td className="px-3 py-2 text-xs text-slate-400">
                      {fmtTime(s.expires_at)}
                    </td>
                    <td className="px-3 py-2 text-right">
                      <button
                        onClick={() => revoke(s.id)}
                        disabled={revoking === s.id}
                        className="rounded-md border border-rose-500/40 px-2.5 py-1 text-xs text-rose-400 transition hover:bg-rose-500/10 disabled:opacity-40"
                      >
                        {revoking === s.id ? "Revoking…" : "Revoke"}
                      </button>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </div>
    </section>
  );
}

// ===========================================================================
// Shared local primitives
// ===========================================================================

type Banner = { kind: "ok" | "err"; text: string } | null;

// Best-effort ISO8601 -> compact local string ("—" when absent/unparseable).
function fmtTime(iso: string | null): string {
  if (!iso) return "—";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  return d.toLocaleString();
}

// One green/red status line (matches GoogleCloud.tsx).
function StatusRow({
  label,
  ok,
  okText,
  badText,
}: {
  label: string;
  ok: boolean;
  okText: string;
  badText: string;
}) {
  return (
    <div className="flex items-center gap-2 rounded-lg border border-edge bg-slate-900/40 px-3 py-2">
      <span
        aria-hidden
        className={`h-2.5 w-2.5 shrink-0 rounded-full ${
          ok ? "bg-emerald-400" : "bg-rose-400"
        }`}
      />
      <span className="text-xs font-medium text-slate-400">{label}</span>
      <span
        className={`ml-auto truncate text-xs ${
          ok ? "text-emerald-300" : "text-rose-300"
        }`}
        title={ok ? okText : badText}
      >
        {ok ? okText : badText}
      </span>
    </div>
  );
}

// A labelled boolean toggle row.
function ToggleRow({
  label,
  hint,
  checked,
  disabled,
  onChange,
}: {
  label: string;
  hint: string;
  checked: boolean;
  disabled: boolean;
  onChange: (value: boolean) => void;
}) {
  return (
    <label className="flex items-start gap-3 rounded-lg border border-edge bg-slate-900/40 px-3 py-2.5">
      <input
        type="checkbox"
        checked={checked}
        disabled={disabled}
        onChange={(e) => onChange(e.target.checked)}
        className="mt-0.5 h-4 w-4 shrink-0 accent-accent disabled:opacity-40"
      />
      <span className="flex flex-col gap-0.5">
        <span className="text-sm text-slate-200">{label}</span>
        <span className="text-xs text-slate-500">{hint}</span>
      </span>
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
