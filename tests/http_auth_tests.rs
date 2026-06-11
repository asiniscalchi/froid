//! Integration tests for the tenant-aware HTTP listener: bearer tokens
//! minted via the Telegram /token flow route `/api` requests to isolated
//! databases, while `/health` and the SPA shell stay public.

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
    auth::TokenResolver,
    cli::Cli,
    http::{TenantRouterConfig, TenantRouters, build_per_user_app},
    journal::{
        extraction::JournalEntryExtractionRuntimeConfig,
        registry::{JournalServiceRegistry, JournalServiceRegistryConfig},
        repository::JournalRepository,
        review::DailyReviewRuntimeConfig,
        review::signals::wiring::DailyReviewSignalRuntimeConfig,
        week_review::WeeklyReviewRuntimeConfig,
    },
    messages::{IncomingMessage, MessageSource},
    tokens::{TokenIssuer, UserTokenStore},
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

async fn registry(temp_base_dir: &std::path::Path) -> JournalServiceRegistry {
    let cli = Cli::try_parse_from([
        "froid",
        "--telegram-bot-token",
        "mock_telegram_token_123",
        "--data-dir",
        temp_base_dir.to_str().unwrap(),
        "--dashboard-enabled",
        "true",
    ])
    .unwrap();
    let config = cli.serve_config().unwrap();

    JournalServiceRegistry::new(JournalServiceRegistryConfig {
        config,
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

/// App with two users ("111" with one seeded entry, "222" empty) and their
/// minted tokens, mirroring the per-user wiring in `app::spawn_http_server`.
async fn per_user_app() -> (Router, String, String) {
    let test_id = ulid::Ulid::new().to_string();
    let temp_base_dir = std::env::temp_dir().join(format!("froid_test_http_{test_id}"));
    tokio::fs::create_dir_all(&temp_base_dir).await.unwrap();

    let registry = registry(&temp_base_dir).await;

    // Seed one entry in Alice's isolated database.
    let pool = registry.pool("111").await.unwrap();
    JournalRepository::new(pool)
        .store(&message("111", "m1", "alice secret note"))
        .await
        .unwrap();

    let store = central_store().await;
    let issuer = TokenIssuer::new(store.clone());
    let alice = issuer.issue("111").await.unwrap();
    let bob = issuer.issue("222").await.unwrap();

    let tenants = TenantRouters::new(
        registry,
        TenantRouterConfig {
            mcp_enabled: false,
            dashboard_enabled: true,
            embedding_config: None,
            shutdown: CancellationToken::new(),
        },
    );
    let app = build_per_user_app(tenants, Arc::new(TokenResolver::new(store)), true);
    (app, alice, bob)
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
async fn minted_tokens_route_to_isolated_databases() {
    let (app, alice, bob) = per_user_app().await;

    let (status, body) = get(app.clone(), "/api/messages/export", Some(&alice)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("alice secret note"),
        "alice should see her entry, got: {body}"
    );

    let (status, body) = get(app, "/api/messages/export", Some(&bob)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !body.contains("alice secret note"),
        "bob must not see alice's entry, got: {body}"
    );
}

#[tokio::test]
async fn per_user_app_rejects_missing_and_unknown_tokens() {
    let (app, _, _) = per_user_app().await;

    let (status, _) = get(app.clone(), "/api/messages/export", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, _) = get(app, "/api/messages/export", Some("froid_not_real")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn rotated_token_loses_access_and_new_token_keeps_it() {
    let test_id = ulid::Ulid::new().to_string();
    let temp_base_dir = std::env::temp_dir().join(format!("froid_test_rotate_{test_id}"));
    tokio::fs::create_dir_all(&temp_base_dir).await.unwrap();
    let registry = registry(&temp_base_dir).await;
    registry.pool("111").await.unwrap();

    let store = central_store().await;
    let issuer = TokenIssuer::new(store.clone());
    let old = issuer.issue("111").await.unwrap();
    let new = issuer.issue("111").await.unwrap();

    let tenants = TenantRouters::new(
        registry,
        TenantRouterConfig {
            mcp_enabled: false,
            dashboard_enabled: true,
            embedding_config: None,
            shutdown: CancellationToken::new(),
        },
    );
    let app = build_per_user_app(tenants, Arc::new(TokenResolver::new(store)), true);

    let (status, _) = get(app.clone(), "/api/messages/export", Some(&old)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, _) = get(app, "/api/messages/export", Some(&new)).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn per_user_app_serves_health_and_spa_without_token() {
    let (app, _, _) = per_user_app().await;

    let (status, body) = get(app.clone(), "/health", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("\"ok\""));

    let (status, body) = get(app, "/", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<div id=\"root\">"));
}
