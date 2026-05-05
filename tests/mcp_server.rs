//! End-to-end test that mounts the MCP HTTP server and drives it with the
//! rmcp streamable-HTTP client. A stub `SemanticJournalSearcher` keeps the
//! test self-contained — no OpenAI credentials are required.

use std::sync::Arc;

use async_trait::async_trait;
use rmcp::{
    ServiceExt,
    model::CallToolRequestParams,
    transport::{
        StreamableHttpClientTransport, StreamableHttpServerConfig,
        streamable_http_server::{
            session::local::LocalSessionManager, tower::StreamableHttpService,
        },
    },
};
use serde_json::json;
use sqlx::SqlitePool;
use tokio_util::sync::CancellationToken;

use froid::{
    adapters::mcp::AnalyzerMcpServer,
    database,
    journal::analyzer::{
        SemanticJournalSearcher, UserContext, build_analyzer_tool_registry,
        types::{AnalyzerError, SemanticHit},
    },
};

struct StubSemanticSearcher;

#[async_trait]
impl SemanticJournalSearcher for StubSemanticSearcher {
    async fn search(
        &self,
        _user_id: &str,
        _query: &str,
        _limit: usize,
    ) -> Result<Vec<SemanticHit>, AnalyzerError> {
        Ok(Vec::new())
    }
}

async fn fresh_pool() -> SqlitePool {
    database::register_sqlite_vec_extension();
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::migrate!().run(&pool).await.unwrap();
    pool
}

#[tokio::test]
async fn lists_and_calls_analyzer_tools_over_streamable_http() {
    let pool = fresh_pool().await;
    let registry = build_analyzer_tool_registry(pool, Arc::new(StubSemanticSearcher));
    let server = AnalyzerMcpServer::new(registry, UserContext::new("test-user"));

    let cancel = CancellationToken::new();
    let service = StreamableHttpService::new(
        {
            let server = server.clone();
            move || Ok(server.clone())
        },
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default()
            .with_sse_keep_alive(None)
            .with_cancellation_token(cancel.child_token()),
    );

    let router = axum::Router::new().nest_service("/mcp", service);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server_handle = tokio::spawn({
        let cancel = cancel.clone();
        async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    cancel.cancelled().await;
                })
                .await
                .unwrap();
        }
    });

    let transport = StreamableHttpClientTransport::from_uri(format!("http://{addr}/mcp"));
    let client = ().serve(transport).await.expect("client should connect");

    let tools = client.list_all_tools().await.expect("list tools ok");
    let mut names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
    names.sort();
    let mut expected = [
        "journal_get_recent",
        "journal_search_text",
        "journal_search_semantic",
        "daily_review_get_range",
        "weekly_review_get_range",
        "signals_search",
    ];
    expected.sort();
    assert_eq!(names, expected);

    let mut args = serde_json::Map::new();
    args.insert("limit".to_string(), json!(5));
    let result = client
        .call_tool(CallToolRequestParams::new("journal_get_recent").with_arguments(args))
        .await
        .expect("call_tool ok");
    assert_ne!(result.is_error, Some(true));
    let structured = result
        .structured_content
        .expect("journal_get_recent returns structured content");
    assert!(structured["entries"].is_array());

    client.cancel().await.expect("client cancel");
    cancel.cancel();
    let _ = server_handle.await;
}
