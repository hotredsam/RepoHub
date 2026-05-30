// TypeScript interfaces mirroring the Rust models in backend/src/models.rs.
// All timestamps are ISO8601 strings.

export interface Repo {
  id: number;
  full_name: string;
  name: string;
  owner: string;
  private: boolean;
  language: string | null;
  description: string | null;
  default_branch: string;
  tracked: boolean;
  local_path: string | null;
  clone_status: string;
  ahead: number;
  behind: number;
  dirty: boolean;
  last_fetch: string | null;
  last_commit_at: string | null;
  disk_kb: number;
  updated_at: string;
}

export interface Prompt {
  id: number;
  repo_id: number | null;
  scope: string;
  source: string;
  model: string | null;
  prompt: string;
  response: string | null;
  tokens_in: number;
  tokens_out: number;
  created_at: string;
}

export interface BulkJob {
  id: number;
  kind: string;
  prompt: string;
  status: string;
  created_at: string;
}

export interface BulkJobItem {
  id: number;
  job_id: number;
  repo_id: number;
  status: string;
  branch: string | null;
  log: string | null;
  error: string | null;
}

// Convenience: a job returned together with its items.
export interface BulkJobWithItems extends BulkJob {
  items: BulkJobItem[];
}

export interface InfraResource {
  id: number;
  name: string;
  kind: string;
  host: string;
  port: number;
  username: string | null;
  base_path: string | null;
  notes: string | null;
  created_at: string;
}

export interface Setting {
  scope: string;
  repo_id: number | null;
  key: string;
  value: string;
}

// Well-known preference keys live as global settings rows.
export interface Preferences {
  pref_languages: string;
  pref_cloud: string;
  [key: string]: string;
}

// ---- Request / response payload helpers ----

export interface HealthResponse {
  ok: boolean;
  service: string;
  version: string;
}

export interface TrackBody {
  tracked: boolean;
}

export interface DeleteReposBody {
  ids: number[];
  delete_local: boolean;
  delete_remote: boolean;
}

export interface BulkPromptBody {
  repo_ids: number[];
  prompt: string;
  kind: string;
}

export interface CreateInfraBody {
  name: string;
  kind: string;
  host: string;
  port: number;
  username?: string | null;
  base_path?: string | null;
  notes?: string | null;
}

export interface InfraTestResult {
  ok: boolean;
  error?: string | null;
}

export interface PromptAnalytics {
  total_tokens_in: number;
  total_tokens_out: number;
  per_repo: { repo_id: number | null; count: number }[];
  per_day: { day: string; count: number }[];
}

export interface TranscriptStatus {
  indexed: number;
  last_reindex: string | null;
}

export interface ReindexResult {
  indexed: number;
}

// Message shape sent over /ws/claude as the first client frame.
export interface ClaudeWsRequest {
  repo_id: number | null;
  prompt: string;
}
