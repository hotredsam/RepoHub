import { useCallback, useEffect, useMemo, useState } from "react";
import {
  getKnowledgeConfig,
  putKnowledgeConfig,
  reindexKnowledge,
  queryKnowledge,
  getGcloud,
} from "../../lib/configApi";
import type {
  KnowledgeConfig,
  EmbedProvider,
  GcloudStatus,
  VectorMatch,
} from "../../lib/configTypes";

// One section of the P16 layered Settings hub: knowledge / RAG configuration.
//
// Layered config — `scope = "global"` edits the live ~/.claude defaults; a
// `scope = "repo"` (with repoId) override inherits global when unset. Toggle
// memory + RAG, pick the embedding provider (Vertex / local), set the context
// budget, reindex the vector store, and run a retrieval test. When Vertex is
// the chosen provider but Google Cloud isn't ready, we surface a clear
// "Configure Google Cloud" banner instead of letting calls fail opaquely.

interface KnowledgeProps {
  scope: string;
  repoId?: number | null;
}

type Banner = { kind: "ok" | "err"; text: string } | null;

// The form mirrors the backend's effective KnowledgeConfig
// (memory/rag/provider/budget). The values are read from `cfg.effective`.
interface KnowledgeForm {
  provider: EmbedProvider;
  memoryEnabled: boolean;
  ragEnabled: boolean;
  contextBudget: number;
}

const DEFAULT_BUDGET = 8000;

function toForm(cfg: KnowledgeConfig): KnowledgeForm {
  // The real values live under `effective` (the inheritance-resolved config).
  const eff = cfg.effective;
  return {
    provider: eff?.embedding_provider === "local" ? "local" : "vertex",
    memoryEnabled: eff?.memory_enabled ?? true,
    ragEnabled: eff?.rag_enabled ?? true,
    contextBudget: eff?.context_budget_tokens ?? DEFAULT_BUDGET,
  };
}

