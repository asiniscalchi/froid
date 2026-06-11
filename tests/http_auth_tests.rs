//! Integration tests for the MCP listener app: bearer tokens minted via the
//! Telegram /token flow gate `/mcp`, while `/health` stays public.
//!
//! Database-level tenant isolation is covered by the auth middleware tests
//! (token → chat id) and the transfer tests (per-tenant journals); here we
//! exercise the assembled listener: the auth layer runs before any tenant
//! routing, and only valid tokens get past it.

use std::sync::Arc;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header::AUTHORIZATION},
};
use clap::Parser;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

use froid::{
    auth::TokenResolver,
    cli::Cli,
    http::{TenantRouterConfig, TenantRouters, build_per_user_app},
    journal::{
        embedding::EmbeddingConfig,
        extraction::JournalEntryExtractionRuntimeConfig,
        registry::{JournalServiceRegistry, JournalServiceRegistryConfig},
        review::DailyReviewRuntimeConfig,
        review::signals::wiring::DailyReviewSignalRuntimeConfig,
        week_review::WeeklyReviewRuntimeConfig,
    },
    tokens::{TokenIssuer, UserTokenStore},
};

async fn registry(temp_base_dir: &std::path::Path) -> JournalServiceRegistry {
    let cli = Cli::try_parse_from([
        "froid",
        "--telegram-bot-token",
        "mock_telegram_token_123",
        "--data-dir",
        temp_base_dir.to_str().unwrap(),
    ])
    .unwrap();

    JournalServiceRegistry::new(JournalServiceRegistryConfig {
        config: cli.serve_config().unwrap(),
        embedding_config: None,
        entry_extraction_config: JournalEntryExtractionRuntimeConfig::from_env(),
        daily_review_config: DailyReviewRuntimeConfig::from_env(),
        weekly_review_config: WeeklyReviewRuntimeConfig::from_env(),
        signal_runtime_config: DailyReviewSignalRuntimeConfig::from_env(),
        delivery_configured: false,
        shutdown: CancellationToken::new(),
    })
    .with_base_dir(temp_base_dir.to_path_buf())
}

async fn central_store() -> UserTokenStore {
    froid::database::register_sqlite_vec_extension();
    let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::migrate!().run(&pool).await.unwrap();
    UserTokenStore::new(pool)
}

/// Listener app plus a token minted for user "111", mirroring the wiring in
/// `app::spawn_http_server`.
async fn mcp_app() -> (Router, String) {
    let test_id = ulid::Ulid::new().to_string();
    let temp_base_dir = std::env::temp_dir().join(format!("froid_test_http_{test_id}"));
    tokio::fs::create_dir_all(&temp_base_dir).await.unwrap();

    let registry = registry(&temp_base_dir).await;

    let store = central_store().await;
    let token = TokenIssuer::new(store.clone()).issue("111").await.unwrap();

    let tenants = TenantRouters::new(
        registry,
        TenantRouterConfig {
            embedding_config: Some(EmbeddingConfig::default()),
            shutdown: CancellationToken::new(),
        },
    );
    let app = build_per_user_app(tenants, Arc::new(TokenResolver::new(store)));
    (app, token)
}

async fn get(app: Router, uri: &str, token: Option<&str>) -> (StatusCode, String) {
    let mut request = Request::builder().uri(uri);
    if let Some(token) = token {
        request = request.header(AUTHORIZATION, format!("Bearer {token}"));
    }
    let response = app
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (status, String::from_utf8_lossy(&body).to_string())
}

#[tokio::test]
async fn mcp_rejects_missing_and_unknown_tokens() {
    let (app, _) = mcp_app().await;

    let (status, _) = get(app.clone(), "/mcp", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, _) = get(app, "/mcp", Some("froid_not_real")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn mcp_lets_minted_tokens_past_the_auth_layer() {
    let (app, token) = mcp_app().await;

    // The request reaches the MCP transport (which answers with a non-auth
    // status for a bodyless GET) instead of being rejected by the auth layer.
    let (status, _) = get(app, "/mcp", Some(&token)).await;
    assert_ne!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn revoked_token_stops_passing_the_auth_layer() {
    let test_id = ulid::Ulid::new().to_string();
    let temp_base_dir = std::env::temp_dir().join(format!("froid_test_revoke_{test_id}"));
    tokio::fs::create_dir_all(&temp_base_dir).await.unwrap();
    let registry = registry(&temp_base_dir).await;

    let store = central_store().await;
    let issuer = TokenIssuer::new(store.clone());
    let token = issuer.issue("111").await.unwrap();
    issuer.revoke("111").await.unwrap();

    let tenants = TenantRouters::new(
        registry,
        TenantRouterConfig {
            embedding_config: Some(EmbeddingConfig::default()),
            shutdown: CancellationToken::new(),
        },
    );
    let app = build_per_user_app(tenants, Arc::new(TokenResolver::new(store)));

    let (status, _) = get(app, "/mcp", Some(&token)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn health_stays_public() {
    let (app, _) = mcp_app().await;

    let (status, body) = get(app, "/health", None).await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("\"ok\""));
}
