use async_trait::async_trait;

use crate::journal::repository::JournalRepository;

use super::types::{
    AnalyzerError, GetRecentRequest, JournalEntryView, MAX_RECENT_LIMIT, MAX_TEXT_SEARCH_LIMIT,
    SearchTextRequest, UserContext,
};

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
}

#[derive(Debug, Clone)]
pub struct DefaultJournalReadService {
    repository: JournalRepository,
}

impl DefaultJournalReadService {
    pub fn new(repository: JournalRepository) -> Self {
        Self { repository }
    }
}

fn validate_limit(limit: u32, max: u32) -> Result<u32, AnalyzerError> {
    if limit == 0 {
        return Err(AnalyzerError::InvalidArgument("limit must be > 0".into()));
    }
    if limit > max {
        return Err(AnalyzerError::LimitTooLarge { max });
    }
    Ok(limit)
}

fn validate_range(
    from: Option<chrono::NaiveDate>,
    to_exclusive: Option<chrono::NaiveDate>,
) -> Result<(), AnalyzerError> {
    if let (Some(from), Some(to)) = (from, to_exclusive)
        && to <= from
    {
        return Err(AnalyzerError::InvalidArgument(
            "to_date_exclusive must be greater than from_date".into(),
        ));
    }
    Ok(())
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
        validate_range(request.from_date, request.to_date_exclusive)?;

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
        validate_range(request.from_date, request.to_date_exclusive)?;
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
        database::register_sqlite_vec_extension();
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        let repository = JournalRepository::new(pool);
        let service = DefaultJournalReadService::new(repository.clone());
        (service, repository)
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
    async fn get_recent_scopes_to_authenticated_user() {
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

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].text, "mine");
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
    async fn search_text_scopes_to_authenticated_user() {
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

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].text, "mine matches");
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
}