export default function Knowledge({ scope, repoId }: KnowledgeProps) {
  const repo_id = repoId ?? null;
  const isRepo = scope === "repo";

  // ---- config ----
  const [form, setForm] = useState<KnowledgeForm | null>(null);
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [configBanner, setConfigBanner] = useState<Banner>(null);

  // ---- gcloud status ----
  const [gcloud, setGcloud] = useState<GcloudStatus | null>(null);

  // ---- reindex ----
  const [reindexing, setReindexing] = useState(false);
  const [reindexBanner, setReindexBanner] = useState<Banner>(null);

  // ---- retrieval test ----
  const [query, setQuery] = useState("");
  const [querying, setQuerying] = useState(false);
  const [matches, setMatches] = useState<VectorMatch[] | null>(null);
  const [queryBanner, setQueryBanner] = useState<Banner>(null);

  // ---- loaders ----
  const loadConfig = useCallback(() => {
    setLoading(true);
    setLoadError(null);
    getKnowledgeConfig(scope, repo_id ?? undefined)
      .then((cfg) => setForm(toForm(cfg)))
      .catch((e) => setLoadError(e instanceof Error ? e.message : String(e)))
      .finally(() => setLoading(false));
  }, [scope, repo_id]);

  const loadGcloud = useCallback(() => {
    getGcloud()
      .then(setGcloud)
      .catch(() => setGcloud(null));
  }, []);

  useEffect(() => {
    loadConfig();
    loadGcloud();
  }, [loadConfig, loadGcloud]);

  // ---- derived ----
  const vertexReady = !!gcloud && gcloud.installed && gcloud.adc;
  const needsGcloud = form?.provider === "vertex" && !vertexReady;

  const setField = <K extends keyof KnowledgeForm>(
    key: K,
    value: KnowledgeForm[K],
  ) => setForm((f) => (f ? { ...f, [key]: value } : f));

  // ---- handlers ----
  const save = async () => {
    if (!form) return;
    setSaving(true);
    setConfigBanner(null);
    try {
      // Send the backend's patch keys verbatim (knowledge.rs::PutConfigBody).
      const saved = await putKnowledgeConfig({
        scope,
        repo_id,
        embedding_provider: form.provider,
        memory_enabled: form.memoryEnabled,
        rag_enabled: form.ragEnabled,
        context_budget_tokens: form.contextBudget,
      });
      setForm(toForm(saved));
      setConfigBanner({ kind: "ok", text: "Knowledge configuration saved." });
    } catch (e) {
      setConfigBanner({
        kind: "err",
        text: e instanceof Error ? e.message : String(e),
      });
    } finally {
      setSaving(false);
    }
  };

  const runReindex = async () => {
    setReindexing(true);
    setReindexBanner(null);
    try {
      // The backend reindexes a list of repo ids; scope is implied by which
      // repo's effective config it resolves. Empty list when no repo selected.
      const res = await reindexKnowledge({
        repo_ids: repo_id != null ? [repo_id] : [],
      });
      const summary =
        res.results.length === 0
          ? "Nothing to reindex (select a repository)."
          : res.results
              .map((r) => `repo ${r.repo_id}: ${r.status}`)
              .join(", ");
      setReindexBanner({
        kind: res.results.some((r) => r.status === "error") ? "err" : "ok",
        text: res.gcloud_ready
          ? summary
          : `${summary} — configure Google Cloud for live indexing.`,
      });
    } catch (e) {
      setReindexBanner({
        kind: "err",
        text: e instanceof Error ? e.message : String(e),
      });
    } finally {
      setReindexing(false);
    }
  };

  const runQuery = async () => {
    const q = query.trim();
    if (!q) {
      setQueryBanner({ kind: "err", text: "Enter a query first." });
      return;
    }
    setQuerying(true);
    setQueryBanner(null);
    setMatches(null);
    try {
      const res = await queryKnowledge({ scope, repo_id, q });
      setMatches(res.matches);
      // The query degrades gracefully (status disabled/not_configured) rather
      // than erroring; surface the backend's message when it is not a clean ok.
      setQueryBanner({
        kind: res.status === "ok" ? "ok" : "err",
        text:
          res.status === "ok"
            ? `${res.matches.length} match${
                res.matches.length === 1 ? "" : "es"
              }.`
            : res.message,
      });
    } catch (e) {
      setQueryBanner({
        kind: "err",
        text: e instanceof Error ? e.message : String(e),
      });
    } finally {
      setQuerying(false);
    }
  };

  const gcloudHint = useMemo(() => {
    if (!gcloud) return "Google Cloud status is unavailable.";
    if (!gcloud.installed)
      return "gcloud is not installed. Install the Google Cloud CLI to use Vertex AI.";
    if (!gcloud.adc)
      return "No application-default credentials. Run: gcloud auth application-default login";
    return "Google Cloud is ready.";
  }, [gcloud]);

  return (
    <section className="rounded-xl border border-edge bg-panel p-6">
      <div className="flex items-center justify-between gap-3">
        <div>
          <h2 className="text-lg font-semibold">Knowledge &amp; retrieval</h2>
          <p className="mt-1 text-xs text-slate-500">
            Memory, RAG, embeddings, and the context budget.{" "}
            {isRepo ? (
              <>
                This is a{" "}
                <span className="text-accent">per-repo override</span> —
                unset values inherit the global defaults.
              </>
            ) : (
              <>These are the global defaults all repos inherit.</>
            )}
          </p>
        </div>
        <button
          onClick={save}
          disabled={saving || loading || !form}
          className="shrink-0 rounded-md bg-accent/20 px-3 py-1.5 text-sm text-accent transition hover:bg-accent/30 disabled:cursor-not-allowed disabled:opacity-40"
        >
          {saving ? "Saving…" : "Save configuration"}
        </button>
      </div>

      {/* Configure-Google-Cloud banner (only when Vertex is selected but not ready) */}
      {needsGcloud && (
        <div className="mt-4 rounded-lg border border-amber-500/40 bg-amber-500/10 p-3">
          <p className="text-sm font-medium text-amber-300">
            Configure Google Cloud
          </p>
          <p className="mt-1 text-xs text-amber-200/80">
            Vertex AI embeddings need the Google Cloud CLI installed and signed
            in. {gcloudHint}
          </p>
          <p className="mt-1.5 font-mono text-[11px] text-amber-200/70">
            gcloud auth application-default login
          </p>
          <p className="mt-1.5 text-xs text-amber-200/70">
            You can switch the provider to{" "}
            <button
              type="button"
              onClick={() => setField("provider", "local")}
              className="underline underline-offset-2 hover:text-amber-100"
            >
              local
            </button>{" "}
            to work without Google Cloud, or set the project/region in the
            Google Cloud settings.
          </p>
        </div>
      )}

      {loading ? (
        <p className="mt-4 text-sm text-slate-400">Loading configuration…</p>
      ) : loadError ? (
        <ErrorRow text={loadError} onRetry={loadConfig} />
      ) : form ? (
        <>
          <div className="mt-5 space-y-4">
            {/* toggles */}
            <div className="grid gap-3 sm:grid-cols-2">
              <Toggle
                label="Memory"
                hint="Let Claude read and write persistent memory for this scope."
                checked={form.memoryEnabled}
                onChange={(v) => setField("memoryEnabled", v)}
              />
              <Toggle
                label="RAG (retrieval)"
                hint="Augment prompts with vector-store search results."
                checked={form.ragEnabled}
                onChange={(v) => setField("ragEnabled", v)}
              />
            </div>

            {/* provider + budget */}
            <div className="grid gap-4 sm:grid-cols-2">
              <Field label="Embedding provider">
                <select
                  value={form.provider}
                  onChange={(e) =>
                    setField("provider", e.target.value as EmbedProvider)
                  }
                  className={inputCls}
                >
                  <option value="vertex">Vertex AI</option>
                  <option value="local">Local</option>
                </select>
              </Field>
              <Field label="Context budget (tokens)">
                <input
                  type="number"
                  min={0}
                  step={500}
                  value={form.contextBudget}
                  onChange={(e) =>
                    setField(
                      "contextBudget",
                      Math.max(0, Number(e.target.value) || 0),
                    )
                  }
                  className={inputCls}
                />
              </Field>
            </div>

          </div>

          {configBanner && <BannerRow banner={configBanner} />}

          {/* reindex */}
          <div className="mt-6 border-t border-edge pt-5">
            <div className="flex flex-wrap items-center justify-between gap-3">
              <div>
                <h3 className="text-sm font-medium text-slate-200">
                  Reindex vector store
                </h3>
                <p className="mt-0.5 text-xs text-slate-500">
                  (Re)embed sources for this scope and upsert them into the
                  vector store.
                </p>
              </div>
              <button
                onClick={runReindex}
                disabled={reindexing || !form.ragEnabled}
                title={
                  form.ragEnabled
                    ? undefined
                    : "Enable RAG to reindex the vector store."
                }
                className="shrink-0 rounded-md border border-edge px-3 py-1.5 text-sm text-slate-300 transition hover:bg-edge disabled:cursor-not-allowed disabled:opacity-40"
              >
                {reindexing ? "Reindexing…" : "Reindex"}
              </button>
            </div>
            {reindexBanner && <BannerRow banner={reindexBanner} />}
          </div>

          {/* retrieval test */}
          <div className="mt-6 border-t border-edge pt-5">
            <h3 className="text-sm font-medium text-slate-200">
              Retrieval test
            </h3>
            <p className="mt-0.5 text-xs text-slate-500">
              Run a nearest-neighbour search to preview what RAG would retrieve.
            </p>
            <div className="mt-3 flex flex-wrap items-end gap-3">
              <label className="flex min-w-[16rem] flex-1 flex-col gap-1.5">
                <span className="text-xs font-medium text-slate-400">
                  Query
                </span>
                <input
                  value={query}
                  onChange={(e) => setQuery(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") runQuery();
                  }}
                  placeholder="How is auth handled?"
                  className={inputCls}
                />
              </label>
              <button
                onClick={runQuery}
                disabled={querying || !query.trim()}
                className="rounded-md bg-accent/20 px-4 py-2 text-sm text-accent transition hover:bg-accent/30 disabled:cursor-not-allowed disabled:opacity-40"
              >
                {querying ? "Searching…" : "Search"}
              </button>
            </div>

            {queryBanner && <BannerRow banner={queryBanner} />}

            {matches && matches.length > 0 && (
              <ul className="mt-3 space-y-2">
                {matches.map((m, i) => (
                  <li
                    key={`${m.id}-${i}`}
                    className="rounded-lg border border-edge bg-slate-900/40 p-3"
                  >
                    <div className="flex items-center gap-3">
                      <span className="font-mono text-xs text-slate-200">
                        {m.id}
                      </span>
                      <span className="ml-auto rounded bg-edge px-1.5 py-0.5 text-[11px] tabular-nums text-slate-400">
                        score {m.score.toFixed(4)}
                      </span>
                    </div>
                    {m.metadata != null && (
                      <pre className="mt-2 max-h-40 overflow-auto rounded bg-slate-950/60 px-2 py-1.5 text-[11px] leading-relaxed text-slate-400">
                        <code>{JSON.stringify(m.metadata, null, 2)}</code>
                      </pre>
                    )}
                  </li>
                ))}
              </ul>
            )}
            {matches && matches.length === 0 && (
              <p className="mt-3 rounded-lg border border-dashed border-edge px-4 py-6 text-center text-sm text-slate-500">
                No matches.
              </p>
            )}
          </div>
        </>
      ) : null}
    </section>
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

function Toggle({
  label,
  hint,
  checked,
  onChange,
}: {
  label: string;
  hint: string;
  checked: boolean;
  onChange: (v: boolean) => void;
}) {
  return (
    <label className="flex cursor-pointer items-start gap-3 rounded-lg border border-edge bg-slate-900/40 p-3">
      <input
        type="checkbox"
        checked={checked}
        onChange={(e) => onChange(e.target.checked)}
        className="mt-0.5 h-4 w-4 accent-accent"
      />
      <span className="flex flex-col">
        <span className="text-sm font-medium text-slate-200">{label}</span>
        <span className="mt-0.5 text-xs text-slate-500">{hint}</span>
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
