use std::{error::Error, fmt, sync::Arc};

use chrono::NaiveDate;

use crate::journal::{
    extraction::repository::JournalEntryExtractionRepository,
    repository::JournalRepository,
    review::{
        DailyReview, DailyReviewFailure, DailyReviewResult, DailyReviewStatus,
        JournalEntryWithExtraction,
        generator::ReviewGenerator,
        repository::{DailyReviewRepository, DailyReviewRepositoryError},
    },
};

const EMPTY_REVIEW_ERROR: &str = "daily review generator returned an empty review";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DailyReviewServiceError {
    Storage(String),
}

impl fmt::Display for DailyReviewServiceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Storage(message) => write!(f, "{message}"),
        }
    }
}

impl Error for DailyReviewServiceError {}

impl From<sqlx::Error> for DailyReviewServiceError {
    fn from(error: sqlx::Error) -> Self {
        Self::Storage(error.to_string())
    }
}

impl From<DailyReviewRepositoryError> for DailyReviewServiceError {
    fn from(error: DailyReviewRepositoryError) -> Self {
        Self::Storage(error.to_string())
    }
}

impl From<crate::journal::extraction::repository::JournalEntryExtractionRepositoryError>
    for DailyReviewServiceError
{
    fn from(
        error: crate::journal::extraction::repository::JournalEntryExtractionRepositoryError,
    ) -> Self {
        Self::Storage(error.to_string())
    }
}

#[derive(Clone)]
pub struct DailyReviewService {
    daily_reviews: DailyReviewRepository,
    journal_entries: JournalRepository,
    extractions: JournalEntryExtractionRepository,
    generator: Arc<dyn ReviewGenerator>,
}

#[async_trait::async_trait]
pub trait DailyReviewRunner: Send + Sync {
    async fn review_day(
        &self,
        user_id: &str,
        utc_date: NaiveDate,
    ) -> Result<DailyReviewResult, DailyReviewServiceError>;

    async fn fetch_review(
        &self,
        user_id: &str,
        utc_date: NaiveDate,
    ) -> Result<Option<DailyReview>, DailyReviewServiceError>;
}

impl DailyReviewService {
    pub fn new<G>(
        daily_reviews: DailyReviewRepository,
        journal_entries: JournalRepository,
        extractions: JournalEntryExtractionRepository,
        generator: G,
    ) -> Self
    where
        G: ReviewGenerator + 'static,
    {
        Self {
            daily_reviews,
            journal_entries,
            extractions,
            generator: Arc::new(generator),
        }
    }

    pub async fn review_day(
        &self,
        user_id: &str,
        utc_date: NaiveDate,
    ) -> Result<DailyReviewResult, DailyReviewServiceError> {
        let existing = self
            .daily_reviews
            .find_by_user_and_date(user_id, utc_date)
            .await?;

        if let Some(review) = &existing
            && review.status == DailyReviewStatus::Completed
            && review
                .review_text
                .as_deref()
                .is_some_and(|text: &str| !text.trim().is_empty())
        {
            return Ok(DailyReviewResult::Existing(review.clone()));
        }

        let entries_with_extractions: Vec<JournalEntryWithExtraction> = self
            .fetch_entries_with_extractions(user_id, utc_date)
            .await?;
        if entries_with_extractions.is_empty() {
            return Ok(DailyReviewResult::EmptyDay);
        }

        let model = self.generator.model();
        let prompt_version = self.generator.prompt_version();

        match self
            .generator
            .generate_daily_review(&entries_with_extractions)
            .await
        {
            Ok(review_text) => {
                let review_text = review_text.trim();
                if review_text.is_empty() {
                    return self
                        .store_failed_review(
                            user_id,
                            utc_date,
                            model,
                            prompt_version,
                            EMPTY_REVIEW_ERROR,
                        )
                        .await;
                }

                let review = self
                    .daily_reviews
                    .upsert_completed(user_id, utc_date, review_text, model, prompt_version)
                    .await?;
                Ok(DailyReviewResult::Generated(review))
            }
            Err(error) => {
                let error_message = error.to_string();
                self.store_failed_review(user_id, utc_date, model, prompt_version, &error_message)
                    .await
            }
        }
    }

