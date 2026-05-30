//! Knowledge & retrieval (memory / RAG / embeddings / context) configuration.
//!
//! Part of the P16 layered Settings hub. The knowledge configuration is stored
//! as a single JSON blob in the existing `settings` table under the well-known
//! key `knowledge_config`, layered exactly like every other setting:
//!
//! - `scope = 'global'`, `repo_id IS NULL`  — the global default.
//! - `scope = 'repo'`,   `repo_id = N`      — an optional per-repo override.
//!
//! A per-repo row, when present, fully overrides the global row (an UNSET repo
//! row inherits the global default). The effective config returned to callers is
//! the global default with any per-repo override merged on top, so the UI can
//! show inherit-vs-override per field.
//!
//! Reindex / query route through `crate::gcloud`, which DEGRADES GRACEFULLY when
//! `gcloud`/ADC are absent: rather than crashing we report a clear, actionable
//! "configure Google Cloud" status. Live Vertex AI calls are TODO in `gcloud`.

use axum::extract::{Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::{ApiResult, AppError};
use crate::gcloud::{self, EmbedProvider};
use crate::state::AppState;

/// Well-known setting key holding the knowledge configuration JSON blob.
const KNOWLEDGE_KEY: &str = "knowledge_config";

/// Layer scopes. Mirrors the convention used elsewhere in the settings hub.
const GLOBAL_SCOPE: &str = "global";
const REPO_SCOPE: &str = "repo";

/// Default context budget when none is configured.
const DEFAULT_CONTEXT_BUDGET_TOKENS: u32 = 8_000;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/knowledge/config",
            get(get_config).put(put_config),
        )
        .route("/api/knowledge/reindex", post(reindex))
        .route("/api/knowledge/query", post(query))
}

// ---------------------------------------------------------------------------
// Knowledge configuration shape.
// ---------------------------------------------------------------------------

/// The knowledge configuration for one layer (global or a single repo).
///
/// Defaults are ENABLED (memory + RAG on, Vertex embeddings) per the P16
/// contract. Deserialization is lenient: a missing or partial stored blob falls
/// back to these defaults so an old/empty row never errors.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnowledgeConfig {
    /// Whether long-term memory is enabled for this scope.
    pub memory_enabled: bool,
    /// Whether retrieval-augmented generation is enabled for this scope.
    pub rag_enabled: bool,
    /// Which embedding backend to use (`vertex` | `local`).
    pub embedding_provider: EmbedProvider,
    /// Soft cap on how many tokens of retrieved context to inject.
    pub context_budget_tokens: u32,
}

impl Default for KnowledgeConfig {
    fn default() -> Self {
        Self {
            memory_enabled: true,
            rag_enabled: true,
            embedding_provider: EmbedProvider::Vertex,
            context_budget_tokens: DEFAULT_CONTEXT_BUDGET_TOKENS,
        }
    }
}

/// A partial config patch as sent by the UI on PUT — every field optional so a
/// caller can toggle a single setting without restating the rest.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct KnowledgeConfigPatch {
    pub memory_enabled: Option<bool>,
    pub rag_enabled: Option<bool>,
    pub embedding_provider: Option<EmbedProvider>,
    pub context_budget_tokens: Option<u32>,
}

impl KnowledgeConfig {
    /// Apply a patch in place, leaving unspecified fields untouched.
    fn apply(&mut self, patch: &KnowledgeConfigPatch) {
        if let Some(v) = patch.memory_enabled {
            self.memory_enabled = v;
        }
        if let Some(v) = patch.rag_enabled {
            self.rag_enabled = v;
        }
        if let Some(v) = patch.embedding_provider {
            self.embedding_provider = v;
        }
        if let Some(v) = patch.context_budget_tokens {
            self.context_budget_tokens = v;
        }
    }
}

// ---------------------------------------------------------------------------
// Settings persistence (NULL-aware upsert + scoped reads).
// ---------------------------------------------------------------------------

/// Read the raw knowledge_config row for a single layer, parsed into a typed
/// config. Returns `None` when no row exists (so the caller can distinguish
/// "inherits" from "explicitly set"). A row that fails to parse falls back to
/// defaults rather than erroring, so a hand-edited or legacy blob is tolerated.
async fn read_layer(
    state: &AppState,
    scope: &str,
    repo_id: Option<i64>,
) -> ApiResult<Option<KnowledgeConfig>> {
    let row: Option<(String,)> = sqlx::query_as::<_, (String,)>(
        "SELECT value FROM settings \
         WHERE scope = ?1 \
           AND ((?2 IS NULL AND repo_id IS NULL) OR repo_id = ?2) \
           AND key = ?3",
    )
    .bind(scope)
    .bind(repo_id)
    .bind(KNOWLEDGE_KEY)
    .fetch_optional(&state.db)
    .await?;

    Ok(row.map(|(value,)| serde_json::from_str(&value).unwrap_or_default()))
}

