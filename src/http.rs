//! Assembly of the HTTP listener serving the MCP endpoint.
//!
//! Authentication is always on: the bearer token (minted via the Telegram
//! `/token` command) identifies a user, and `/mcp` requests are forwarded to
//! a lazily built router bound to that user's isolated database. Only the
//! `/health` probe is served without authentication.

use std::{collections::HashMap, error::Error, sync::Arc};

use axum::{
    Router,
    extract::{Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::any,
};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use sqlx::SqlitePool;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;
use tracing::error;

use crate::{
    adapters::mcp::AnalyzerMcpServer,
    auth::{AuthenticatedTenant, TokenResolver, require_user_bearer},
    journal::{
        analyzer::{DefaultSemanticJournalSearcher, build_analyzer_mcp_components},
        embedding::{EmbeddingConfig, RigOpenAiEmbedder, SqliteEmbeddingRepository},
        registry::JournalServiceRegistry,
        repository::JournalRepository,
    },
};

type BoxError = Box<dyn Error + Send + Sync>;

/// Shared context for building a tenant's router.
pub struct TenantRouterConfig {
    pub embedding_config: Option<EmbeddingConfig>,
    pub shutdown: CancellationToken,
}

/// Build the protected `/mcp` routes bound to one database.
fn build_tenant_router(pool: &SqlitePool, config: &TenantRouterConfig) -> Result<Router, BoxError> {
    // The caller refuses to start the listener without embedding
    // configuration, so this only guards internal misuse.
    let cfg = config
        .embedding_config
        .as_ref()
        .ok_or("MCP requires embedding configuration")?;

    let embedder = RigOpenAiEmbedder::from_env(cfg.clone())?;
    let semantic = Arc::new(DefaultSemanticJournalSearcher::new(
        SqliteEmbeddingRepository::new(pool.clone()),
        embedder,
        JournalRepository::new(pool.clone()),
    ));

    let components = build_analyzer_mcp_components(pool.clone(), semantic);
    let server = AnalyzerMcpServer::new(components);

    let service = StreamableHttpService::new(
        {
            let server = server.clone();
            move || Ok(server.clone())
        },
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default()
            .disable_allowed_hosts()
            .with_stateful_mode(false)
            .with_cancellation_token(config.shutdown.child_token()),
    );

    Ok(Router::new().nest_service("/mcp", service))
}

/// Lazily built, cached per-tenant routers backed by the registry's isolated
/// databases.
#[derive(Clone)]
pub struct TenantRouters {
    registry: JournalServiceRegistry,
    config: Arc<TenantRouterConfig>,
    cache: Arc<RwLock<HashMap<String, Router>>>,
}

impl TenantRouters {
    pub fn new(registry: JournalServiceRegistry, config: TenantRouterConfig) -> Self {
        Self {
            registry,
            config: Arc::new(config),
            cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    async fn router_for(&self, chat_id: &str) -> Result<Router, BoxError> {
        {
            let guard = self.cache.read().await;
            if let Some(router) = guard.get(chat_id) {
                return Ok(router.clone());
            }
        }

        let pool = self.registry.pool(chat_id).await?;

        let mut guard = self.cache.write().await;
        // Double-check to avoid race condition
        if let Some(router) = guard.get(chat_id) {
            return Ok(router.clone());
        }

        let router = build_tenant_router(&pool, &self.config)?;
        guard.insert(chat_id.to_string(), router.clone());
        Ok(router)
    }
}

/// Forward an authenticated request to the router of the tenant resolved by
/// [`require_user_bearer`].
async fn forward_to_tenant(State(tenants): State<TenantRouters>, request: Request) -> Response {
    let Some(AuthenticatedTenant(chat_id)) = request.extensions().get::<AuthenticatedTenant>()
    else {
        error!("authenticated request reached tenant forwarding without a tenant extension");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let chat_id = chat_id.clone();

    match tenants.router_for(&chat_id).await {
        Ok(router) => match router.oneshot(request).await {
            Ok(response) => response,
            Err(infallible) => match infallible {},
        },
        Err(err) => {
            error!(chat_id = %chat_id, error = %err, "failed to build tenant router");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// Full listener app: `/mcp` is routed to the authenticated user's database;
/// `/health` is public.
pub fn build_per_user_app(tenants: TenantRouters, resolver: Arc<TokenResolver>) -> Router {
    Router::new()
        .route("/mcp", any(forward_to_tenant))
        .route("/mcp/{*path}", any(forward_to_tenant))
        .with_state(tenants)
        .layer(axum::middleware::from_fn_with_state(
            resolver,
            require_user_bearer,
        ))
        .merge(crate::health::router())
}
