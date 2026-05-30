import { useEffect, useState } from "react";
import Dashboard from "./views/Dashboard";
import Chat from "./views/Chat";
import RepoDetail from "./views/RepoDetail";
import Connections from "./views/Connections";
import Consistency from "./views/Consistency";
import Merge from "./views/Merge";
import Prompts from "./views/Prompts";
import Tickets from "./views/Tickets";
import Settings from "./views/Settings";
import RemoteAccess from "./views/RemoteAccess";
import Codex from "./views/Codex";
import Audit from "./views/Audit";
import Permissions from "./views/Permissions";
import Login from "./views/Login";
import { getAuthStatus, logout } from "./lib/api";
import type { AuthStatus } from "./lib/types";

type Health = { ok: boolean; service: string; version: string };

const TABS = [
  "Dashboard",
  "Chat",
  "Repo",
  "Connections",
  "Consistency",
  "Merge",
  "Prompts",
  "Tickets",
  "Remote",
  "Codex",
  "Audit",
  "Permissions",
  "Settings",
] as const;
type Tab = (typeof TABS)[number];

export default function App() {
  const [tab, setTab] = useState<Tab>("Dashboard");
  const [health, setHealth] = useState<Health | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [auth, setAuth] = useState<AuthStatus | null>(null);

  useEffect(() => {
    fetch("/api/health")
      .then((r) => r.json())
      .then(setHealth)
      .catch((e) => setErr(String(e)));
    // Auth posture decides whether we render the Login screen instead of the
    // tab shell. Failure here is non-fatal (e.g. backend offline) — we fall
    // back to the local-first tab shell.
    getAuthStatus()
      .then(setAuth)
      .catch(() => setAuth(null));
  }, []);

  // Remote + OAuth-configured + not signed in: show the full-screen Login.
  if (auth && auth.configured && auth.remote && !auth.authenticated) {
    return <Login />;
  }

  return (
    <div className="flex h-full flex-col">
      <header className="flex items-center gap-4 border-b border-edge bg-panel px-5 py-3">
        <span className="text-lg font-semibold tracking-tight">
          Repo<span className="text-accent">Hub</span>
        </span>
        <nav className="flex flex-wrap gap-1">
          {TABS.map((t) => (
            <button
              key={t}
              onClick={() => setTab(t)}
              className={`rounded-md px-3 py-1.5 text-sm transition ${
                tab === t
                  ? "bg-accent/20 text-accent"
                  : "text-slate-400 hover:bg-edge hover:text-slate-200"
              }`}
            >
              {t}
            </button>
          ))}
        </nav>
        <div className="ml-auto flex items-center gap-3 text-xs">
          {auth?.authenticated && auth.email ? (
            <AccountMenu
              email={auth.email}
              onSignedOut={() => {
                getAuthStatus()
                  .then(setAuth)
                  .catch(() => setAuth(null));
              }}
            />
          ) : null}
          <div className="flex items-center gap-2">
            <span
              className={`h-2 w-2 rounded-full ${
                health?.ok
                  ? "bg-emerald-400"
                  : err
                    ? "bg-rose-500"
                    : "bg-amber-400"
              }`}
            />
            <span className="text-slate-500">
              {health?.ok
                ? `backend v${health.version}`
                : err
                  ? "backend offline"
                  : "connecting…"}
            </span>
          </div>
        </div>
      </header>

      <main className="flex-1 overflow-auto p-6">
        <View tab={tab} />
      </main>
    </div>
  );
}

function AccountMenu({
  email,
  onSignedOut,
}: {
  email: string;
  onSignedOut: () => void;
}) {
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);

  const signOut = async () => {
    setBusy(true);
    try {
      await logout();
    } catch {
      // ignore — clearing the cookie locally is best-effort
    } finally {
      setBusy(false);
      setOpen(false);
      onSignedOut();
    }
  };

  return (
    <div className="relative">
      <button
        onClick={() => setOpen((o) => !o)}
        className="flex items-center gap-1.5 rounded-md px-2 py-1 text-slate-300 transition hover:bg-edge"
      >
        <span className="h-2 w-2 rounded-full bg-accent" />
        <span className="max-w-[16rem] truncate">{email}</span>
        <span className="text-slate-500">▾</span>
      </button>
      {open ? (
        <div className="absolute right-0 z-10 mt-1 w-44 rounded-md border border-edge bg-panel py-1 shadow-xl">
          <button
            onClick={signOut}
            disabled={busy}
            className="block w-full px-3 py-1.5 text-left text-slate-300 transition hover:bg-edge disabled:opacity-50"
          >
            {busy ? "Signing out…" : "Sign out"}
          </button>
        </div>
      ) : null}
    </div>
  );
}

function View({ tab }: { tab: Tab }) {
  switch (tab) {
    case "Dashboard":
      return <Dashboard />;
    case "Chat":
      return <Chat />;
    case "Repo":
      return <RepoDetail />;
    case "Connections":
      return <Connections />;
    case "Consistency":
      return <Consistency />;
    case "Merge":
      return <Merge />;
    case "Prompts":
      return <Prompts />;
    case "Tickets":
      return <Tickets />;
    case "Remote":
      return <RemoteAccess />;
    case "Codex":
      return <Codex />;
    case "Audit":
      return <Audit />;
    case "Permissions":
      return <Permissions />;
    case "Settings":
      return <Settings />;
  }
}
