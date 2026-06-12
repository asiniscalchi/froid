use std::sync::Arc;

use sqlx::SqlitePool;

use crate::journal::repository::JournalRepository;
use crate::journal::review::repository::DailyReviewRepository;
use crate::journal::review::signals::repository::DailyReviewSignalRepository;
use crate::journal::week_review::repository::WeeklyReviewRepository;

use super::journal::{DefaultJournalReadService, JournalReadService};
use super::review::{DefaultReviewReadService, ReviewReadService};
use super::semantic::SemanticJournalSearcher;
use super::signal::{DefaultSignalReadService, SignalReadService};
use super::tools::{
    ToolRegistry,
    journal::{JournalGetRecentTool, JournalSearchSemanticTool, JournalSearchTextTool},
    review::{DailyReviewGetRangeTool, WeeklyReviewGetRangeTool},
    signal::SignalsSearchTool,
};

/// Bundle of read services and tool registry used to drive the MCP adapter.
///
/// The MCP adapter needs the tool registry to serve `tools/*` requests and the
/// journal / review services to serve `resources/*` requests, so they are
/// constructed together and exposed as a single value.
#[derive(Clone)]
pub struct AnalyzerMcpComponents {
    pub registry: Arc<ToolRegistry>,
    pub journal_service: Arc<dyn JournalReadService>,
    pub review_service: Arc<dyn ReviewReadService>,
}

/// Build the analyzer tool registry and the underlying read services backed
/// by `pool`. The semantic journal searcher is injected so the caller controls
/// how the embedder is constructed (and so tests can supply a stub without
/// needing an OpenAI API key).
pub fn build_analyzer_mcp_components(
    pool: SqlitePool,
    semantic: Arc<dyn SemanticJournalSearcher>,
) -> AnalyzerMcpComponents {
    let journal_repo = JournalRepository::new(pool.clone());
    let daily_repo = DailyReviewRepository::new(pool.clone());
    let weekly_repo = WeeklyReviewRepository::new(pool.clone());
    let signal_repo = DailyReviewSignalRepository::new(pool);

    let journal_service: Arc<dyn JournalReadService> =
        Arc::new(DefaultJournalReadService::new(journal_repo, semantic));
    let review_service: Arc<dyn ReviewReadService> =
        Arc::new(DefaultReviewReadService::new(daily_repo, weekly_repo));
    let signal_service: Arc<dyn SignalReadService> =
        Arc::new(DefaultSignalReadService::new(signal_repo));

    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(JournalGetRecentTool::new(journal_service.clone())));
    registry.register(Arc::new(JournalSearchTextTool::new(
        journal_service.clone(),
    )));
    registry.register(Arc::new(JournalSearchSemanticTool::new(
        journal_service.clone(),
    )));
    registry.register(Arc::new(DailyReviewGetRangeTool::new(
        review_service.clone(),
    )));
    registry.register(Arc::new(WeeklyReviewGetRangeTool::new(
        review_service.clone(),
    )));
    registry.register(Arc::new(SignalsSearchTool::new(signal_service)));

    AnalyzerMcpComponents {
        registry: Arc::new(registry),
        journal_service,
        review_service,
    }
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use sqlx::SqlitePool;

    use super::*;
    use crate::database;
    use crate::journal::analyzer::types::{AnalyzerError, SemanticHit};

    struct StubSemanticSearcher;

    #[async_trait]
    impl SemanticJournalSearcher for StubSemanticSearcher {
        async fn search(
            &self,
            _query: &str,
            _from_date: Option<chrono::NaiveDate>,
            _to_date_exclusive: Option<chrono::NaiveDate>,
            _limit: usize,
        ) -> Result<Vec<SemanticHit>, AnalyzerError> {
            Ok(Vec::new())
        }
    }

    async fn pool() -> SqlitePool {
        database::register_sqlite_vec_extension();
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        pool
    }

    #[tokio::test]
    async fn registers_every_analyzer_tool() {
        let pool = pool().await;
        let components = build_analyzer_mcp_components(pool, Arc::new(StubSemanticSearcher));
        let registry = components.registry;

        let names: Vec<&str> = registry.tools().iter().map(|t| t.name()).collect();

        assert_eq!(names.len(), 6);
        for expected in [
            "journal_get_recent",
            "journal_search_text",
            "journal_search_semantic",
            "daily_review_get_range",
            "weekly_review_get_range",
            "signals_search",
        ] {
            assert!(
                names.contains(&expected),
                "missing tool {expected} in {names:?}"
            );
        }
    }

    #[tokio::test]
    async fn registered_tools_are_dispatchable_by_name() {
        let pool = pool().await;
        let components = build_analyzer_mcp_components(pool, Arc::new(StubSemanticSearcher));
        let result = components
            .registry
            .dispatch("journal_get_recent", serde_json::json!({"limit": 5}))
            .await
            .unwrap();
        assert!(result["entries"].is_array());
    }
}