/// Upsert the knowledge_config row for one layer.
///
/// SQLite treats `NULL` as distinct in unique constraints, so we mirror
/// `settings_api`'s explicit update-then-insert (NULL-aware) rather than relying
/// on `ON CONFLICT`. Runtime sqlx only — no compile-time macros.
async fn upsert_layer(
    state: &AppState,
    scope: &str,
    repo_id: Option<i64>,
    cfg: &KnowledgeConfig,
) -> ApiResult<()> {
    let value = serde_json::to_string(cfg)
        .map_err(|e| AppError::msg(format!("failed to serialize knowledge config: {e}")))?;

    let mut tx = state.db.begin().await?;

    let updated = sqlx::query(
        "UPDATE settings SET value = ?4 \
         WHERE scope = ?1 \
           AND ((?2 IS NULL AND repo_id IS NULL) OR repo_id = ?2) \
           AND key = ?3",
    )
    .bind(scope)
    .bind(repo_id)
    .bind(KNOWLEDGE_KEY)
    .bind(&value)
    .execute(&mut *tx)
    .await?;

    if updated.rows_affected() == 0 {
        sqlx::query(
            "INSERT INTO settings (scope, repo_id, key, value) VALUES (?1, ?2, ?3, ?4)",
        )
        .bind(scope)
        .bind(repo_id)
        .bind(KNOWLEDGE_KEY)
        .bind(&value)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok(())
}

/// Resolve the effective config for a layer: the global default, with the
/// per-repo override (if any) merged on top. When `repo_id` is `None` this is
/// just the global config (or its defaults when unset).
async fn resolve_effective(
    state: &AppState,
    repo_id: Option<i64>,
) -> ApiResult<KnowledgeConfig> {
    let mut effective = read_layer(state, GLOBAL_SCOPE, None)
        .await?
        .unwrap_or_default();

    if let Some(id) = repo_id {
        if let Some(repo_cfg) = read_layer(state, REPO_SCOPE, Some(id)).await? {
            effective = repo_cfg;
        }
    }
    Ok(effective)
}

// ---------------------------------------------------------------------------
// GET /api/knowledge/config?scope=&repo_id=
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct ConfigQuery {
    /// `global` (default) or `repo`.
    pub scope: Option<String>,
    pub repo_id: Option<i64>,
}

/// Normalize a requested scope into one of the two known layers. Anything other
/// than `repo` (including the absent/empty case) resolves to `global`.
fn normalize_scope(scope: Option<&str>) -> &'static str {
    match scope.map(str::trim) {
        Some(REPO_SCOPE) => REPO_SCOPE,
        _ => GLOBAL_SCOPE,
    }
}

#[derive(Debug, Serialize)]
pub struct ConfigResponse {
    /// Which layer this response describes.
    pub scope: String,
    pub repo_id: Option<i64>,
    /// The raw config stored for THIS layer, or `null` when it inherits.
    pub layer: Option<KnowledgeConfig>,
    /// The effective config after applying the inheritance chain.
    pub effective: KnowledgeConfig,
    /// `true` when this layer has no row of its own and inherits the global
    /// default. Always `false` for the global layer itself.
    pub inherited: bool,
}

async fn get_config(
    State(state): State<AppState>,
    Query(q): Query<ConfigQuery>,
) -> ApiResult<Json<ConfigResponse>> {
    let scope = normalize_scope(q.scope.as_deref());

    if scope == REPO_SCOPE {
        let repo_id = q
            .repo_id
            .ok_or_else(|| AppError::msg("repo_id is required for scope=repo"))?;

        let layer = read_layer(&state, REPO_SCOPE, Some(repo_id)).await?;
        let effective = resolve_effective(&state, Some(repo_id)).await?;
        let inherited = layer.is_none();

        Ok(Json(ConfigResponse {
            scope: REPO_SCOPE.to_string(),
            repo_id: Some(repo_id),
            layer,
            effective,
            inherited,
        }))
    } else {
        let layer = read_layer(&state, GLOBAL_SCOPE, None).await?;
        // The effective global config is the stored row or the built-in default.
        let effective = layer.clone().unwrap_or_default();

        Ok(Json(ConfigResponse {
            scope: GLOBAL_SCOPE.to_string(),
            repo_id: None,
            layer,
            effective,
            inherited: false,
        }))
    }
}

