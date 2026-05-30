import { useEffect, useMemo, useState } from "react";
import { listRepos, getPreferences } from "../lib/api";
import type { Repo, Preferences } from "../lib/types";

// Consistency.tsx — placeholder for the styling/consistency engine.
// Lists curated "config bundle" ideas that could later be applied across
// tracked repos to keep formatting, linting, CI and editor settings uniform.
// Preferences (default language / cloud) gently bias the suggested bundles.

interface ConfigBundle {
  id: string;
  title: string;
  description: string;
  files: string[];
  tags: string[];
}

const BASE_BUNDLES: ConfigBundle[] = [
  {
    id: "editorconfig",
    title: "EditorConfig baseline",
    description:
      "Drop a shared .editorconfig so indentation, charset and newline rules are identical in every repo and editor.",
    files: [".editorconfig"],
    tags: ["formatting", "universal"],
  },
  {
    id: "gitignore",
    title: "Curated .gitignore",
    description:
      "Language-aware ignore rules plus the RepoHub data-root guards, merged from per-language templates.",
    files: [".gitignore"],
    tags: ["hygiene", "universal"],
  },
  {
    id: "license-readme",
    title: "License + README scaffold",
    description:
      "Ensure a LICENSE and a README skeleton with a status table exist, matching the house style.",
    files: ["LICENSE", "README.md"],
    tags: ["docs", "universal"],
  },
  {
    id: "rust-fmt",
    title: "Rust: rustfmt + clippy",
    description:
      "Shared rustfmt.toml and a clippy lint profile so all Rust crates format and lint identically.",
    files: ["rustfmt.toml", "clippy.toml"],
    tags: ["lint", "Rust"],
  },
  {
    id: "prettier",
    title: "JS/TS: Prettier + ESLint",
    description:
      "Common Prettier config and an ESLint flat-config preset for TypeScript and React projects.",
    files: [".prettierrc", "eslint.config.js"],
    tags: ["lint", "TypeScript", "JavaScript"],
  },
  {
    id: "ci-gha",
    title: "GitHub Actions CI",
    description:
      "A reusable build/test workflow stamped into .github/workflows so every repo runs the same checks on push.",
    files: [".github/workflows/ci.yml"],
    tags: ["ci", "universal"],
  },
  {
    id: "gcp-deploy",
    title: "Google Cloud deploy stub",
    description:
      "A Cloud Run / Cloud Build skeleton and Dockerfile template for repos targeting Google Cloud.",
    files: ["cloudbuild.yaml", "Dockerfile"],
    tags: ["deploy", "Google Cloud"],
  },
];

function preferredTags(prefs: Preferences | null): Set<string> {
  const tags = new Set<string>();
  if (!prefs) return tags;
  for (const lang of (prefs.pref_languages ?? "").split(",")) {
    const t = lang.trim();
    if (t) tags.add(t);
  }
  const cloud = (prefs.pref_cloud ?? "").trim();
  if (cloud) tags.add(cloud);
  return tags;
}