    pub async fn fetch_review(
        &self,
        user_id: &str,
        utc_date: NaiveDate,
    ) -> Result<Option<DailyReview>, DailyReviewServiceError> {
        let review = self
            .daily_reviews
            .find_by_user_and_date(user_id, utc_date)
            .await?;
        Ok(review.filter(|r| {
            r.status == DailyReviewStatus::Completed
                && r.review_text
                    .as_deref()
                    .is_some_and(|t: &str| !t.trim().is_empty())
        }))
    }

    async fn fetch_entries_with_extractions(
        &self,
        user_id: &str,
        date: NaiveDate,
    ) -> Result<Vec<JournalEntryWithExtraction>, DailyReviewServiceError> {
        let entries = self.journal_entries.fetch_today(user_id, date).await?;
        if entries.is_empty() {
            return Ok(vec![]);
        }

        let entry_ids: Vec<i64> = entries.iter().map(|s| s.id).collect();
        let mut completed_extractions = self
            .extractions
            .find_completed_by_journal_entry_ids(&entry_ids)
            .await?;

        Ok(entries
            .into_iter()
            .map(|stored| {
                let extraction = completed_extractions.remove(&stored.id);
                JournalEntryWithExtraction {
                    id: stored.id,
                    entry: stored.entry,
                    extraction,
                }
            })
            .collect())
    }

    async fn store_failed_review(
        &self,
        user_id: &str,
        utc_date: NaiveDate,
        model: &str,
        prompt_version: &str,
        error_message: &str,
    ) -> Result<DailyReviewResult, DailyReviewServiceError> {
        self.daily_reviews
            .upsert_failed(user_id, utc_date, model, prompt_version, error_message)
            .await?;
        Ok(DailyReviewResult::GenerationFailed(DailyReviewFailure {
            user_id: user_id.to_string(),
            review_date: utc_date,
            model: model.to_string(),
            prompt_version: prompt_version.to_string(),
            error_message: error_message.to_string(),
        }))
    }
}

#[async_trait::async_trait]
impl DailyReviewRunner for DailyReviewService {
    async fn review_day(
        &self,
        user_id: &str,
        utc_date: NaiveDate,
    ) -> Result<DailyReviewResult, DailyReviewServiceError> {
        DailyReviewService::review_day(self, user_id, utc_date).await
    }

    async fn fetch_review(
        &self,
        user_id: &str,
        utc_date: NaiveDate,
    ) -> Result<Option<DailyReview>, DailyReviewServiceError> {
        DailyReviewService::fetch_review(self, user_id, utc_date).await
    }
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use chrono::{NaiveDate, TimeZone, Utc};
    use sqlx::SqlitePool;

    use super::*;
    use crate::{
        database,
        journal::{
            repository::JournalRepository,
            review::{
                DailyReview,
                generator::{ReviewGenerationError, ReviewGenerator, fake::FakeReviewGenerator},
            },
        },
        messages::{IncomingMessage, MessageSource},
    };

    async fn setup(
        generator: FakeReviewGenerator,
    ) -> (
        DailyReviewService,
        DailyReviewRepository,
        JournalRepository,
        JournalEntryExtractionRepository,
        FakeReviewGenerator,
    ) {
        database::register_sqlite_vec_extension();
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();

        let daily_reviews = DailyReviewRepository::new(pool.clone());
        let journal_entries = JournalRepository::new(pool.clone());
        let extractions = JournalEntryExtractionRepository::new(pool);
        let service = DailyReviewService::new(
            daily_reviews.clone(),
            journal_entries.clone(),
            extractions.clone(),
            generator.clone(),
        );

        (
            service,
            daily_reviews,
            journal_entries,
            extractions,
            generator,
        )
    }

