import { useEffect, useState } from "react";
import Dashboard from "./views/Dashboard";
import Chat from "./views/Chat";
import RepoDetail from "./views/RepoDetail";
import Connections from "./views/Connections";
import Consistency from "./views/Consistency";
import Prompts from "./views/Prompts";
import Settings from "./views/Settings";

type Health = { ok: boolean; service: string; version: string };

const TABS = [
  "Dashboard",
  "Chat",
  "Repo",
  "Connections",
  "Consistency",
  "Prompts",
  "Settings",
] as const;
type Tab = (typeof TABS)[number];

export default function App() {
  const [tab, setTab] = useState<Tab>("Dashboard");
  const [health, setHealth] = useState<Health | null>(null);
  const [err, setErr] = useState<string | null>(null);

  useEffect(() => {
    fetch("/api/health")
      .then((r) => r.json())
      .then(setHealth)
      .catch((e) => setErr(String(e)));
  }, []);

  return (
    <div className="flex h-full flex-col">
      <header className="flex items-center gap-4 border-b border-edge bg-panel px-5 py-3">
        <span className="text-lg font-semibold tracking-tight">
          Repo<span className="text-accent">Hub</span>
        </span>
        <nav className="flex gap-1">
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
        <div className="ml-auto flex items-center gap-2 text-xs">
          <span
            className={`h-2 w-2 rounded-full ${
              health?.ok ? "bg-emerald-400" : err ? "bg-rose-500" : "bg-amber-400"
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
      </header>

      <main className="flex-1 overflow-auto p-6">
        <View tab={tab} />
      </main>
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
    case "Prompts":
      return <Prompts />;
    case "Settings":
      return <Settings />;
  }
}
