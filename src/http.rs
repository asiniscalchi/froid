//! Assembly of the shared HTTP listener (MCP endpoint + dashboard).
//!
//! Two layouts exist, selected by the auth configuration:
//!
//! - **Single tenant** (`FROID_AUTH_TOKEN` or no auth): every request is
//!   served from one fixed database, optionally behind one bearer token.
//! - **Per user** (`FROID_AUTH_TOKENS`): the bearer token identifies a user
//!   (Telegram chat id) and `/mcp` + `/api` requests are forwarded to a
//!   lazily built router bound to that user's isolated database.
//!
//! In both layouts the `/health` probe and the static SPA shell are served
//! without authentication; only the data-bearing routes sit behind tokens.

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
    auth::{AuthenticatedTenant, TokenResolver, require_bearer, require_user_bearer},
    dashboard,
    journal::{
        analyzer::{DefaultSemanticJournalSearcher, UserContext, build_analyzer_mcp_components},
        embedding::{EmbeddingConfig, RigOpenAiEmbedder, SqliteEmbeddingRepository},
        registry::JournalServiceRegistry,
        repository::JournalRepository,
    },
};

type BoxError = Box<dyn Error + Send + Sync>;

/// Feature flags and shared context for building a tenant's router.
pub struct TenantRouterConfig {
    pub mcp_enabled: bool,
    pub dashboard_enabled: bool,
    pub embedding_config: Option<EmbeddingConfig>,
    pub shutdown: CancellationToken,
}

/// Build the protected routes (`/mcp`, `/api`) bound to one database.
/// `capture_conversation_id` is the conversation web-captured entries are
/// filed under (the owning user's chat id).
pub fn build_tenant_router(
    pool: &SqlitePool,
    capture_conversation_id: &str,
    config: &TenantRouterConfig,
) -> Result<Router, BoxError> {
    let mut router = Router::new();

    if config.mcp_enabled {
        // The caller refuses to start the listener when MCP is enabled
        // without embedding configuration, so this only guards internal misuse.
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
        let user = UserContext::new(crate::messages::SINGLE_USER_ID);
        let server = AnalyzerMcpServer::new(components, user);

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

        router = router.nest_service("/mcp", service);
    }

    if config.dashboard_enabled {
        router = router.merge(dashboard::api_router(pool, capture_conversation_id));
    }

    Ok(router)
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

        let router = build_tenant_router(&pool, chat_id, &self.config)?;
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

/// Full listener app for per-user token auth: `/mcp` and `/api` are routed to
/// the authenticated user's database; `/health` and the SPA shell are public.
pub fn build_per_user_app(
    tenants: TenantRouters,
    resolver: Arc<TokenResolver>,
    dashboard_enabled: bool,
) -> Router {
    let mut router = Router::new()
        .route("/mcp", any(forward_to_tenant))
        .route("/mcp/{*path}", any(forward_to_tenant))
        .route("/api/{*path}", any(forward_to_tenant))
        .with_state(tenants)
        .layer(axum::middleware::from_fn_with_state(
            resolver,
            require_user_bearer,
        ))
        .merge(crate::health::router());

    if dashboard_enabled {
        router = router.merge(dashboard::spa_router());
    }
    router
}

/// Full listener app for the single-tenant layouts: the tenant router sits
/// behind the optional shared token; `/health` and the SPA shell are public.
pub fn build_single_tenant_app(
    tenant_router: Router,
    token: Option<Arc<str>>,
    dashboard_enabled: bool,
) -> Router {
    let mut router = tenant_router;

    if let Some(token) = token {
        router = router.layer(axum::middleware::from_fn_with_state(token, require_bearer));
    }
    router = router.merge(crate::health::router());

    if dashboard_enabled {
        router = router.merge(dashboard::spa_router());
    }
    router
}
