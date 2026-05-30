import { useCallback, useEffect, useState } from "react";
import { getGcloud, putGcloud } from "../../lib/configApi";
import type { GcloudStatus } from "../../lib/configTypes";

// GoogleCloud.tsx (P16) — the single Google Cloud config block.
//
// Every GCP-backed feature (Secret Manager secrets, Vertex AI embeddings,
// Vertex AI Vector Search, Vertex AI Gen AI Evaluation) keys off ONE block:
// project + region, plus the locally-detected SDK install + ADC status. We
// surface that status with green/red indicators and, when gcloud is not
// installed or ADC is missing, walk the user through the setup steps.
//
// GET  /api/gcloud  -> { installed, adc, project, region }
// PUT  /api/gcloud  -> { project?, region? } (stored as global settings rows)

type Banner = { kind: "ok" | "err"; text: string } | null;

const inputCls =
  "w-full rounded-md border border-edge bg-slate-900/60 px-3 py-2 text-sm text-slate-200 outline-none focus:border-accent";

// `ready` mirrors gcloud::GcloudStatus::ready() — installed && adc.
function isReady(s: GcloudStatus): boolean {
  return s.installed && s.adc;
}

export default function GoogleCloud() {
  const [status, setStatus] = useState<GcloudStatus | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [banner, setBanner] = useState<Banner>(null);

  // Controlled form fields for the project/region config block.
  const [project, setProject] = useState("");
  const [region, setRegion] = useState("");
  const [saving, setSaving] = useState(false);

  const load = useCallback(() => {
    setLoading(true);
    setError(null);
    getGcloud()
      .then((s) => {
        setStatus(s);
        setProject(s.project ?? "");
        setRegion(s.region ?? "");
      })
      .catch((e) => setError(e instanceof Error ? e.message : String(e)))
      .finally(() => setLoading(false));
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  const save = async () => {
    setSaving(true);
    setBanner(null);
    try {
      // Empty inputs become null so the backend can clear an unset block.
      const saved = await putGcloud({
        project: project.trim() ? project.trim() : null,
        region: region.trim() ? region.trim() : null,
      });
      setStatus(saved);
      setProject(saved.project ?? "");
      setRegion(saved.region ?? "");
      setBanner({ kind: "ok", text: "Google Cloud config saved." });
    } catch (e) {
      setBanner({ kind: "err", text: e instanceof Error ? e.message : String(e) });
    } finally {
      setSaving(false);
    }
  };

  const ready = status ? isReady(status) : false;

  return (
    <section className="rounded-xl border border-edge bg-panel p-6">
      <div className="flex items-center justify-between">
        <h2 className="text-lg font-semibold">Google Cloud</h2>
        <button
          onClick={load}
          disabled={loading}
          className="rounded-md border border-edge px-3 py-1.5 text-sm text-slate-300 transition hover:bg-edge hover:text-slate-100 disabled:cursor-not-allowed disabled:opacity-40"
        >
          {loading ? "Checking…" : "Refresh"}
        </button>
      </div>
      <p className="mt-1 text-xs text-slate-500">
        The single Google Cloud config block — project, region, and locally
        detected SDK + credentials. Every GCP-backed feature depends on this:{" "}
        <span className="text-slate-400">Secret Manager</span> secrets,{" "}
        <span className="text-slate-400">Vertex AI</span> embeddings, Vector
        Search, and Gen AI Evaluation all read it.
      </p>

      {banner && <BannerRow banner={banner} />}

      <div className="mt-4">
        {loading ? (
          <p className="text-sm text-slate-400">Checking Google Cloud status…</p>
        ) : error ? (
          <ErrorRow text={error} onRetry={load} />
        ) : status ? (
          <>
            {/* ---- status indicators ---- */}
            <div className="grid gap-2 sm:grid-cols-2">
              <StatusRow
                label="Google Cloud SDK"
                ok={status.installed}
                okText="gcloud installed"
                badText="gcloud not found on PATH"
              />
              <StatusRow
                label="Credentials (ADC)"
                ok={status.adc}
                okText="application-default credentials present"
                badText="no application-default credentials"
              />
              <StatusRow
                label="Project"
                ok={!!status.project}
                okText={status.project ?? ""}
                badText="not set"
              />
              <StatusRow
                label="Region"
                ok={!!status.region}
                okText={status.region ?? ""}
                badText="not set"
              />
            </div>

            <div
              className={`mt-4 rounded-md px-3 py-2 text-sm ${
                ready
                  ? "bg-emerald-500/10 text-emerald-300"
                  : "bg-amber-500/10 text-amber-300"
              }`}
            >
              {ready
                ? "Google Cloud is ready. GCP-backed features can authenticate (live calls are still being wired)."
                : "Google Cloud is not configured. GCP-backed features (secrets, embeddings, vector search, evals) will degrade gracefully until the steps below are complete."}
            </div>

            {/* ---- project / region form ---- */}
            <div className="mt-5 rounded-lg border border-edge bg-slate-900/40 p-4">
              <h3 className="text-sm font-medium text-slate-200">
                Project &amp; region
              </h3>
              <p className="mt-1 text-xs text-slate-500">
                Stored as global settings (<code className="text-slate-400">gcloud_project</code>,{" "}
                <code className="text-slate-400">gcloud_region</code>) and shared
                by every GCP-backed feature.
              </p>
              <div className="mt-4 grid gap-3 sm:grid-cols-2">
                <Field
                  label="Project ID"
                  hint="e.g. my-project-1234"
                >
                  <input
                    value={project}
                    onChange={(e) => setProject(e.target.value)}
                    placeholder="my-project-1234"
                    className={inputCls}
                  />
                </Field>
                <Field label="Region" hint="e.g. us-central1">
                  <input
                    value={region}
                    onChange={(e) => setRegion(e.target.value)}
                    placeholder="us-central1"
                    className={inputCls}
                  />
                </Field>
              </div>
              <div className="mt-4 flex justify-end">
                <button
                  onClick={save}
                  disabled={saving}
                  className="rounded-md bg-accent/20 px-4 py-2 text-sm text-accent transition hover:bg-accent/30 disabled:cursor-not-allowed disabled:opacity-40"
                >
                  {saving ? "Saving…" : "Save config"}
                </button>
              </div>
            </div>

            {/* ---- setup instructions (only when not ready) ---- */}
            {!ready && <SetupInstructions status={status} />}
          </>
        ) : null}
      </div>
    </section>
  );
}

// One green/red status line.
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

// Step-by-step setup, shown when gcloud is not installed or ADC is missing.
function SetupInstructions({ status }: { status: GcloudStatus }) {
  const steps: { title: string; cmd?: string; note: string; done: boolean }[] = [
    {
      title: "Install the Google Cloud SDK",
      note: status.installed
        ? "Detected on PATH."
        : "Install gcloud, then make sure it is on your PATH. See cloud.google.com/sdk/docs/install.",
      done: status.installed,
    },
    {
      title: "Log in to your Google account",
      cmd: "gcloud auth login",
      note: "Opens a browser to authenticate the gcloud CLI itself.",
      done: status.installed,
    },
    {
      title: "Create application default credentials",
      cmd: "gcloud auth application-default login",
      note: status.adc
        ? "ADC file detected — agents and Vertex AI client libraries can authenticate."
        : "Required for client libraries (embeddings, vector search, evals, Secret Manager) to authenticate.",
      done: status.adc,
    },
    {
      title: "Set the active project",
      cmd: "gcloud config set project <project-id>",
      note: status.project
        ? `Project "${status.project}" is configured above.`
        : "Use the same project id you save in the form above.",
      done: !!status.project,
    },
    {
      title: "Enable the required APIs",
      cmd: "gcloud services enable secretmanager.googleapis.com aiplatform.googleapis.com",
      note: "Secret Manager backs secrets; Vertex AI (aiplatform) backs embeddings, vector search, and evals.",
      done: false,
    },
  ];

  return (
    <div className="mt-5 rounded-lg border border-amber-500/30 bg-amber-500/5 p-4">
      <h3 className="text-sm font-medium text-amber-200">Set up Google Cloud</h3>
      <p className="mt-1 text-xs text-slate-400">
        gcloud is not installed in this environment yet, so GCP-backed features
        return a clear "not configured" message instead of failing. Complete
        these steps on the machine running RepoHub, then press Refresh.
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

function Field({
  label,
  hint,
  children,
}: {
  label: string;
  hint?: string;
  children: React.ReactNode;
}) {
  return (
    <label className="flex min-w-[8rem] flex-1 flex-col gap-1.5">
      <span className="text-xs font-medium text-slate-400">
        {label}
        {hint && <span className="ml-2 font-normal text-slate-600">{hint}</span>}
      </span>
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