export default function Consistency() {
  const [repos, setRepos] = useState<Repo[] | null>(null);
  const [prefs, setPrefs] = useState<Preferences | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setError(null);
    Promise.all([listRepos(), getPreferences().catch(() => null)])
      .then(([r, p]) => {
        if (cancelled) return;
        setRepos(r);
        setPrefs(p);
      })
      .catch((e) => {
        if (!cancelled) setError(e instanceof Error ? e.message : String(e));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const wanted = useMemo(() => preferredTags(prefs), [prefs]);

  const bundles = useMemo(() => {
    // Surface preference-matching bundles first; keep ordering otherwise stable.
    const score = (b: ConfigBundle) =>
      b.tags.some((t) => wanted.has(t)) ? 0 : 1;
    return [...BASE_BUNDLES].sort((a, b) => score(a) - score(b));
  }, [wanted]);

  const tracked = useMemo(
    () => (repos ?? []).filter((r) => r.tracked),
    [repos],
  );

  return (
    <div className="mx-auto max-w-5xl space-y-6">
      <header className="rounded-xl border border-edge bg-panel p-6">
        <h1 className="text-xl font-semibold">Consistency</h1>
        <p className="mt-2 text-sm text-slate-400">
          The styling / consistency engine will let you stamp shared config
          bundles across your tracked repos so formatting, linting, CI and docs
          stay uniform. Changes will always land on the{" "}
          <code className="rounded bg-edge px-1 py-0.5 text-xs text-slate-200">
            repohub-staging
          </code>{" "}
          branch for review — never auto-merged. Below are the bundles RepoHub
          will be able to apply.
        </p>
        {prefs && (wanted.size > 0) && (
          <p className="mt-3 text-xs text-slate-500">
            Suggestions biased toward your preferences:{" "}
            {[...wanted].map((t) => (
              <span
                key={t}
                className="mr-1 inline-block rounded-full bg-accent/15 px-2 py-0.5 text-accent"
              >
                {t}
              </span>
            ))}
          </p>
        )}
      </header>

      {/* Config bundle ideas */}
      <section className="space-y-3">
        <h2 className="text-sm font-medium uppercase tracking-wide text-slate-400">
          Config bundle ideas
        </h2>
        <div className="grid gap-4 sm:grid-cols-2">
          {bundles.map((b) => {
            const matches = b.tags.some((t) => wanted.has(t));
            return (
              <div
                key={b.id}
                className={`rounded-xl border bg-panel p-4 transition ${
                  matches
                    ? "border-accent/40"
                    : "border-edge"
                }`}
              >
                <div className="flex items-start justify-between gap-2">
                  <h3 className="font-medium text-slate-200">{b.title}</h3>
                  {matches && (
                    <span className="shrink-0 rounded-full bg-accent/20 px-2 py-0.5 text-[10px] uppercase tracking-wide text-accent">
                      Suggested
                    </span>
                  )}
                </div>
                <p className="mt-1.5 text-sm text-slate-400">{b.description}</p>
                <div className="mt-3 flex flex-wrap gap-1.5">
                  {b.files.map((f) => (
                    <code
                      key={f}
                      className="rounded bg-edge px-1.5 py-0.5 text-[11px] text-slate-300"
                    >
                      {f}
                    </code>
                  ))}
                </div>
                <div className="mt-2 flex flex-wrap gap-1.5">
                  {b.tags.map((t) => (
                    <span
                      key={t}
                      className="rounded-full border border-edge px-2 py-0.5 text-[10px] text-slate-500"
                    >
                      {t}
                    </span>
                  ))}
                </div>
                <button
                  disabled
                  title="The consistency engine is not wired up yet."
                  className="mt-3 cursor-not-allowed rounded-md border border-edge px-3 py-1.5 text-xs text-slate-500"
                >
                  Apply to tracked repos (coming later)
                </button>
              </div>
            );
          })}
        </div>
      </section>

      {/* Tracked repos these bundles would target */}
      <section className="rounded-xl border border-edge bg-panel p-6">
        <div className="flex items-center justify-between">
          <h2 className="text-sm font-medium uppercase tracking-wide text-slate-400">
            Target repos
          </h2>
          {repos && (
            <span className="text-xs text-slate-500">
              {tracked.length} tracked / {repos.length} total
            </span>
          )}
        </div>

        {loading ? (
          <p className="mt-4 text-sm text-slate-400">Loading repos…</p>
        ) : error ? (
          <div className="mt-4 rounded-lg border border-rose-500/40 bg-rose-500/10 p-3 text-sm text-rose-300">
            Failed to load repos: {error}
          </div>
        ) : !repos || repos.length === 0 ? (
          <p className="mt-4 text-sm text-slate-400">
            No repos yet. Refresh from GitHub on the Dashboard to get started.
          </p>
        ) : tracked.length === 0 ? (
          <p className="mt-4 text-sm text-slate-400">
            No tracked repos yet. Track and clone repos on the Dashboard to make
            them eligible for consistency bundles.
          </p>
        ) : (
          <ul className="mt-4 divide-y divide-edge">
            {tracked.map((r) => (
              <li
                key={r.id}
                className="flex items-center justify-between gap-3 py-2.5"
              >
                <div className="min-w-0">
                  <div className="truncate text-sm text-slate-200">
                    {r.full_name}
                  </div>
                  {r.description && (
                    <div className="truncate text-xs text-slate-500">
                      {r.description}
                    </div>
                  )}
                </div>
                <div className="flex shrink-0 items-center gap-2 text-[11px]">
                  {r.language && (
                    <span className="rounded-full bg-edge px-2 py-0.5 text-slate-300">
                      {r.language}
                    </span>
                  )}
                  <span
                    className={`rounded-full px-2 py-0.5 ${
                      r.dirty
                        ? "bg-amber-500/15 text-amber-300"
                        : "bg-emerald-500/15 text-emerald-300"
                    }`}
                  >
                    {r.dirty ? "dirty" : "clean"}
                  </span>
                </div>
              </li>
            ))}
          </ul>
        )}
      </section>
    </div>
  );
}