    fn date() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 4, 28).unwrap()
    }

    fn incoming(
        user_id: &str,
        source_message_id: &str,
        text: &str,
        received_at: chrono::DateTime<Utc>,
    ) -> IncomingMessage {
        IncomingMessage {
            source: MessageSource::Telegram,
            source_conversation_id: "42".to_string(),
            source_message_id: source_message_id.to_string(),
            user_id: user_id.to_string(),
            text: text.to_string(),
            received_at,
        }
    }

    fn at_date(day: u32, source_message_id: &str, text: &str) -> IncomingMessage {
        incoming(
            "user-1",
            source_message_id,
            text,
            Utc.with_ymd_and_hms(2026, 4, day, 10, 0, 0).unwrap(),
        )
    }

    fn generated_review(result: DailyReviewResult) -> DailyReview {
        match result {
            DailyReviewResult::Generated(review) => review,
            other => panic!("expected generated review, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn returns_existing_completed_review_without_calling_generator() {
        let (service, daily_reviews, _journal_entries, _extractions, generator) =
            setup(FakeReviewGenerator::succeeding("new review")).await;
        let existing = daily_reviews
            .upsert_completed("user-1", date(), "existing review", "model", "v1")
            .await
            .unwrap();

        let result = service.review_day("user-1", date()).await.unwrap();

        assert_eq!(result, DailyReviewResult::Existing(existing));
        assert_eq!(generator.calls(), 0);
    }

    #[tokio::test]
    async fn generates_and_stores_review_for_entries() {
        let (service, daily_reviews, journal_entries, _extractions, generator) =
            setup(FakeReviewGenerator::succeeding("generated review")).await;
        journal_entries
            .store(&at_date(28, "1", "first entry"))
            .await
            .unwrap();
        journal_entries
            .store(&at_date(28, "2", "second entry"))
            .await
            .unwrap();

        let review = generated_review(service.review_day("user-1", date()).await.unwrap());

        assert_eq!(review.review_text, Some("generated review".to_string()));
        assert_eq!(review.status, DailyReviewStatus::Completed);
        assert_eq!(review.error_message, None);
        assert_eq!(review.model, "fake-review-model");
        assert_eq!(review.prompt_version, "fake-prompt-v1");
        assert_eq!(generator.calls(), 1);

        let stored = daily_reviews
            .find_by_user_and_date("user-1", date())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored, review);
    }

    #[tokio::test]
    async fn blank_generated_review_is_persisted_as_failure() {
        let (service, daily_reviews, journal_entries, _extractions, generator) =
            setup(FakeReviewGenerator::succeeding("   \n\t")).await;
        journal_entries
            .store(&at_date(28, "1", "first entry"))
            .await
            .unwrap();

        let result = service.review_day("user-1", date()).await.unwrap();

        assert_eq!(
            result,
            DailyReviewResult::GenerationFailed(DailyReviewFailure {
                user_id: "user-1".to_string(),
                review_date: date(),
                model: "fake-review-model".to_string(),
                prompt_version: "fake-prompt-v1".to_string(),
                error_message: "daily review generator returned an empty review".to_string(),
            })
        );
        assert_eq!(generator.calls(), 1);

        let stored = daily_reviews
            .find_by_user_and_date("user-1", date())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.status, DailyReviewStatus::Failed);
        assert_eq!(stored.review_text, None);
        assert_eq!(
            stored.error_message,
            Some("daily review generator returned an empty review".to_string())
        );
    }

    #[tokio::test]
    async fn blank_existing_completed_review_is_regenerated() {
        let (service, daily_reviews, journal_entries, _extractions, generator) =
            setup(FakeReviewGenerator::succeeding("regenerated review")).await;
        journal_entries
            .store(&at_date(28, "1", "first entry"))
            .await
            .unwrap();
        let existing = daily_reviews
            .upsert_completed("user-1", date(), "", "model", "v1")
            .await
            .unwrap();

        let review = generated_review(service.review_day("user-1", date()).await.unwrap());

        assert_eq!(generator.calls(), 1);
        assert_eq!(review.id, existing.id);
        assert_eq!(review.review_text, Some("regenerated review".to_string()));
        assert_eq!(review.status, DailyReviewStatus::Completed);
    }

    #[tokio::test]
    async fn empty_day_returns_empty_without_calling_generator() {
        let (service, _daily_reviews, _journal_entries, _extractions, generator) =
            setup(FakeReviewGenerator::succeeding("generated review")).await;

        let result = service.review_day("user-1", date()).await.unwrap();

        assert_eq!(result, DailyReviewResult::EmptyDay);
        assert_eq!(generator.calls(), 0);
    }

    #[tokio::test]
    async fn generation_failure_is_persisted_and_returned_as_domain_result() {
        let (service, daily_reviews, journal_entries, _extractions, generator) =
            setup(FakeReviewGenerator::failing("provider down")).await;
        journal_entries
            .store(&at_date(28, "1", "first entry"))
            .await
            .unwrap();

        let result = service.review_day("user-1", date()).await.unwrap();

        assert_eq!(
            result,
            DailyReviewResult::GenerationFailed(DailyReviewFailure {
                user_id: "user-1".to_string(),
                review_date: date(),
                model: "fake-review-model".to_string(),
                prompt_version: "fake-prompt-v1".to_string(),
                error_message: "provider down".to_string(),
            })
        );
        assert_eq!(generator.calls(), 1);

        let stored = daily_reviews
            .find_by_user_and_date("user-1", date())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.status, DailyReviewStatus::Failed);
        assert_eq!(stored.review_text, None);
        assert_eq!(stored.error_message, Some("provider down".to_string()));
        assert_eq!(stored.model, "fake-review-model");
        assert_eq!(stored.prompt_version, "fake-prompt-v1");
    }

    #[tokio::test]
    async fn failed_review_can_be_retried_and_updated_to_completed() {
        let generator = FakeReviewGenerator::new(vec![
            Err(crate::journal::review::generator::ReviewGenerationError::new("provider down")),
            Ok("retry review".to_string()),
        ]);
        let (service, daily_reviews, journal_entries, _extractions, generator) =
            setup(generator).await;
        journal_entries
            .store(&at_date(28, "1", "first entry"))
            .await
            .unwrap();

        assert!(matches!(
            service.review_day("user-1", date()).await.unwrap(),
            DailyReviewResult::GenerationFailed(_)
        ));
        let failed = daily_reviews
            .find_by_user_and_date("user-1", date())
            .await
            .unwrap()
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));

        let completed = generated_review(service.review_day("user-1", date()).await.unwrap());

        assert_eq!(generator.calls(), 2);
        assert_eq!(completed.id, failed.id);
        assert_eq!(completed.created_at, failed.created_at);
        assert!(completed.updated_at > failed.updated_at);
        assert_eq!(completed.status, DailyReviewStatus::Completed);
        assert_eq!(completed.review_text, Some("retry review".to_string()));
        assert_eq!(completed.error_message, None);
    }

    #[tokio::test]
    async fn users_are_isolated_for_same_review_date() {
        let (service, _daily_reviews, journal_entries, _extractions, generator) =
            setup(FakeReviewGenerator::new(vec![
                Ok("user one review".to_string()),
                Ok("user two review".to_string()),
            ]))
            .await;
        journal_entries
            .store(&incoming(
                "user-1",
                "1",
                "user one entry",
                Utc.with_ymd_and_hms(2026, 4, 28, 10, 0, 0).unwrap(),
            ))
            .await
            .unwrap();
        journal_entries
            .store(&incoming(
                "user-2",
                "2",
                "user two entry",
                Utc.with_ymd_and_hms(2026, 4, 28, 11, 0, 0).unwrap(),
            ))
            .await
            .unwrap();

        let user_one = generated_review(service.review_day("user-1", date()).await.unwrap());
        let user_two = generated_review(service.review_day("user-2", date()).await.unwrap());

        assert_eq!(user_one.review_text, Some("user one review".to_string()));
        assert_eq!(user_two.review_text, Some("user two review".to_string()));
        assert_ne!(user_one.user_id, user_two.user_id);
        assert_eq!(generator.calls(), 2);
    }

    #[tokio::test]
    async fn dates_are_isolated_and_only_requested_utc_date_entries_are_used() {
        let (service, _daily_reviews, journal_entries, _extractions, generator) =
            setup(FakeReviewGenerator::succeeding("day review")).await;
        journal_entries
            .store(&at_date(27, "1", "previous day"))
            .await
            .unwrap();
        journal_entries
            .store(&at_date(28, "2", "requested day"))
            .await
            .unwrap();
        journal_entries
            .store(&at_date(29, "3", "next day"))
            .await
            .unwrap();

        let review = generated_review(service.review_day("user-1", date()).await.unwrap());

        assert_eq!(review.review_date, date());
        assert_eq!(generator.calls(), 1);
        let entries_seen = generator.entries_seen();
        assert_eq!(entries_seen.len(), 1);
        assert_eq!(entries_seen[0].len(), 1);
        assert_eq!(entries_seen[0][0].entry.text, "requested day");
    }

    #[tokio::test]
    async fn same_user_can_store_reviews_for_different_dates() {
        let (service, _daily_reviews, journal_entries, _extractions, _generator) =
            setup(FakeReviewGenerator::new(vec![
                Ok("first day review".to_string()),
                Ok("second day review".to_string()),
            ]))
            .await;
        let next_date = NaiveDate::from_ymd_opt(2026, 4, 29).unwrap();
        journal_entries
            .store(&at_date(28, "1", "first day"))
            .await
            .unwrap();
        journal_entries
            .store(&at_date(29, "2", "second day"))
            .await
            .unwrap();

        let first = generated_review(service.review_day("user-1", date()).await.unwrap());
        let second = generated_review(service.review_day("user-1", next_date).await.unwrap());

        assert_eq!(first.review_date, date());
        assert_eq!(second.review_date, next_date);
        assert_eq!(first.review_text, Some("first day review".to_string()));
        assert_eq!(second.review_text, Some("second day review".to_string()));
    }

    #[tokio::test]
    async fn fetch_review_returns_completed_review() {
        let (service, daily_reviews, _journal_entries, _extractions, _generator) =
            setup(FakeReviewGenerator::succeeding("any")).await;
        daily_reviews
            .upsert_completed("user-1", date(), "review text", "model", "v1")
            .await
            .unwrap();

        let result = service.fetch_review("user-1", date()).await.unwrap();

        assert_eq!(result.unwrap().review_text, Some("review text".to_string()));
    }

    #[tokio::test]
    async fn fetch_review_returns_none_when_no_review_exists() {
        let (service, _daily_reviews, _journal_entries, _extractions, _generator) =
            setup(FakeReviewGenerator::succeeding("any")).await;

        let result = service.fetch_review("user-1", date()).await.unwrap();

        assert!(result.is_none());
    }

    #[tokio::test]
    async fn fetch_review_returns_none_for_failed_review() {
        let (service, daily_reviews, _journal_entries, _extractions, _generator) =
            setup(FakeReviewGenerator::succeeding("any")).await;
        daily_reviews
            .upsert_failed("user-1", date(), "model", "v1", "provider down")
            .await
            .unwrap();

        let result = service.fetch_review("user-1", date()).await.unwrap();

        assert!(result.is_none());
    }

    #[tokio::test]
    async fn persisting_failed_generation_failure_returns_service_error() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();

        let daily_reviews = DailyReviewRepository::new(pool.clone());
        let journal_entries = JournalRepository::new(pool.clone());
        let extractions = JournalEntryExtractionRepository::new(pool.clone());
        journal_entries
            .store(&at_date(28, "1", "first entry"))
            .await
            .unwrap();

        let service = DailyReviewService::new(
            daily_reviews,
            journal_entries,
            extractions,
            PoolClosingGenerator { pool },
        );

        let error = service.review_day("user-1", date()).await.unwrap_err();

        assert!(matches!(error, DailyReviewServiceError::Storage(_)));
    }

    struct PoolClosingGenerator {
        pool: SqlitePool,
    }

    #[async_trait]
    impl ReviewGenerator for PoolClosingGenerator {
        fn model(&self) -> &str {
            "pool-closing-model"
        }

        fn prompt_version(&self) -> &str {
            "pool-closing-prompt-v1"
        }

        async fn generate_daily_review(
            &self,
            _entries: &[JournalEntryWithExtraction],
        ) -> Result<String, ReviewGenerationError> {
            self.pool.close().await;
            Err(ReviewGenerationError::new("provider down"))
        }
    }

    #[tokio::test]
    async fn review_day_fetches_completed_extractions_and_passes_them_to_generator() {
        let (service, _daily_reviews, journal_entries, extractions, generator) =
            setup(FakeReviewGenerator::succeeding("extraction review")).await;

        let entry_id = journal_entries
            .store(&at_date(28, "1", "entry with extraction"))
            .await
            .unwrap()
            .unwrap();
        journal_entries
            .store(&at_date(28, "2", "entry without extraction"))
            .await
            .unwrap();

        let extraction_result = crate::journal::extraction::JournalEntryExtractionResult {
            summary: "Extracted".to_string(),
            domains: vec!["test".to_string()],
            emotions: vec![],
            behaviors: vec![],
            needs: vec![],
            possible_patterns: vec![],
        };
        extractions
            .insert_pending_if_absent(entry_id, "model", "v1")
            .await
            .unwrap();
        extractions
            .mark_completed(
                entry_id,
                &serde_json::to_string(&extraction_result).unwrap(),
                "model",
                "v1",
            )
            .await
            .unwrap();

        service.review_day("user-1", date()).await.unwrap();

        let seen = generator.entries_seen();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].len(), 2);

        // Entry 1 has extraction
        assert_eq!(seen[0][0].entry.text, "entry with extraction");
        assert_eq!(seen[0][0].extraction, Some(extraction_result));

        // Entry 2 has no extraction
        assert_eq!(seen[0][1].entry.text, "entry without extraction");
        assert_eq!(seen[0][1].extraction, None);
    }
}