// ---------------------------------------------------------------------------
// PUT /api/knowledge/config  — upsert the config for one layer.
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct PutConfigBody {
    /// `global` (default) or `repo`.
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub repo_id: Option<i64>,
    /// Partial patch — only the supplied fields change. For a repo layer that
    /// currently inherits, the patch is applied on top of the effective global
    /// config so toggling one field still produces a complete override row.
    #[serde(flatten)]
    pub patch: KnowledgeConfigPatch,
}

async fn put_config(
    State(state): State<AppState>,
    Json(body): Json<PutConfigBody>,
) -> ApiResult<Json<ConfigResponse>> {
    let scope = normalize_scope(body.scope.as_deref());

    if scope == REPO_SCOPE {
        let repo_id = body
            .repo_id
            .ok_or_else(|| AppError::msg("repo_id is required for scope=repo"))?;

        // Start from the repo's existing override if it has one; otherwise from
        // the effective (inherited) config so a partial patch yields a complete,
        // self-contained override row.
        let mut cfg = match read_layer(&state, REPO_SCOPE, Some(repo_id)).await? {
            Some(existing) => existing,
            None => resolve_effective(&state, Some(repo_id)).await?,
        };
        cfg.apply(&body.patch);
        upsert_layer(&state, REPO_SCOPE, Some(repo_id), &cfg).await?;

        let effective = resolve_effective(&state, Some(repo_id)).await?;
        Ok(Json(ConfigResponse {
            scope: REPO_SCOPE.to_string(),
            repo_id: Some(repo_id),
            layer: Some(cfg),
            effective,
            inherited: false,
        }))
    } else {
        let mut cfg = read_layer(&state, GLOBAL_SCOPE, None)
            .await?
            .unwrap_or_default();
        cfg.apply(&body.patch);
        upsert_layer(&state, GLOBAL_SCOPE, None, &cfg).await?;

        Ok(Json(ConfigResponse {
            scope: GLOBAL_SCOPE.to_string(),
            repo_id: None,
            layer: Some(cfg.clone()),
            effective: cfg,
            inherited: false,
        }))
    }
}

// ---------------------------------------------------------------------------
// POST /api/knowledge/reindex  — (re)embed + upsert vectors for repos.
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct ReindexBody {
    /// Repos to (re)index. Empty means "all configured" — but with no live
    /// backend yet we simply echo the request and report pending status.
    #[serde(default)]
    pub repo_ids: Vec<i64>,
}

