// Typed, same-origin fetch helpers for the P16 unified Settings hub endpoints.
//
// Layered Claude Code config: GLOBAL (live ~/.claude) + optional PER-REPO
// override (lands on repohub-staging for review). Matches the style of
// lib/api.ts — same low-level `request` helper, all helpers throw on a non-ok
// response. Kept separate from api.ts so the new config surface is isolated.

import type {
  GlobalConfig,
  GlobalConfigPatch,
  ClaudeSettings,
  HistoryEntry,
  RevertBody,
  AgentsResponse,
  PutAgentBody,
  WriteAgentResponse,
  DeleteAgentBody,
  RepoConfig,
  McpResponse,
  PutMcpBody,
  DeleteMcpBody,
  KnowledgeConfig,
  PutKnowledgeConfigBody,
  ReindexKnowledgeBody,
  ReindexKnowledgeResult,
  KnowledgeQueryBody,
  KnowledgeQueryResult,
  GcloudStatus,
  PutGcloudBody,
  EvalSuite,
  CreateEvalSuiteBody,
  EvalRun,
} from "./configTypes";

// ---- low-level helper (mirrors lib/api.ts) ----

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

const get = <T>(path: string) => request<T>(path);
const post = <T>(path: string, body?: unknown) =>
  request<T>(path, {
    method: "POST",
    body: body === undefined ? undefined : JSON.stringify(body),
  });
const put = <T>(path: string, body?: unknown) =>
  request<T>(path, {
    method: "PUT",
    body: body === undefined ? undefined : JSON.stringify(body),
  });
const del = <T>(path: string, body?: unknown) =>
  request<T>(path, {
    method: "DELETE",
    body: body === undefined ? undefined : JSON.stringify(body),
  });

function qs(
  params: Record<string, string | number | undefined | null>,
): string {
  const sp = new URLSearchParams();
  for (const [k, v] of Object.entries(params)) {
    if (v !== undefined && v !== null && v !== "") sp.set(k, String(v));
  }
  const s = sp.toString();
  return s ? `?${s}` : "";
}

// ---- config/global (live ~/.claude/settings.json) ----

// Read the current live global settings document.
export const getGlobalConfig = () => get<GlobalConfig>("/api/config/global");

// Deep-merge a patch into ~/.claude/settings.json (snapshots first; MERGE keys).
export const putGlobalConfig = (patch: ClaudeSettings) =>
  put<GlobalConfig>("/api/config/global", { patch } satisfies GlobalConfigPatch);

// ---- config/history + revert (the ~/.claude git safety net) ----

// Git log of ~/.claude, newest first ([] when not yet a repo).
export const getConfigHistory = () =>
  get<HistoryEntry[]>("/api/config/history");

// Restore settings.json to its state at `hash` (snapshots before reverting).
export const revertConfig = (hash: string) =>
  post<GlobalConfig>("/api/config/revert", { hash } satisfies RevertBody);

// ---- config/repo/:id (per-repo override layer, for review) ----

// The full per-repo config layer (settings/mcp/agents) plus the inherited
// global layer, read from the repohub-staging branch.
export const getRepoConfig = (repoId: number) =>
  get<RepoConfig>(`/api/config/repo/${repoId}`);

// ---- config/agents (subagents: global live, per-repo on staging) ----

// List agents for a scope. Omit args for the global set.
export const getAgents = (scope?: string, repo_id?: number) =>
  get<AgentsResponse>(`/api/config/agents${qs({ scope, repo_id })}`);

// Create/update one agent in the given scope. Returns write metadata, not the
// full Agent — re-fetch or reuse the local agent for the list row.
export const putAgent = (body: PutAgentBody) =>
  put<WriteAgentResponse>("/api/config/agents", body);

// Remove an agent by name from the given scope.
export const deleteAgent = (body: DeleteAgentBody) =>
  del<void>("/api/config/agents", body);

// ---- config/mcp (.mcp.json server map) ----

// MCP server map for a scope. Omit args for the global set.
export const getMcp = (scope?: string, repo_id?: number) =>
  get<McpResponse>(`/api/config/mcp${qs({ scope, repo_id })}`);

// Upsert one MCP server entry.
export const putMcp = (body: PutMcpBody) =>
  put<McpResponse>("/api/config/mcp", body);

// Remove one MCP server entry by name.
export const deleteMcp = (body: DeleteMcpBody) =>
  del<McpResponse>("/api/config/mcp", body);

// ---- knowledge (embeddings + vector store, per scope) ----

// Read the embedding/vector configuration for a scope (inherits global).
export const getKnowledgeConfig = (scope?: string, repo_id?: number) =>
  get<KnowledgeConfig>(`/api/knowledge/config${qs({ scope, repo_id })}`);

// Set the embedding/vector configuration for a scope.
export const putKnowledgeConfig = (body: PutKnowledgeConfigBody) =>
  put<KnowledgeConfig>("/api/knowledge/config", body);

// (Re)embed + upsert into the vector store (stubbed; degrades gracefully).
export const reindexKnowledge = (body?: ReindexKnowledgeBody) =>
  post<ReindexKnowledgeResult>("/api/knowledge/reindex", body);

// Nearest-neighbour search over the vector store.
export const queryKnowledge = (body: KnowledgeQueryBody) =>
  post<KnowledgeQueryResult>("/api/knowledge/query", body);

// ---- gcloud (the single project + region + ADC/installed config block) ----

// Local Google Cloud status (installed/adc/project/region).
export const getGcloud = () => get<GcloudStatus>("/api/gcloud");

// Set the project/region global config block.
export const putGcloud = (body: PutGcloudBody) =>
  put<GcloudStatus>("/api/gcloud", body);

// ---- evals (Vertex AI Gen AI Evaluation; stubbed via gcloud lib) ----

// List evaluation suites (optionally filtered by scope/repo).
export const listEvalSuites = (scope?: string, repo_id?: number) =>
  get<EvalSuite[]>(`/api/evals${qs({ scope, repo_id })}`);

// Create an evaluation suite.
export const createEvalSuite = (body: CreateEvalSuiteBody) =>
  post<EvalSuite>("/api/evals", body);

// Delete an evaluation suite by id.
export const deleteEvalSuite = (id: number) =>
  del<void>(`/api/evals/${id}`);

// Run a suite (records an eval_run). Stubbed; degrades gracefully.
export const runEvalSuite = (id: number) =>
  post<EvalRun>(`/api/evals/${id}/run`);

// List recorded eval runs (optionally for a single suite).
export const listEvalRuns = (suite_id?: number) =>
  get<EvalRun[]>(`/api/evals/runs${qs({ suite_id })}`);
