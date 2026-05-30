import { useEffect, useState } from "react";

type Health = { ok: boolean; service: string; version: string };

const TABS = [
  "Dashboard",
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
        <Placeholder tab={tab} />
      </main>
    </div>
  );
}

function Placeholder({ tab }: { tab: Tab }) {
  return (
    <div className="mx-auto max-w-2xl rounded-xl border border-edge bg-panel p-8">
      <h1 className="text-xl font-semibold">{tab}</h1>
      <p className="mt-2 text-sm text-slate-400">
        Scaffolded. This view will be built in a later phase.
      </p>
    </div>
  );
}
