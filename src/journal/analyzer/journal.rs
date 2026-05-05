use std::sync::Arc;

use async_trait::async_trait;

use crate::journal::repository::JournalRepository;

use super::semantic::SemanticJournalSearcher;
use super::types::{
    AnalyzerError, GetRecentRequest, JournalEntryView, MAX_RECENT_LIMIT, MAX_SEMANTIC_LIMIT,
    MAX_TEXT_SEARCH_LIMIT, SearchSemanticRequest, SearchTextRequest, SemanticHit, UserContext,
};
use super::validation::{validate_limit, validate_optional_range};

#[async_trait]
pub trait JournalReadService: Send + Sync {
    async fn get_recent(
        &self,
        ctx: &UserContext,
        request: GetRecentRequest,
    ) -> Result<Vec<JournalEntryView>, AnalyzerError>;

    async fn search_text(
        &self,
        ctx: &UserContext,
        request: SearchTextRequest,
    ) -> Result<Vec<JournalEntryView>, AnalyzerError>;

    async fn search_semantic(
        &self,
        ctx: &UserContext,
        request: SearchSemanticRequest,
    ) -> Result<Vec<SemanticHit>, AnalyzerError>;
}

#[derive(Clone)]
pub struct DefaultJournalReadService {
    repository: JournalRepository,
    semantic: Arc<dyn SemanticJournalSearcher>,
}

impl DefaultJournalReadService {
    pub fn new(repository: JournalRepository, semantic: Arc<dyn SemanticJournalSearcher>) -> Self {
        Self {
            repository,
            semantic,
        }
    }
}

fn map_storage_error(err: sqlx::Error) -> AnalyzerError {
    AnalyzerError::Internal(Box::new(err))
}

#[async_trait]
impl JournalReadService for DefaultJournalReadService {
    async fn get_recent(
        &self,
        ctx: &UserContext,
        request: GetRecentRequest,
    ) -> Result<Vec<JournalEntryView>, AnalyzerError> {
        let limit = validate_limit(request.limit, MAX_RECENT_LIMIT)?;
        validate_optional_range(request.from_date, request.to_date_exclusive)?;

        let entries = match (request.from_date, request.to_date_exclusive) {
            (None, None) => self
                .repository
                .fetch_recent(&ctx.user_id, limit)
                .await
                .map_err(map_storage_error)?,
            (from, to) => {
                let from = from.unwrap_or(chrono::NaiveDate::from_ymd_opt(1970, 1, 1).unwrap());
                let to = to.unwrap_or(chrono::NaiveDate::from_ymd_opt(9999, 1, 1).unwrap());
                self.repository
                    .fetch_in_range(&ctx.user_id, from, to, limit)
                    .await
                    .map_err(map_storage_error)?
            }
        };

        Ok(entries.into_iter().map(JournalEntryView::from).collect())
    }

    async fn search_text(
        &self,
        ctx: &UserContext,
        request: SearchTextRequest,
    ) -> Result<Vec<JournalEntryView>, AnalyzerError> {
        let limit = validate_limit(request.limit, MAX_TEXT_SEARCH_LIMIT)?;
        validate_optional_range(request.from_date, request.to_date_exclusive)?;
        let trimmed = request.query.trim();
        if trimmed.is_empty() {
            return Err(AnalyzerError::InvalidArgument(
                "query must not be empty".into(),
            ));
        }

        let entries = self
            .repository
            .search_text(
                &ctx.user_id,
                trimmed,
                request.from_date,
                request.to_date_exclusive,
                limit,
            )
            .await
            .map_err(map_storage_error)?;

        Ok(entries.into_iter().map(JournalEntryView::from).collect())
    }

