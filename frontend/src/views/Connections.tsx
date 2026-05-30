import { useCallback, useEffect, useMemo, useState } from "react";
import ReactFlow, {
  Background,
  Controls,
  addEdge,
  useNodesState,
  useEdgesState,
} from "reactflow";
import type { Connection, Edge, Node } from "reactflow";
import "reactflow/dist/style.css";

// Connections.tsx (P7)
// Cross-repo integration graph. Repos become nodes laid out on a simple grid.
// Drag from a SOURCE node handle to a TARGET node to declare an integration:
// a modal collects a free-text instruction and POSTs it; Claude scaffolds the
// wiring on the repohub-staging branch and returns a diff stat + response
// excerpt, which we surface as the new edge's label.

// ---- API contract (kept local to this unit; mirrors backend connections.rs) ----

interface GraphRepo {
  id: number;
  name: string;
  full_name: string;
  language: string | null;
}

// Shape used purely for building a react-flow edge. The backend persists no
// connections, so these are derived locally from the integrate request.
interface GraphConnection {
  source_id: number;
  target_id: number;
  instruction: string;
}

interface IntegrateBody {
  source_id: number;
  target_id: number;
  instruction: string;
}

// Matches backend connections.rs `IntegrateResult`.
interface IntegrateResult {
  ok: boolean;
  branch: string;
  committed: boolean;
  diff_stat: string;
  response_excerpt: string;
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(path, {
    headers: { "Content-Type": "application/json", ...(init?.headers ?? {}) },
    ...init,
  });
  if (!res.ok) {
    let msg = `${res.status} ${res.statusText}`;
    try {
      const body = await res.json();
      if (body && typeof body.error === "string") msg = body.error;
    } catch {
      // ignore non-JSON error bodies
    }
    throw new Error(`${path} failed: ${msg}`);
  }
  if (res.status === 204) return undefined as T;
  const text = await res.text();
  return (text ? JSON.parse(text) : undefined) as T;
}

// Backend returns a bare JSON array of repo nodes (no persisted connections).
const getGraph = () => request<GraphRepo[]>("/api/connections/graph");

const integrate = (body: IntegrateBody) =>
  request<IntegrateResult>("/api/connections/integrate", {
    method: "POST",
    body: JSON.stringify(body),
  });

// ---- layout helpers ----

const COLS = 3;
const COL_GAP = 240;
const ROW_GAP = 130;

function gridPosition(index: number): { x: number; y: number } {
  const col = index % COLS;
  const row = Math.floor(index / COLS);
  return { x: col * COL_GAP, y: row * ROW_GAP };
}

function repoNode(repo: GraphRepo, index: number): Node {
  return {
    id: String(repo.id),
    position: gridPosition(index),
    data: {
      label: (
        <div className="text-left">
          <div className="truncate text-sm font-medium text-slate-100">
            {repo.name}
          </div>
          <div className="mt-0.5 text-[11px] text-slate-400">
            {repo.language ?? "—"}
          </div>
        </div>
      ),
    },
    style: {
      width: 180,
      borderRadius: 10,
      border: "1px solid #2a3344",
      background: "#0f1626",
      color: "#e2e8f0",
      padding: "10px 12px",
    },
  };
}

function connectionEdge(c: GraphConnection): Edge {
  return {
    id: `e-${c.source_id}-${c.target_id}`,
    source: String(c.source_id),
    target: String(c.target_id),
    label: c.instruction,
    animated: true,
    labelStyle: { fill: "#cbd5e1", fontSize: 11 },
    labelBgStyle: { fill: "#0f1626" },
    labelBgPadding: [6, 3] as [number, number],
    labelBgBorderRadius: 4,
    style: { stroke: "#6366f1" },
  };
}

// ---- component ----

interface PendingConnect {
  source: GraphRepo;
  target: GraphRepo;
}

