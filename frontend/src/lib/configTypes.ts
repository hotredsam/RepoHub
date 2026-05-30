// TypeScript types for the P16 unified, layered Settings hub payloads.
//
// These mirror the Rust foundation API in backend/src/claude_fs.rs and
// backend/src/gcloud.rs, plus the eval_suites / eval_runs DB schema. They are
// kept separate from lib/types.ts (P0-P15) so the new config surface does not
// touch the existing contract types. All timestamps are ISO8601 strings.

import type { Setting } from "./types";

// ---------------------------------------------------------------------------
// claude_fs — global settings, history, agents
// ---------------------------------------------------------------------------

// The merged contents of ~/.claude/settings.json (live, global layer). A free
// JSON object — Claude Code defines the concrete keys.
export type ClaudeSettings = Record<string, unknown>;

// GET /api/config/global — the current live global settings document.
export interface GlobalConfig {
  settings: ClaudeSettings;
}

// PUT /api/config/global — a JSON patch deep-merged into ~/.claude/settings.json
// (MERGE keys, never blind-overwrite). `null` overwrites a key with null.
export interface GlobalConfigPatch {
  patch: ClaudeSettings;
}

// One entry in the ~/.claude git safety-net history (newest first).
// Mirrors claude_fs::HistoryEntry.
export interface HistoryEntry {
  hash: string;
  msg: string;
  date: string;
}

// POST /api/config/revert — restore settings.json to its state at `hash`.
export interface RevertBody {
  hash: string;
}

// A Claude Code subagent: YAML frontmatter fields + a Markdown body that serves
// as the system prompt. Mirrors claude_fs::Agent (None fields omitted on the
// wire, so they arrive as undefined here).
export interface Agent {
  name: string;
  description?: string | null;
  model?: string | null;
  // Comma/space-separated tool list, as authored in the frontmatter.
  tools?: string | null;
  // The Markdown system prompt below the frontmatter.
  body: string;
}

// GET /api/config/agents — the agent set for a scope.
//
// `scope = "global"` reads ~/.claude/agents (live). `scope = "repo"` with a
// repo_id reads the repo's .claude/agents on the repohub-staging branch.
export interface AgentsResponse {
  scope: string;
  repo_id: number | null;
  agents: Agent[];
}

// PUT /api/config/agents — create/update one agent in the given scope.
//
// Flat wire shape matching the backend agents_config::PutAgentBody: the agent's
// body is sent as `system_prompt`.
export interface PutAgentBody {
  // Defaults to "global" when omitted.
  scope?: string;
  // Required when scope = "repo".
  repo_id?: number | null;
  name: string;
  description?: string | null;
  model?: string | null;
  tools?: string | null;
  // The Markdown system prompt (Agent.body on the client).
  system_prompt: string;
}

// PUT /api/config/agents response. Mirrors agents_config::WriteAgentResponse —
// it echoes metadata, NOT the full Agent.
export interface WriteAgentResponse {
  ok: boolean;
  scope: string;
  repo_id: number | null;
  name: string;
  // Absolute path of the written file.
  path: string;
  // For repo scope, the branch the change was committed to.
  branch?: string | null;
}

// DELETE /api/config/agents — remove an agent by name from the given scope.
export interface DeleteAgentBody {
  scope?: string;
  repo_id?: number | null;
  name: string;
}

// ---------------------------------------------------------------------------
// Per-repo config layer — overrides that land on repohub-staging for review.
// ---------------------------------------------------------------------------

// One configured MCP server, normalized by the backend (mcp_config::McpServer).
// The backend returns a flat ARRAY of these (not an object map).
export interface McpServer {
  name: string;
  command: string;
  args: string[];
  // Environment variables; may carry secrets, so never logged.
  env: Record<string, unknown>;
}

// GET /api/config/repo/:id — the full per-repo override layer for review.
//
// Per-repo overrides inherit the global layer when unset; the UI uses
// `settings` (the repo's .claude/settings.json) plus `global` to show
// inherit-vs-override. All per-repo values come from the repohub-staging branch
// (not the live working tree).
export interface RepoConfig {
  repo_id: number;
  branch: string;
  // The repo's .claude/settings.json (override layer); {} when unset.
  settings: ClaudeSettings;
  // The live global settings this repo inherits from.
  global: ClaudeSettings;
  // The repo's .mcp.json servers; [] when unset.
  mcp: McpServer[];
  // The repo's .claude/agents/*.
  agents: Agent[];
}

// GET /api/config/mcp?repo_id= — MCP servers for a scope.
//
// `scope = "global"` reads ~/.claude.json mcpServers (or ~/.claude/settings.json);
// `scope = "repo"` reads the repo's .mcp.json on repohub-staging. `servers` is a
// flat ARRAY (mcp_config::ListResponse / WriteResponse).
export interface McpResponse {
  scope: string;
  repo_id: number | null;
  // Present on list responses (the source path/spec the servers were read from).
  source?: string;
  // For per-repo writes, the staging branch the change landed on.
  branch?: string | null;
  servers: McpServer[];
}