    async fn search_semantic(
        &self,
        ctx: &UserContext,
        request: SearchSemanticRequest,
    ) -> Result<Vec<SemanticHit>, AnalyzerError> {
        let limit = validate_limit(request.limit, MAX_SEMANTIC_LIMIT)?;
        validate_optional_range(request.from_date, request.to_date_exclusive)?;
        let trimmed = request.query.trim();
        if trimmed.is_empty() {
            return Err(AnalyzerError::InvalidArgument(
                "query must not be empty".into(),
            ));
        }

        let date_filter_active = request.from_date.is_some() || request.to_date_exclusive.is_some();
        let fetch_limit = if date_filter_active {
            MAX_SEMANTIC_LIMIT as usize
        } else {
            limit as usize
        };

        let mut hits = self
            .semantic
            .search(&ctx.user_id, trimmed, fetch_limit)
            .await?;

        if date_filter_active {
            let from = request.from_date;
            let to = request.to_date_exclusive;
            hits.retain(|hit| {
                let date = hit.received_at.date_naive();
                from.is_none_or(|f| date >= f) && to.is_none_or(|t| date < t)
            });
        }

        hits.truncate(limit as usize);
        Ok(hits)
    }
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, NaiveDate, TimeZone, Utc};
    use sqlx::SqlitePool;

    use super::*;
    use crate::{
        database,
        messages::{IncomingMessage, MessageSource},
    };

    async fn setup() -> (DefaultJournalReadService, JournalRepository) {
        setup_with_semantic(Arc::new(StubSemanticSearcher::default())).await
    }

    async fn setup_with_semantic(
        semantic: Arc<dyn SemanticJournalSearcher>,
    ) -> (DefaultJournalReadService, JournalRepository) {
        database::register_sqlite_vec_extension();
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        let repository = JournalRepository::new(pool);
        let service = DefaultJournalReadService::new(repository.clone(), semantic);
        (service, repository)
    }

    #[derive(Default, Clone)]
    struct StubSemanticSearcher {
        hits: std::sync::Arc<std::sync::Mutex<Vec<SemanticHit>>>,
        last_limit: std::sync::Arc<std::sync::Mutex<Option<usize>>>,
    }

    impl StubSemanticSearcher {
        fn with_hits(hits: Vec<SemanticHit>) -> Self {
            Self {
                hits: std::sync::Arc::new(std::sync::Mutex::new(hits)),
                last_limit: std::sync::Arc::new(std::sync::Mutex::new(None)),
            }
        }

        fn last_limit(&self) -> Option<usize> {
            *self.last_limit.lock().unwrap()
        }
    }

    #[async_trait]
    impl SemanticJournalSearcher for StubSemanticSearcher {
        async fn search(
            &self,
            _user_id: &str,
            _query: &str,
            limit: usize,
        ) -> Result<Vec<SemanticHit>, AnalyzerError> {
            *self.last_limit.lock().unwrap() = Some(limit);
            Ok(self.hits.lock().unwrap().clone())
        }
    }

    fn semantic_req(query: &str, limit: u32) -> SearchSemanticRequest {
        SearchSemanticRequest {
            query: query.to_string(),
            limit,
            from_date: None,
            to_date_exclusive: None,
        }
    }

    fn hit(id: i64, received_at: DateTime<Utc>, text: &str, distance: f32) -> SemanticHit {
        SemanticHit {
            id,
            received_at,
            text: text.to_string(),
            distance,
        }
    }

    fn ctx() -> UserContext {
        UserContext::new("user-1")
    }

    fn at(y: i32, m: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, h, mi, 0).unwrap()
    }

    fn message(source_message_id: &str, text: &str, received_at: DateTime<Utc>) -> IncomingMessage {
        IncomingMessage {
            source: MessageSource::Telegram,
            source_conversation_id: "42".to_string(),
            source_message_id: source_message_id.to_string(),
            user_id: "user-1".to_string(),
            text: text.to_string(),
            received_at,
        }
    }

    fn recent(limit: u32) -> GetRecentRequest {
        GetRecentRequest {
            limit,
            from_date: None,
            to_date_exclusive: None,
        }
    }

    fn search(query: &str, limit: u32) -> SearchTextRequest {
        SearchTextRequest {
            query: query.to_string(),
            limit,
            from_date: None,
            to_date_exclusive: None,
        }
    }

    #[tokio::test]
    async fn get_recent_returns_entries_newest_first_with_ids() {
        let (service, repo) = setup().await;
        repo.store(&message("1", "first", at(2026, 4, 28, 10, 0)))
            .await
            .unwrap();
        repo.store(&message("2", "second", at(2026, 4, 28, 11, 0)))
            .await
            .unwrap();

        let result = service.get_recent(&ctx(), recent(10)).await.unwrap();

        assert_eq!(result.len(), 2);
        assert_eq!(result[0].text, "second");
        assert_eq!(result[1].text, "first");
        assert!(result[0].id > 0);
    }

    #[tokio::test]
    async fn get_recent_filters_by_date_range_when_provided() {
        let (service, repo) = setup().await;
        repo.store(&message("1", "before", at(2026, 4, 27, 10, 0)))
            .await
            .unwrap();
        repo.store(&message("2", "in", at(2026, 4, 28, 10, 0)))
            .await
            .unwrap();
        repo.store(&message("3", "after", at(2026, 4, 29, 0, 0)))
            .await
            .unwrap();

        let req = GetRecentRequest {
            limit: 10,
            from_date: Some(NaiveDate::from_ymd_opt(2026, 4, 28).unwrap()),
            to_date_exclusive: Some(NaiveDate::from_ymd_opt(2026, 4, 29).unwrap()),
        };
        let result = service.get_recent(&ctx(), req).await.unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].text, "in");
    }

    #[tokio::test]
    async fn get_recent_supports_open_ended_from_only() {
        let (service, repo) = setup().await;
        repo.store(&message("1", "before", at(2026, 4, 27, 10, 0)))
            .await
            .unwrap();
        repo.store(&message("2", "in", at(2026, 4, 28, 10, 0)))
            .await
            .unwrap();

        let req = GetRecentRequest {
            limit: 10,
            from_date: Some(NaiveDate::from_ymd_opt(2026, 4, 28).unwrap()),
            to_date_exclusive: None,
        };
        let result = service.get_recent(&ctx(), req).await.unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].text, "in");
    }

    #[tokio::test]
    async fn get_recent_rejects_zero_limit() {
        let (service, _) = setup().await;
        let err = service.get_recent(&ctx(), recent(0)).await.unwrap_err();
        assert!(matches!(err, AnalyzerError::InvalidArgument(_)));
    }

    #[tokio::test]
    async fn get_recent_rejects_limit_above_max() {
        let (service, _) = setup().await;
        let err = service
            .get_recent(&ctx(), recent(MAX_RECENT_LIMIT + 1))
            .await
            .unwrap_err();
        assert!(matches!(err, AnalyzerError::LimitTooLarge { .. }));
    }

    #[tokio::test]
    async fn get_recent_rejects_inverted_range() {
        let (service, _) = setup().await;
        let req = GetRecentRequest {
            limit: 10,
            from_date: Some(NaiveDate::from_ymd_opt(2026, 4, 29).unwrap()),
            to_date_exclusive: Some(NaiveDate::from_ymd_opt(2026, 4, 28).unwrap()),
        };
        let err = service.get_recent(&ctx(), req).await.unwrap_err();
        assert!(matches!(err, AnalyzerError::InvalidArgument(_)));
    }

    #[tokio::test]
    async fn get_recent_returns_all_single_user_entries() {
        let (service, repo) = setup().await;
        repo.store(&message("1", "mine", at(2026, 4, 28, 10, 0)))
            .await
            .unwrap();
        repo.store(&IncomingMessage {
            source: MessageSource::Telegram,
            source_conversation_id: "99".to_string(),
            source_message_id: "2".to_string(),
            user_id: "user-2".to_string(),
            text: "theirs".to_string(),
            received_at: at(2026, 4, 28, 11, 0),
        })
        .await
        .unwrap();

        let result = service.get_recent(&ctx(), recent(10)).await.unwrap();

        assert_eq!(result.len(), 2);
        assert_eq!(result[0].text, "theirs");
        assert_eq!(result[1].text, "mine");
    }

    #[tokio::test]
    async fn search_text_returns_matching_entries() {
        let (service, repo) = setup().await;
        repo.store(&message(
            "1",
            "felt anxious before the call",
            at(2026, 4, 28, 10, 0),
        ))
        .await
        .unwrap();
        repo.store(&message("2", "calm afternoon", at(2026, 4, 28, 11, 0)))
            .await
            .unwrap();

        let result = service
            .search_text(&ctx(), search("ANXIOUS", 10))
            .await
            .unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].text, "felt anxious before the call");
    }

    #[tokio::test]
    async fn search_text_trims_whitespace_and_rejects_empty_query() {
        let (service, _) = setup().await;
        let err = service
            .search_text(&ctx(), search("   ", 10))
            .await
            .unwrap_err();
        assert!(matches!(err, AnalyzerError::InvalidArgument(_)));
    }

    #[tokio::test]
    async fn search_text_rejects_limit_above_max() {
        let (service, _) = setup().await;
        let err = service
            .search_text(&ctx(), search("x", MAX_TEXT_SEARCH_LIMIT + 1))
            .await
            .unwrap_err();
        assert!(matches!(err, AnalyzerError::LimitTooLarge { .. }));
    }

    #[tokio::test]
    async fn search_text_returns_all_single_user_matches() {
        let (service, repo) = setup().await;
        repo.store(&message("1", "mine matches", at(2026, 4, 28, 10, 0)))
            .await
            .unwrap();
        repo.store(&IncomingMessage {
            source: MessageSource::Telegram,
            source_conversation_id: "99".to_string(),
            source_message_id: "2".to_string(),
            user_id: "user-2".to_string(),
            text: "theirs matches".to_string(),
            received_at: at(2026, 4, 28, 11, 0),
        })
        .await
        .unwrap();

        let result = service
            .search_text(&ctx(), search("matches", 10))
            .await
            .unwrap();

        assert_eq!(result.len(), 2);
        assert_eq!(result[0].text, "theirs matches");
        assert_eq!(result[1].text, "mine matches");
    }

    #[tokio::test]
    async fn search_text_applies_date_filter() {
        let (service, repo) = setup().await;
        repo.store(&message("1", "match before", at(2026, 4, 27, 10, 0)))
            .await
            .unwrap();
        repo.store(&message("2", "match within", at(2026, 4, 28, 10, 0)))
            .await
            .unwrap();

        let req = SearchTextRequest {
            query: "match".to_string(),
            limit: 10,
            from_date: Some(NaiveDate::from_ymd_opt(2026, 4, 28).unwrap()),
            to_date_exclusive: Some(NaiveDate::from_ymd_opt(2026, 4, 29).unwrap()),
        };
        let result = service.search_text(&ctx(), req).await.unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].text, "match within");
    }

    #[tokio::test]
    async fn search_semantic_returns_hits_from_underlying_searcher() {
        let stub = StubSemanticSearcher::with_hits(vec![hit(
            1,
            at(2026, 4, 28, 10, 0),
            "anxious entry",
            0.1,
        )]);
        let (service, _) = setup_with_semantic(Arc::new(stub.clone())).await;

        let result = service
            .search_semantic(&ctx(), semantic_req("anxiety", 5))
            .await
            .unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].text, "anxious entry");
        assert_eq!(stub.last_limit(), Some(5));
    }

    #[tokio::test]
    async fn search_semantic_rejects_zero_limit() {
        let (service, _) = setup().await;
        let err = service
            .search_semantic(&ctx(), semantic_req("x", 0))
            .await
            .unwrap_err();
        assert!(matches!(err, AnalyzerError::InvalidArgument(_)));
    }

    #[tokio::test]
    async fn search_semantic_rejects_limit_above_max() {
        let (service, _) = setup().await;
        let err = service
            .search_semantic(&ctx(), semantic_req("x", MAX_SEMANTIC_LIMIT + 1))
            .await
            .unwrap_err();
        assert!(matches!(err, AnalyzerError::LimitTooLarge { .. }));
    }

    #[tokio::test]
    async fn search_semantic_rejects_blank_query() {
        let (service, _) = setup().await;
        let err = service
            .search_semantic(&ctx(), semantic_req("   ", 5))
            .await
            .unwrap_err();
        assert!(matches!(err, AnalyzerError::InvalidArgument(_)));
    }

    #[tokio::test]
    async fn search_semantic_rejects_inverted_range() {
        let (service, _) = setup().await;
        let req = SearchSemanticRequest {
            query: "x".to_string(),
            limit: 5,
            from_date: Some(NaiveDate::from_ymd_opt(2026, 4, 29).unwrap()),
            to_date_exclusive: Some(NaiveDate::from_ymd_opt(2026, 4, 28).unwrap()),
        };
        let err = service.search_semantic(&ctx(), req).await.unwrap_err();
        assert!(matches!(err, AnalyzerError::InvalidArgument(_)));
    }

    #[tokio::test]
    async fn search_semantic_filters_by_date_and_truncates_to_limit() {
        let stub = StubSemanticSearcher::with_hits(vec![
            hit(1, at(2026, 4, 27, 10, 0), "before", 0.1),
            hit(2, at(2026, 4, 28, 10, 0), "in-1", 0.2),
            hit(3, at(2026, 4, 28, 12, 0), "in-2", 0.3),
            hit(4, at(2026, 4, 29, 0, 0), "boundary", 0.4),
        ]);
        let (service, _) = setup_with_semantic(Arc::new(stub.clone())).await;

        let req = SearchSemanticRequest {
            query: "x".to_string(),
            limit: 1,
            from_date: Some(NaiveDate::from_ymd_opt(2026, 4, 28).unwrap()),
            to_date_exclusive: Some(NaiveDate::from_ymd_opt(2026, 4, 29).unwrap()),
        };
        let result = service.search_semantic(&ctx(), req).await.unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].text, "in-1");
        assert_eq!(stub.last_limit(), Some(MAX_SEMANTIC_LIMIT as usize));
    }

    #[tokio::test]
    async fn search_semantic_does_not_oversample_when_no_date_filter() {
        let stub = StubSemanticSearcher::with_hits(vec![]);
        let (service, _) = setup_with_semantic(Arc::new(stub.clone())).await;

        let _ = service
            .search_semantic(&ctx(), semantic_req("x", 3))
            .await
            .unwrap();

        assert_eq!(stub.last_limit(), Some(3));
    }
}
