//! Integration tests for the tenant-aware HTTP listener: per-user bearer
//! tokens route `/api` requests to isolated databases, while `/health` and
//! the SPA shell stay public.

use std::sync::Arc;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header::AUTHORIZATION},
};
use chrono::Utc;
use clap::Parser;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

use froid::{
    auth::{TokenResolver, UserTokens},
    cli::Cli,
    http::{TenantRouterConfig, TenantRouters, build_per_user_app, build_single_tenant_app},
    journal::{
        extraction::JournalEntryExtractionRuntimeConfig,
        registry::{JournalServiceRegistry, JournalServiceRegistryConfig},
        repository::JournalRepository,
        review::DailyReviewRuntimeConfig,
        review::signals::wiring::DailyReviewSignalRuntimeConfig,
        week_review::WeeklyReviewRuntimeConfig,
    },
    messages::{IncomingMessage, MessageSource},
};

fn message(chat_id: &str, message_id: &str, text: &str) -> IncomingMessage {
    IncomingMessage {
        source: MessageSource::Telegram,
        source_conversation_id: chat_id.to_string(),
        source_message_id: message_id.to_string(),
        user_id: chat_id.to_string(),
        text: text.to_string(),
        received_at: Utc::now(),
    }
}

async fn per_user_app() -> Router {
    let test_id = ulid::Ulid::new().to_string();
    let temp_base_dir = std::env::temp_dir().join(format!("froid_test_http_{test_id}"));
    tokio::fs::create_dir_all(&temp_base_dir).await.unwrap();

    let cli = Cli::try_parse_from([
        "froid",
        "--telegram-bot-token",
        "mock_telegram_token_123",
        "--data-dir",
        temp_base_dir.to_str().unwrap(),
        "--dashboard-enabled",
        "true",
        "--auth-tokens",
        "111:alice-secret,222:bob-secret",
    ])
    .unwrap();
    let config = cli.serve_config().unwrap();
    let user_tokens = config.http_auth.user_tokens.clone();

    let shutdown = CancellationToken::new();
    let registry = JournalServiceRegistry::new(JournalServiceRegistryConfig {
        config,
        embedding_config: None,
        entry_extraction_config: JournalEntryExtractionRuntimeConfig::from_env(),
        daily_review_config: DailyReviewRuntimeConfig::from_env(),
        weekly_review_config: WeeklyReviewRuntimeConfig::from_env(),
        signal_runtime_config: DailyReviewSignalRuntimeConfig::from_env(),
        delivery_configured: false,
        shutdown: shutdown.clone(),
    })
    .with_base_dir(temp_base_dir);

    // Seed one entry in Alice's isolated database.
    let pool = registry.pool("111").await.unwrap();
    JournalRepository::new(pool)
        .store(&message("111", "m1", "alice secret note"))
        .await
        .unwrap();

    let tenants = TenantRouters::new(
        registry,
        TenantRouterConfig {
            mcp_enabled: false,
            dashboard_enabled: true,
            embedding_config: None,
            shutdown,
        },
    );
    build_per_user_app(
        tenants,
        Arc::new(TokenResolver::new(UserTokens::new(user_tokens), None)),
        true,
    )
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
async fn telegram_issued_token_routes_to_its_tenant_database() {
    let test_id = ulid::Ulid::new().to_string();
    let temp_base_dir = std::env::temp_dir().join(format!("froid_test_issued_{test_id}"));
    tokio::fs::create_dir_all(&temp_base_dir).await.unwrap();

    let cli = Cli::try_parse_from([
        "froid",
        "--telegram-bot-token",
        "mock_telegram_token_123",
        "--data-dir",
        temp_base_dir.to_str().unwrap(),
        "--dashboard-enabled",
        "true",
        "--auth-dynamic-tokens",
        "true",
    ])
    .unwrap();
    let config = cli.serve_config().unwrap();
    assert!(config.http_auth.dynamic_tokens);

    let shutdown = CancellationToken::new();
    let registry = JournalServiceRegistry::new(JournalServiceRegistryConfig {
        config,
        embedding_config: None,
        entry_extraction_config: JournalEntryExtractionRuntimeConfig::from_env(),
        daily_review_config: DailyReviewRuntimeConfig::from_env(),
        weekly_review_config: WeeklyReviewRuntimeConfig::from_env(),
        signal_runtime_config: DailyReviewSignalRuntimeConfig::from_env(),
        delivery_configured: false,
        shutdown: shutdown.clone(),
    })
    .with_base_dir(temp_base_dir);

    // Seed one entry in the user's isolated database.
    let pool = registry.pool("555").await.unwrap();
    JournalRepository::new(pool)
        .store(&message("555", "m1", "issued token note"))
        .await
        .unwrap();

    // Central store, as wired in app::serve when dynamic tokens are enabled.
    froid::database::register_sqlite_vec_extension();
    let central = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::migrate!().run(&central).await.unwrap();
    let store = froid::tokens::UserTokenStore::new(central);

    // The user mints a token via Telegram.
    let token = froid::tokens::TokenIssuer::new(store.clone())
        .issue("555")
        .await
        .unwrap();

    let tenants = TenantRouters::new(
        registry,
        TenantRouterConfig {
            mcp_enabled: false,
            dashboard_enabled: true,
            embedding_config: None,
            shutdown,
        },
    );
    let app = build_per_user_app(
        tenants,
        Arc::new(TokenResolver::new(UserTokens::new(Vec::new()), Some(store))),
        true,
    );

    let (status, body) = get(app.clone(), "/api/messages/export", Some(&token)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("issued token note"),
        "issued token should reach its own journal, got: {body}"
    );

    let (status, _) = get(app, "/api/messages/export", Some("froid_not_a_real_token")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn per_user_tokens_route_to_isolated_databases() {
    let app = per_user_app().await;

    let (status, body) = get(app.clone(), "/api/messages/export", Some("alice-secret")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("alice secret note"),
        "alice should see her entry, got: {body}"
    );

    let (status, body) = get(app, "/api/messages/export", Some("bob-secret")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !body.contains("alice secret note"),
        "bob must not see alice's entry, got: {body}"
    );
}

#[tokio::test]
async fn per_user_app_rejects_missing_and_unknown_tokens() {
    let app = per_user_app().await;

    let (status, _) = get(app.clone(), "/api/messages/export", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, _) = get(app, "/api/messages/export", Some("mallory")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn per_user_app_serves_health_and_spa_without_token() {
    let app = per_user_app().await;

    let (status, body) = get(app.clone(), "/health", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("\"ok\""));

    let (status, body) = get(app, "/", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<div id=\"root\">"));
}

#[tokio::test]
async fn single_tenant_app_protects_api_and_serves_spa_publicly() {
    use froid::http::build_tenant_router;

    froid::database::register_sqlite_vec_extension();
    let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::migrate!().run(&pool).await.unwrap();

    let tenant_router = build_tenant_router(
        &pool,
        "default",
        &TenantRouterConfig {
            mcp_enabled: false,
            dashboard_enabled: true,
            embedding_config: None,
            shutdown: CancellationToken::new(),
        },
    )
    .unwrap();
    let app = build_single_tenant_app(tenant_router, Some(Arc::from("secret")), true);

    let (status, _) = get(app.clone(), "/api/messages/export", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, _) = get(app.clone(), "/api/messages/export", Some("secret")).await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = get(app.clone(), "/health", None).await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = get(app, "/", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<div id=\"root\">"));
}