export default function Connections() {
  const [repos, setRepos] = useState<GraphRepo[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const [nodes, setNodes, onNodesChange] = useNodesState([]);
  const [edges, setEdges, onEdgesChange] = useEdgesState([]);

  // Modal / integration state.
  const [pending, setPending] = useState<PendingConnect | null>(null);
  const [instruction, setInstruction] = useState("");
  const [integrating, setIntegrating] = useState(false);
  const [modalError, setModalError] = useState<string | null>(null);
  const [result, setResult] = useState<IntegrateResult | null>(null);

  const repoById = useMemo(() => {
    const m = new Map<number, GraphRepo>();
    for (const r of repos) m.set(r.id, r);
    return m;
  }, [repos]);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const list = await getGraph();
      setRepos(list);
      setNodes(list.map((r, i) => repoNode(r, i)));
      // The backend exposes no persisted connections; start with no edges.
      setEdges([]);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  }, [setNodes, setEdges]);

  useEffect(() => {
    void load();
  }, [load]);

  // User drew an edge: open the modal instead of committing immediately.
  const onConnect = useCallback(
    (params: Connection) => {
      if (!params.source || !params.target || params.source === params.target) {
        return;
      }
      const source = repoById.get(Number(params.source));
      const target = repoById.get(Number(params.target));
      if (!source || !target) return;
      setPending({ source, target });
      setInstruction("");
      setModalError(null);
      setResult(null);
    },
    [repoById],
  );

  const closeModal = useCallback(() => {
    setPending(null);
    setInstruction("");
    setModalError(null);
    setResult(null);
    setIntegrating(false);
  }, []);

  const submitIntegration = useCallback(async () => {
    if (!pending) return;
    const text = instruction.trim();
    if (!text) {
      setModalError("Describe the integration before continuing.");
      return;
    }
    setIntegrating(true);
    setModalError(null);
    try {
      const res = await integrate({
        source_id: pending.source.id,
        target_id: pending.target.id,
        instruction: text,
      });
      setResult(res);
      // Render the new edge from the local source/target (the backend's
      // IntegrateResult does not echo the ids) labeled with the instruction.
      setEdges((eds) =>
        addEdge(
          connectionEdge({
            source_id: pending.source.id,
            target_id: pending.target.id,
            instruction: text,
          }),
          eds,
        ),
      );
    } catch (e) {
      setModalError(e instanceof Error ? e.message : String(e));
    } finally {
      setIntegrating(false);
    }
  }, [pending, instruction, setEdges]);

  return (
    <div className="mx-auto flex max-w-6xl flex-col gap-4">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h1 className="text-xl font-semibold text-slate-100">Connections</h1>
          <p className="mt-1 max-w-2xl text-sm text-slate-400">
            Drag from one repo to another to declare an integration. Claude
            scaffolds the wiring across both repos on their{" "}
            <code className="rounded bg-edge px-1 py-0.5 text-xs text-slate-200">
              repohub-staging
            </code>{" "}
            branches.
          </p>
        </div>
        <button
          onClick={() => void load()}
          disabled={loading}
          className="rounded-md border border-edge px-3 py-1.5 text-sm text-slate-300 transition hover:bg-edge hover:text-slate-100 disabled:opacity-50"
        >
          {loading ? "Loading…" : "Refresh"}
        </button>
      </div>

      {error ? (
        <div className="rounded-lg border border-rose-500/40 bg-rose-500/10 p-4 text-sm text-rose-300">
          <p className="font-medium">Could not load the connection graph.</p>
          <p className="mt-1 text-rose-300/80">{error}</p>
          <button
            onClick={() => void load()}
            className="mt-3 rounded-md border border-rose-500/40 px-3 py-1 text-xs text-rose-200 transition hover:bg-rose-500/20"
          >
            Try again
          </button>
        </div>
      ) : loading ? (
        <div className="h-[600px] animate-pulse rounded-xl border border-edge bg-edge/20" />
      ) : repos.length === 0 ? (
        <div className="rounded-xl border border-dashed border-edge p-12 text-center">
          <p className="text-sm text-slate-200">No tracked repos to connect.</p>
          <p className="mt-2 text-xs text-slate-500">
            Track some repositories on the Dashboard first — once a repo is
            tracked it appears here as a node you can wire to others.
          </p>
        </div>
      ) : (
        <div className="h-[600px] overflow-hidden rounded-xl border border-edge bg-panel">
          <ReactFlow
            nodes={nodes}
            edges={edges}
            onNodesChange={onNodesChange}
            onEdgesChange={onEdgesChange}
            onConnect={onConnect}
            fitView
            proOptions={{ hideAttribution: true }}
          >
            <Background color="#1e293b" gap={18} />
            <Controls />
          </ReactFlow>
        </div>
      )}

      {pending && (
        <IntegrateModal
          source={pending.source}
          target={pending.target}
          instruction={instruction}
          onInstructionChange={setInstruction}
          integrating={integrating}
          error={modalError}
          result={result}
          onCancel={closeModal}
          onSubmit={() => void submitIntegration()}
        />
      )}
    </div>
  );
}