#[derive(Debug, Serialize)]
pub struct ReindexResult {
    pub repo_id: i64,
    /// `ok`, `pending`, `disabled`, or `error`.
    pub status: String,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct ReindexResponse {
    /// Whether Google Cloud is ready for live calls (informational).
    pub gcloud_ready: bool,
    pub results: Vec<ReindexResult>,
}

async fn reindex(
    State(state): State<AppState>,
    Json(body): Json<ReindexBody>,
) -> ApiResult<Json<ReindexResponse>> {
    let gstatus = gcloud::status(&state.db).await;
    let ready = gstatus.ready();

    let mut results = Vec::with_capacity(body.repo_ids.len());

    for repo_id in body.repo_ids {
        // Per-repo effective config decides whether RAG is even on and which
        // provider to embed with.
        let cfg = resolve_effective(&state, Some(repo_id)).await?;

        if !cfg.rag_enabled {
            results.push(ReindexResult {
                repo_id,
                status: "disabled".to_string(),
                message: "RAG is disabled for this repo (enable it in knowledge settings)"
                    .to_string(),
            });
            continue;
        }

        if !ready {
            // Degrade gracefully — never crash. Record a pending status the UI
            // can surface alongside a "configure Google Cloud" call to action.
            results.push(ReindexResult {
                repo_id,
                status: "pending".to_string(),
                message: "pending: configure Google Cloud (install gcloud + run \
                          gcloud auth application-default login)"
                    .to_string(),
            });
            continue;
        }

        // gcloud is detected: attempt the (currently stubbed) live path. Both
        // embed + upsert are TODO inside `gcloud` and will return a clear
        // not-yet-implemented error; surface that per-repo rather than failing
        // the whole batch.
        let probe = vec!["repohub:reindex-probe".to_string()];
        let outcome = match gcloud::embed(&state.db, cfg.embedding_provider, &probe).await {
            Ok(vectors) => {
                let items: Vec<gcloud::VectorItem> = vectors
                    .into_iter()
                    .enumerate()
                    .map(|(i, embedding)| gcloud::VectorItem {
                        id: format!("repo:{repo_id}:{i}"),
                        embedding,
                        metadata: json!({ "repo_id": repo_id }),
                    })
                    .collect();
                match gcloud::vector_upsert(&state.db, &items).await {
                    Ok(()) => ("ok".to_string(), "reindexed".to_string()),
                    Err(e) => ("error".to_string(), e.to_string()),
                }
            }
            Err(e) => ("error".to_string(), e.to_string()),
        };

        results.push(ReindexResult {
            repo_id,
            status: outcome.0,
            message: outcome.1,
        });
    }

    Ok(Json(ReindexResponse {
        gcloud_ready: ready,
        results,
    }))
}

// ---------------------------------------------------------------------------
// POST /api/knowledge/query  — retrieval test.
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct QueryBody {
    /// `global` (default) or `repo` — selects which layer's config applies.
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub repo_id: Option<i64>,
    /// The free-text query to embed + look up.
    pub q: String,
}

#[derive(Debug, Serialize)]
pub struct QueryResponse {
    pub gcloud_ready: bool,
    /// `ok`, `disabled`, or `not_configured`.
    pub status: String,
    pub message: String,
    /// Nearest-neighbour matches, when the live path succeeds.
    pub matches: Vec<gcloud::VectorMatch>,
}

async fn query(
    State(state): State<AppState>,
    Json(body): Json<QueryBody>,
) -> ApiResult<Json<QueryResponse>> {
    let q = body.q.trim();
    if q.is_empty() {
        return Err(AppError::msg("query text must not be empty"));
    }

    let scope = normalize_scope(body.scope.as_deref());
    let repo_id = if scope == REPO_SCOPE {
        Some(
            body.repo_id
                .ok_or_else(|| AppError::msg("repo_id is required for scope=repo"))?,
        )
    } else {
        None
    };

    let cfg = resolve_effective(&state, repo_id).await?;

    if !cfg.rag_enabled {
        return Ok(Json(QueryResponse {
            gcloud_ready: false,
            status: "disabled".to_string(),
            message: "RAG is disabled for this scope (enable it in knowledge settings)"
                .to_string(),
            matches: Vec::new(),
        }));
    }

    let gstatus = gcloud::status(&state.db).await;
    if !gstatus.ready() {
        return Ok(Json(QueryResponse {
            gcloud_ready: false,
            status: "not_configured".to_string(),
            message: "Google Cloud not configured: install gcloud + run \
                      gcloud auth application-default login"
                .to_string(),
            matches: Vec::new(),
        }));
    }

    // gcloud is detected: embed the query, then run a nearest-neighbour lookup.
    // Both are stubbed (TODO live) and will return a clear not-yet-implemented
    // error; surface it as the response status rather than a 500.
    let embedded = match gcloud::embed(&state.db, cfg.embedding_provider, &[q.to_string()]).await {
        Ok(mut v) if !v.is_empty() => v.remove(0),
        Ok(_) => Vec::new(),
        Err(e) => {
            return Ok(Json(QueryResponse {
                gcloud_ready: true,
                status: "not_configured".to_string(),
                message: e.to_string(),
                matches: Vec::new(),
            }));
        }
    };

    match gcloud::vector_query(&state.db, &embedded).await {
        Ok(matches) => Ok(Json(QueryResponse {
            gcloud_ready: true,
            status: "ok".to_string(),
            message: "ok".to_string(),
            matches,
        })),
        Err(e) => Ok(Json(QueryResponse {
            gcloud_ready: true,
            status: "not_configured".to_string(),
            message: e.to_string(),
            matches: Vec::new(),
        })),
    }
}

/// Expose the effective resolver to sibling modules (e.g. the context budget
/// is consulted when assembling prompts). Kept thin and read-only.
pub async fn effective_config(
    state: &AppState,
    repo_id: Option<i64>,
) -> ApiResult<KnowledgeConfig> {
    resolve_effective(state, repo_id).await
}

/// The configured context budget for a scope, in tokens.
pub async fn context_budget_tokens(state: &AppState, repo_id: Option<i64>) -> ApiResult<u32> {
    Ok(resolve_effective(state, repo_id).await?.context_budget_tokens)
}

/// Convenience accessor: the raw stored value for a layer, useful for the UI's
/// inherit-vs-override diff. Returns `Value::Null` when the layer inherits.
pub async fn layer_value(
    state: &AppState,
    scope: &str,
    repo_id: Option<i64>,
) -> ApiResult<Value> {
    Ok(match read_layer(state, normalize_scope(Some(scope)), repo_id).await? {
        Some(cfg) => serde_json::to_value(cfg)
            .map_err(|e| AppError::msg(format!("failed to serialize knowledge config: {e}")))?,
        None => Value::Null,
    })
}
