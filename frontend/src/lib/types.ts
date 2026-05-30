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

// ---- settings_api.rs: AI suggestion of repo config ----

// One file the model proposes (and the unit we apply onto the staging branch).
export interface SuggestedFile {
  path: string;
  content: string;
}

export interface SuggestBody {
  repo_id: number;
}

export interface SuggestResponse {
  claude_md: string;
  settings_json: string;
  dev_configs: SuggestedFile[];
  rationale: string;
}

export interface ApplySuggestionBody {
  repo_id: number;
  files: SuggestedFile[];
}

export interface ApplySuggestionResponse {
  ok: boolean;
  branch: string;
}

// ---- auth.rs / auth_api.rs ----

// Current auth posture as seen by the browser. Mirrors the backend
// `/api/auth/status` JSON (auth_google::AuthStatus) exactly.
export interface AuthStatus {
  // Whether Google OAuth is configured (client id + secret resolvable).
  configured: boolean;
  // Whether this request arrived over the tailnet / a remote origin.
  remote: boolean;
  // Effective gate config (global settings).
  loopback_allowed: boolean;
  remote_required: boolean;
  // Whether the caller is currently authenticated as a user.
  authenticated: boolean;
  // Email of the signed-in user (when authenticated).
  email: string | null;
  // Allowlist of emails permitted to sign in.
  allowed_emails: string[];
  // The URL the frontend should send the browser to in order to log in.
  login_url: string;
}

// One session row as surfaced by GET /api/auth/sessions
// (auth_google::SessionView — token_hash is deliberately omitted, `revoked` is a
// bool, and `current` flags the caller's own session).
export interface SessionRow {
  id: number;
  email: string;
  origin: string | null;
  ua: string | null;
  ip: string | null;
  created_at: string;
  last_seen_at: string | null;
  expires_at: string | null;
  revoked: boolean;
  current: boolean;
}

// Editable subset of the gate config (auth_google::ConfigBody). `allowed_emails`
// is a comma/space-separated string on the backend (Option<String>).
export interface AuthConfigBody {
  loopback_allowed?: boolean;
  remote_required?: boolean;
  allowed_emails?: string;
}

// Response of PUT /api/auth/config (auth_google::AuthStatusConfig) — only the
// three editable fields, NOT the full AuthStatus.
export interface AuthConfigResult {
  allowed_emails: string[];
  loopback_allowed: boolean;
  remote_required: boolean;
}

// ---- tailscale.rs ----

// Mirrors tailscale::TsStatus exactly.
export interface TsStatus {
  // Whether the `tailscale` CLI is discoverable (PATH or macOS app bundle).
  installed: boolean;
  // Whether this node is logged in to a tailnet.
  logged_in: boolean;
  // Whether a `tailscale serve` mapping is currently active.
  serve_enabled: boolean;
  // WARNING: whether a public Funnel is active. RepoHub never enables it.
  funnel_enabled: boolean;
  // The published HTTPS URL when serve is enabled.
  published_url: string | null;
  // This node's MagicDNS name, e.g. samuels-mac-mini.tail97ef37.ts.net.
  dns_name: string | null;
  // The tailnet (MagicDNSSuffix), e.g. tail97ef37.ts.net.
  tailnet: string | null;
}

// Response of POST /api/tailscale/serve/enable (tailscale::EnableResult).
export interface TsEnableResult {
  status: TsStatus;
  // Surfaced only when a public Funnel was detected (RepoHub never enables it).
  funnel_warning: string | null;
}

// Response of POST /api/tailscale/serve/disable.
export interface TsDisableResult {
  status: TsStatus;
}

// ---- codex.rs ----

// One entry in the documented Codex API surface (codex::api_surface()).
export interface CodexApiSurfaceEntry {
  method: string;
  path: string;
  summary: string;
  destructive?: boolean;
}

// Body of GET / PUT /api/codex (codex::get_codex). Never includes a raw token.
export interface CodexState {
  config: {
    enabled: boolean;
    destructive_confirm: boolean;
    token_prefix: string;
  };
  status: {
    enabled: boolean;
    active_credentials: number;
    has_active_credential: boolean;
  };
  // Metadata for the single active credential, if any.
  active_credential: CodexCredential | null;
  api_surface: CodexApiSurfaceEntry[];
}

// Editable subset of the Codex config (codex::PutCodexBody).
export interface CodexConfigBody {
  enabled?: boolean;
  destructive_confirm?: boolean;
}

// Credential metadata safe to show the owner (codex::CredentialMeta — never the
// hash or raw token).
export interface CodexCredential {
  id: number;
  label: string | null;
  created_at: string;
  last_used_at: string | null;
  revoked: boolean;
  revoked_at: string | null;
}

// Response of POST /api/codex/token — the raw token is shown ONCE here.
export interface IssuedToken {
  token: string;
  credential: {
    id: number;
    label: string | null;
    created_at: string;
  };
  warning: string;
}

// ---- audit.rs ----

export interface AuditEntry {
  id: number;
  ts: string;
  actor: string | null;
  actor_kind: string | null;
  credential_id: number | null;
  method: string | null;
  path: string | null;
  query: string | null;
  status: number | null;
  summary: string | null;
  body_excerpt: string | null;
}

// ---- gh_perms.rs ----

// One scope flagged as broader than this local-first app needs.
export interface OverBroad {
  scope: string;
  why: string;
}

// Per-repo permission summary (best-effort; null/false on probe failure).
export interface RepoPerm {
  full_name: string;
  role: string | null;
  branch_protected: boolean;
}

// Body of GET /api/permissions (gh_perms::PermissionsReport).
export interface PermissionsReport {
  account: string | null;
  token_scopes: string[];
  over_broad: OverBroad[];
  repos: RepoPerm[];
  // Always null on GET; populated only by POST /api/permissions/analyze.
  summary: string | null;
}

// Body of POST /api/permissions/analyze: the Claude summary (or null) plus a
// degradation note and the same raw report regardless of CLI availability.
export interface AnalyzeResponse {
  summary: string | null;
  error?: string | null;
  raw: PermissionsReport;
}