// ---- modal ----

function IntegrateModal({
  source,
  target,
  instruction,
  onInstructionChange,
  integrating,
  error,
  result,
  onCancel,
  onSubmit,
}: {
  source: GraphRepo;
  target: GraphRepo;
  instruction: string;
  onInstructionChange: (v: string) => void;
  integrating: boolean;
  error: string | null;
  result: IntegrateResult | null;
  onCancel: () => void;
  onSubmit: () => void;
}) {
  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-4"
      onClick={onCancel}
    >
      <div
        className="w-full max-w-lg rounded-xl border border-edge bg-panel p-6 shadow-xl"
        onClick={(e) => e.stopPropagation()}
      >
        <h2 className="text-base font-semibold text-slate-100">
          Integrate{" "}
          <span className="text-accent">{source.name}</span> into{" "}
          <span className="text-accent">{target.name}</span>
        </h2>
        <p className="mt-1 text-xs text-slate-500">
          Describe how <code className="text-slate-300">{source.full_name}</code>{" "}
          should be wired into{" "}
          <code className="text-slate-300">{target.full_name}</code>. The change
          lands on each repo's repohub-staging branch.
        </p>

        {result ? (
          <div className="mt-4 space-y-3">
            <div className="rounded-md bg-emerald-500/10 px-3 py-2 text-sm text-emerald-300">
              Integration scaffolded.
            </div>
            <div>
              <p className="text-xs font-medium uppercase tracking-wide text-slate-500">
                Diff stat
              </p>
              <pre className="mt-1 max-h-32 overflow-auto whitespace-pre-wrap rounded-md border border-edge bg-slate-900/60 px-3 py-2 font-mono text-xs text-slate-300">
                {result.diff_stat || "(no changes reported)"}
              </pre>
            </div>
            <div>
              <p className="text-xs font-medium uppercase tracking-wide text-slate-500">
                Claude response
              </p>
              <pre className="mt-1 max-h-48 overflow-auto whitespace-pre-wrap rounded-md border border-edge bg-slate-900/60 px-3 py-2 text-xs text-slate-300">
                {result.response_excerpt || "(no response excerpt)"}
              </pre>
            </div>
            <div className="flex justify-end">
              <button
                onClick={onCancel}
                className="rounded-md bg-accent/20 px-4 py-2 text-sm text-accent transition hover:bg-accent/30"
              >
                Done
              </button>
            </div>
          </div>
        ) : (
          <>
            <textarea
              autoFocus
              value={instruction}
              onChange={(e) => onInstructionChange(e.target.value)}
              disabled={integrating}
              rows={5}
              placeholder={`e.g. Expose ${source.name}'s client as a typed dependency and call it from ${target.name}'s API layer.`}
              className="mt-4 w-full resize-y rounded-md border border-edge bg-slate-900/60 px-3 py-2 text-sm text-slate-200 outline-none placeholder:text-slate-600 focus:border-accent disabled:opacity-50"
            />

            {error && (
              <p className="mt-3 rounded-md bg-rose-500/10 px-3 py-2 text-sm text-rose-400">
                {error}
              </p>
            )}

            <div className="mt-4 flex justify-end gap-2">
              <button
                onClick={onCancel}
                disabled={integrating}
                className="rounded-md border border-edge px-4 py-2 text-sm text-slate-300 transition hover:bg-edge disabled:opacity-50"
              >
                Cancel
              </button>
              <button
                onClick={onSubmit}
                disabled={integrating || !instruction.trim()}
                className="rounded-md bg-accent/20 px-4 py-2 text-sm text-accent transition hover:bg-accent/30 disabled:cursor-not-allowed disabled:opacity-40"
              >
                {integrating ? "Integrating…" : "Integrate"}
              </button>
            </div>
          </>
        )}
      </div>
    </div>
  );
}
