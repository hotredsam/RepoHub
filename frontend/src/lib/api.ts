// Typed, same-origin fetch helpers for every /api endpoint in the RepoHub contract.
// All helpers throw on a non-ok response.

import type {
  Repo,
  Prompt,
  BulkJob,
  BulkJobWithItems,
  InfraResource,
  Setting,
  Preferences,
  HealthResponse,
  DeleteReposBody,
  BulkPromptBody,
  CreateInfraBody,
  InfraTestResult,
  PromptAnalytics,
  TranscriptStatus,
  ReindexResult,
  SuggestBody,
  SuggestResponse,
  ApplySuggestionBody,
  ApplySuggestionResponse,
  AuthStatus,
  AuthConfigBody,
  AuthConfigResult,
  SessionRow,
  TsStatus,
  TsEnableResult,
  TsDisableResult,
  CodexState,
  CodexConfigBody,
  CodexCredential,
  IssuedToken,
  AuditEntry,
  PermissionsReport,
  AnalyzeResponse,
} from "./types";

// ---- low-level helper ----

async function request<T>(
  path: string,
  init?: RequestInit,
): Promise<T> {
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

function qs(params: Record<string, string | number | undefined | null>): string {
  const sp = new URLSearchParams();
  for (const [k, v] of Object.entries(params)) {
    if (v !== undefined && v !== null && v !== "") sp.set(k, String(v));
  }
  const s = sp.toString();
  return s ? `?${s}` : "";
}

// ---- health ----

export const getHealth = () => get<HealthResponse>("/api/health");

// ---- repos.rs ----

export const listRepos = () => get<Repo[]>("/api/repos");

export const refreshRepos = () => post<Repo[]>("/api/repos/refresh");

export const trackRepo = (id: number, tracked: boolean) =>
  post<Repo>(`/api/repos/${id}/track`, { tracked });

export const cloneRepo = (id: number) => post<Repo>(`/api/repos/${id}/clone`);

export const pullRepo = (id: number) => post<Repo>(`/api/repos/${id}/pull`);

export const pullAllRepos = () => post<Repo[]>("/api/repos/pull-all");

export const fetchAllRepos = () => post<Repo[]>("/api/repos/fetch-all");

export const refreshRepoStatus = (id: number) =>
  post<Repo>(`/api/repos/${id}/refresh-status`);

export const deleteRepos = (body: DeleteReposBody) =>
  del<{ deleted: number[] }>("/api/repos", body);

// ---- claude_api.rs (HTTP part; websocket lives in ws.ts) ----

export const recentPrompts = (repo_id?: number) =>
  get<Prompt[]>(`/api/prompts/recent${qs({ repo_id })}`);

// ---- bulk.rs ----

export const createBulkPrompt = (body: BulkPromptBody) =>
  post<BulkJobWithItems>("/api/bulk/prompt", body);

export const listBulkJobs = () => get<BulkJob[]>("/api/bulk/jobs");

export const getBulkJob = (id: number) =>
  get<BulkJobWithItems>(`/api/bulk/jobs/${id}`);

// ---- infra.rs ----

export const listInfra = () => get<InfraResource[]>("/api/infra");

export const createInfra = (body: CreateInfraBody) =>
  post<InfraResource>("/api/infra", body);

export const deleteInfra = (id: number) =>
  del<void>(`/api/infra/${id}`);

export const testInfra = (id: number) =>
  post<InfraTestResult>(`/api/infra/${id}/test`);

// ---- settings_api.rs ----

export const getSettings = (scope?: string, repo_id?: number) =>
  get<Setting[]>(`/api/settings${qs({ scope, repo_id })}`);

export const putSetting = (setting: Setting) =>
  put<Setting>("/api/settings", setting);

export const getPreferences = () => get<Preferences>("/api/preferences");

export const putPreferences = (prefs: Partial<Preferences>) =>
  put<Preferences>("/api/preferences", prefs);

// Ask Claude to PROPOSE repo config (preview only — nothing is written).
export const suggestSettings = (body: SuggestBody) =>
  post<SuggestResponse>("/api/settings/suggest", body);

// Write the selected suggested files onto the repo's staging branch.
export const applySuggestion = (body: ApplySuggestionBody) =>
  post<ApplySuggestionResponse>("/api/settings/apply-suggestion", body);

// ---- prompts_api.rs ----

export const searchPrompts = (opts: {
  search?: string;
  repo_id?: number;
  limit?: number;
}) =>
  get<Prompt[]>(
    `/api/prompts${qs({
      search: opts.search,
      repo_id: opts.repo_id,
      limit: opts.limit,
    })}`,
  );

export const promptAnalytics = () =>
  get<PromptAnalytics>("/api/prompts/analytics");

// ---- transcripts.rs ----

export const reindexTranscripts = () =>
  post<ReindexResult>("/api/transcripts/reindex");

export const transcriptStatus = () =>
  get<TranscriptStatus>("/api/transcripts/status");

// ---- auth.rs / auth_api.rs ----

export const getAuthStatus = () => get<AuthStatus>("/api/auth/status");

// Returns AuthStatusConfig (the 3 editable fields), not the full AuthStatus.
export const putAuthConfig = (body: AuthConfigBody) =>
  put<AuthConfigResult>("/api/auth/config", body);

export const logout = () => post<void>("/api/auth/logout");

export const listSessions = (email?: string) =>
  get<SessionRow[]>(`/api/auth/sessions${qs({ email })}`);

// Backend: POST /api/auth/sessions/:id/revoke (NOT DELETE /api/auth/sessions/:id).
export const revokeSession = (id: number) =>
  post<{ revoked: number }>(`/api/auth/sessions/${id}/revoke`);

// ---- tailscale.rs ----

export const getTailscale = () => get<TsStatus>("/api/tailscale");

// Returns { status, funnel_warning }.
export const enableTailscaleServe = () =>
  post<TsEnableResult>("/api/tailscale/serve/enable");

// Returns { status }.
export const disableTailscaleServe = () =>
  post<TsDisableResult>("/api/tailscale/serve/disable");

// ---- codex.rs ----

export const getCodex = () => get<CodexState>("/api/codex");

export const putCodex = (body: CodexConfigBody) =>
  put<CodexState>("/api/codex", body);

// Backend: POST /api/codex/token (SINGULAR). Returns { token, credential, warning }.
export const issueCodexToken = (label?: string) =>
  post<IssuedToken>("/api/codex/token", { label });

export const killCodex = () =>
  post<{ killed: boolean; enabled: boolean }>("/api/codex/kill");

export const listCodexCredentials = () =>
  get<CodexCredential[]>("/api/codex/credentials");

// Backend: POST /api/codex/credentials/:id/revoke (NOT DELETE).
export const revokeCodexCredential = (id: number) =>
  post<{ id: number; revoked: boolean }>(
    `/api/codex/credentials/${id}/revoke`,
  );

// ---- audit.rs ----

// Backend AuditQuery reads `actor`, `method`, `path`, `limit`, and `since`
// (RFC3339); there is no `before` cursor.
export const listAudit = (opts?: {
  actor?: string;
  method?: string;
  path?: string;
  limit?: number;
  since?: string;
}) =>
  get<AuditEntry[]>(
    `/api/audit${qs({
      actor: opts?.actor,
      method: opts?.method,
      path: opts?.path,
      limit: opts?.limit,
      since: opts?.since,
    })}`,
  );

// ---- gh_perms.rs ----

export const getPermissions = () =>
  get<PermissionsReport>("/api/permissions");

// Returns { summary, error?, raw } — NOT a bare PermissionsReport.
export const analyzePermissions = () =>
  post<AnalyzeResponse>("/api/permissions/analyze");