// PUT /api/config/mcp — upsert one MCP server entry. Flat wire shape matching
// mcp_config::UpsertBody (command/args/env at the top level).
export interface PutMcpBody {
  scope?: string;
  repo_id?: number | null;
  name: string;
  command: string;
  args?: string[];
  env?: Record<string, unknown>;
}

// DELETE /api/config/mcp — remove an MCP server entry by name.
export interface DeleteMcpBody {
  scope?: string;
  repo_id?: number | null;
  name: string;
}

// ---------------------------------------------------------------------------
// Knowledge — embeddings + vector store (Vertex AI or local, per scope).
// ---------------------------------------------------------------------------

// Which embedding backend to use. Mirrors gcloud::EmbedProvider
// (serde rename_all = "lowercase").
export type EmbedProvider = "vertex" | "local";

// The knowledge config for one layer. Mirrors knowledge.rs::KnowledgeConfig.
export interface KnowledgeLayer {
  memory_enabled: boolean;
  rag_enabled: boolean;
  embedding_provider: EmbedProvider;
  context_budget_tokens: number;
}

// GET/PUT /api/knowledge/config — mirrors knowledge.rs::ConfigResponse. The
// real values live under `effective`; `layer` is null when this scope inherits.
export interface KnowledgeConfig {
  scope: string;
  repo_id: number | null;
  // The raw config stored for THIS layer, or null when it inherits.
  layer: KnowledgeLayer | null;
  // The effective config after applying the inheritance chain.
  effective: KnowledgeLayer;
  // True when this layer inherits the global default.
  inherited: boolean;
}

// PUT /api/knowledge/config — a partial patch (knowledge.rs::PutConfigBody +
// flattened KnowledgeConfigPatch). Only supplied fields change.
export interface PutKnowledgeConfigBody {
  scope?: string;
  repo_id?: number | null;
  memory_enabled?: boolean;
  rag_enabled?: boolean;
  embedding_provider?: EmbedProvider;
  context_budget_tokens?: number;
}

// POST /api/knowledge/reindex — (re)embed + upsert into the vector store.
// Stubbed via the gcloud lib; degrades gracefully when gcloud is unconfigured.
export interface ReindexKnowledgeBody {
  // Repos to (re)index; empty means "all configured".
  repo_ids?: number[];
}

// One per-repo reindex outcome. Mirrors knowledge.rs::ReindexResult.
export interface ReindexResult {
  repo_id: number;
  // ok | pending | disabled | error
  status: string;
  message: string;
}

// Mirrors knowledge.rs::ReindexResponse.
export interface ReindexKnowledgeResult {
  gcloud_ready: boolean;
  results: ReindexResult[];
}

// POST /api/knowledge/query — nearest-neighbour search over the vector store.
// Mirrors knowledge.rs::QueryBody (the query text field is `q`).
export interface KnowledgeQueryBody {
  scope?: string;
  repo_id?: number | null;
  q: string;
}

// One match returned from a vector query. Mirrors gcloud::VectorMatch.
export interface VectorMatch {
  id: string;
  score: number;
  metadata: unknown;
}

// Mirrors knowledge.rs::QueryResponse.
export interface KnowledgeQueryResult {
  gcloud_ready: boolean;
  // ok | disabled | not_configured
  status: string;
  message: string;
  matches: VectorMatch[];
}

// ---------------------------------------------------------------------------
// Google Cloud — the single config block (project + region + ADC/installed).
// ---------------------------------------------------------------------------

// GET /api/gcloud — local Google Cloud status. Mirrors gcloud::GcloudStatus.
export interface GcloudStatus {
  // `which gcloud` succeeded.
  installed: boolean;
  // Application Default Credentials file present on disk.
  adc: boolean;
  // Configured project id (global setting "gcloud_project").
  project: string | null;
  // Configured region (global setting "gcloud_region").
  region: string | null;
}

// PUT /api/gcloud — set the project/region global config block.
export interface PutGcloudBody {
  project?: string | null;
  region?: string | null;
}

// ---------------------------------------------------------------------------
// Evals — Vertex AI Gen AI Evaluation (stubbed via gcloud lib).
// ---------------------------------------------------------------------------

// A stored evaluation suite. Mirrors the eval_suites table.
export interface EvalSuite {
  id: number;
  name: string;
  scope: string;
  repo_id: number | null;
  // Serialized JSON eval config (TEXT column); null when unset.
  config_json: string | null;
  created_at: string | null;
}

// POST /api/evals — create a suite.
export interface CreateEvalSuiteBody {
  name: string;
  scope?: string;
  repo_id?: number | null;
  // The eval config object; serialized to config_json server-side.
  config?: unknown;
}

// One recorded run of a suite. Mirrors the eval_runs table.
export interface EvalRun {
  id: number;
  suite_id: number | null;
  model: string | null;
  score: number | null;
  passed: number | null;
  total: number | null;
  // Serialized JSON detail (TEXT column); null when unset.
  detail_json: string | null;
  created_at: string | null;
}

// Re-export Setting so config-layer callers have a single import surface.
export type { Setting };
