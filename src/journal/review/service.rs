use std::{error::Error, fmt, sync::Arc};

use tracing::warn;

use chrono::NaiveDate;

use crate::journal::{
    repository::JournalRepository,
    review::{
        DailyReviewFailure, DailyReviewResult, DailyReviewStatus,
        generator::ReviewGenerator,
        repository::{DailyReviewRepository, DailyReviewRepositoryError},
    },
};

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

#[derive(Clone)]
pub struct DailyReviewService {
    daily_reviews: DailyReviewRepository,
    journal_entries: JournalRepository,
    generator: Arc<dyn ReviewGenerator>,
}

#[async_trait::async_trait]
pub trait DailyReviewRunner: Send + Sync {
    async fn review_day(
        &self,
        user_id: &str,
        utc_date: NaiveDate,
    ) -> Result<DailyReviewResult, DailyReviewServiceError>;
}

impl DailyReviewService {
    pub fn new<G>(
        daily_reviews: DailyReviewRepository,
        journal_entries: JournalRepository,
        generator: G,
    ) -> Self
    where
        G: ReviewGenerator + 'static,
    {
        Self {
            daily_reviews,
            journal_entries,
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
        {
            let latest_entry_at = self
                .journal_entries
                .latest_entry_received_at_for_user_date(user_id, utc_date)
                .await?;
            let is_fresh = latest_entry_at.is_none_or(|at| at <= review.updated_at);
            if is_fresh {
                return Ok(DailyReviewResult::Existing(review.clone()));
            }
        }

        let entries = self.journal_entries.fetch_today(user_id, utc_date).await?;
        if entries.is_empty() {
            return Ok(DailyReviewResult::EmptyDay);
        }

        let model = self.generator.model();
        let prompt_version = self.generator.prompt_version();
        let has_existing_completed = existing
            .as_ref()
            .is_some_and(|r| r.status == DailyReviewStatus::Completed);

        match self.generator.generate_daily_review(&entries).await {
            Ok(review_text) => {
                let review = self
                    .daily_reviews
                    .upsert_completed(user_id, utc_date, &review_text, model, prompt_version)
                    .await?;
                Ok(DailyReviewResult::Generated(review))
            }
            Err(error) => {
                let error_message = error.to_string();
                if has_existing_completed {
                    warn!(
                        user_id = user_id,
                        review_date = %utc_date,
                        model = model,
                        prompt_version = prompt_version,
                        error = %error_message,
                        "stale review regeneration failed, preserving existing completed review"
                    );
                } else {
                    self.daily_reviews
                        .upsert_failed(user_id, utc_date, model, prompt_version, &error_message)
                        .await?;
                }
                Ok(DailyReviewResult::GenerationFailed(DailyReviewFailure {
                    user_id: user_id.to_string(),
                    review_date: utc_date,
                    model: model.to_string(),
                    prompt_version: prompt_version.to_string(),
                    error_message,
                }))
            }
        }
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
        FakeReviewGenerator,
    ) {
        database::register_sqlite_vec_extension();
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();

        let daily_reviews = DailyReviewRepository::new(pool.clone());
        let journal_entries = JournalRepository::new(pool);
        let service = DailyReviewService::new(
            daily_reviews.clone(),
            journal_entries.clone(),
            generator.clone(),
        );

        (service, daily_reviews, journal_entries, generator)
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
        let (service, daily_reviews, _journal_entries, generator) =
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
        let (service, daily_reviews, journal_entries, generator) =
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
    async fn empty_day_returns_empty_without_calling_generator() {
        let (service, _daily_reviews, _journal_entries, generator) =
            setup(FakeReviewGenerator::succeeding("generated review")).await;

        let result = service.review_day("user-1", date()).await.unwrap();

        assert_eq!(result, DailyReviewResult::EmptyDay);
        assert_eq!(generator.calls(), 0);
    }

    #[tokio::test]
    async fn generation_failure_is_persisted_and_returned_as_domain_result() {
        let (service, daily_reviews, journal_entries, generator) =
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
        let (service, daily_reviews, journal_entries, generator) = setup(generator).await;
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
        let (service, _daily_reviews, journal_entries, generator) =
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
        let (service, _daily_reviews, journal_entries, generator) =
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
        assert_eq!(entries_seen[0][0].text, "requested day");
    }

    #[tokio::test]
    async fn same_user_can_store_reviews_for_different_dates() {
        let (service, _daily_reviews, journal_entries, _generator) =
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
    async fn persisting_failed_generation_failure_returns_service_error() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();

        let daily_reviews = DailyReviewRepository::new(pool.clone());
        let journal_entries = JournalRepository::new(pool.clone());
        journal_entries
            .store(&at_date(28, "1", "first entry"))
            .await
            .unwrap();

        let service = DailyReviewService::new(
            daily_reviews,
            journal_entries,
            PoolClosingGenerator { pool },
        );

        let error = service.review_day("user-1", date()).await.unwrap_err();

        assert!(matches!(error, DailyReviewServiceError::Storage(_)));
    }

    fn at(h: u32, m: u32) -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 4, 28, h, m, 0).unwrap()
    }

    // Backdates the updated_at of the daily_review row for user-1 / date() so that
    // entries with received_at after the backdated time will be detected as stale.
    async fn backdate_review_updated_at(journal_entries: &JournalRepository, updated_at: &str) {
        sqlx::query(
            "UPDATE daily_reviews SET updated_at = ? WHERE user_id = 'user-1' AND review_date = '2026-04-28'",
        )
        .bind(updated_at)
        .execute(journal_entries.pool())
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn fresh_completed_review_is_returned_without_calling_generator() {
        let (service, daily_reviews, journal_entries, generator) =
            setup(FakeReviewGenerator::succeeding("new review")).await;
        journal_entries
            .store(&incoming("user-1", "1", "early entry", at(9, 0)))
            .await
            .unwrap();
        let existing = daily_reviews
            .upsert_completed("user-1", date(), "existing review", "model", "v1")
            .await
            .unwrap();
        // Review updated_at is wall-clock now; entry received_at is 2026-04-28 09:00.
        // Since entry is older than review, review is fresh.

        let result = service.review_day("user-1", date()).await.unwrap();

        assert_eq!(result, DailyReviewResult::Existing(existing));
        assert_eq!(generator.calls(), 0);
    }

    #[tokio::test]
    async fn stale_completed_review_is_regenerated_when_newer_entry_exists() {
        let (service, daily_reviews, journal_entries, generator) =
            setup(FakeReviewGenerator::succeeding("regenerated review")).await;
        journal_entries
            .store(&incoming("user-1", "1", "early entry", at(9, 0)))
            .await
            .unwrap();
        let before_regen = daily_reviews
            .upsert_completed("user-1", date(), "old review", "model", "v1")
            .await
            .unwrap();
        // Backdate review so a new same-day entry at 11:00 looks newer.
        backdate_review_updated_at(&journal_entries, "2026-04-28T09:30:00.000Z").await;
        let existing = daily_reviews
            .find_by_user_and_date("user-1", date())
            .await
            .unwrap()
            .unwrap();
        journal_entries
            .store(&incoming("user-1", "2", "new entry", at(11, 0)))
            .await
            .unwrap();

        let result = service.review_day("user-1", date()).await.unwrap();

        assert!(
            matches!(result, DailyReviewResult::Generated(ref r) if r.review_text == Some("regenerated review".to_string()))
        );
        assert_eq!(generator.calls(), 1);
        let stored = daily_reviews
            .find_by_user_and_date("user-1", date())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.id, before_regen.id);
        assert_eq!(stored.created_at, before_regen.created_at);
        assert!(stored.updated_at > existing.updated_at);
        assert_eq!(stored.review_text, Some("regenerated review".to_string()));
        assert_eq!(stored.status, DailyReviewStatus::Completed);
        assert_eq!(stored.error_message, None);
    }

    #[tokio::test]
    async fn stale_review_regeneration_updates_model_and_prompt_version() {
        let (service, daily_reviews, journal_entries, _) =
            setup(FakeReviewGenerator::succeeding("regen")).await;
        journal_entries
            .store(&incoming("user-1", "1", "early entry", at(9, 0)))
            .await
            .unwrap();
        daily_reviews
            .upsert_completed("user-1", date(), "old review", "old-model", "old-v")
            .await
            .unwrap();
        backdate_review_updated_at(&journal_entries, "2026-04-28T09:30:00.000Z").await;
        journal_entries
            .store(&incoming("user-1", "2", "new entry", at(11, 0)))
            .await
            .unwrap();

        service.review_day("user-1", date()).await.unwrap();

        let stored = daily_reviews
            .find_by_user_and_date("user-1", date())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.model, "fake-review-model");
        assert_eq!(stored.prompt_version, "fake-prompt-v1");
    }

    #[tokio::test]
    async fn stale_review_regeneration_failure_preserves_existing_completed_review() {
        let (service, daily_reviews, journal_entries, generator) =
            setup(FakeReviewGenerator::failing("provider down")).await;
        journal_entries
            .store(&incoming("user-1", "1", "early entry", at(9, 0)))
            .await
            .unwrap();
        daily_reviews
            .upsert_completed("user-1", date(), "old review", "model", "v1")
            .await
            .unwrap();
        backdate_review_updated_at(&journal_entries, "2026-04-28T09:30:00.000Z").await;
        let existing = daily_reviews
            .find_by_user_and_date("user-1", date())
            .await
            .unwrap()
            .unwrap();
        journal_entries
            .store(&incoming("user-1", "2", "new entry", at(11, 0)))
            .await
            .unwrap();

        let result = service.review_day("user-1", date()).await.unwrap();

        assert!(matches!(result, DailyReviewResult::GenerationFailed(_)));
        assert_eq!(generator.calls(), 1);
        let stored = daily_reviews
            .find_by_user_and_date("user-1", date())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored, existing);
    }

    #[tokio::test]
    async fn stale_review_regeneration_failure_does_not_change_row_to_failed() {
        let (service, daily_reviews, journal_entries, _) =
            setup(FakeReviewGenerator::failing("provider down")).await;
        journal_entries
            .store(&incoming("user-1", "1", "early entry", at(9, 0)))
            .await
            .unwrap();
        daily_reviews
            .upsert_completed("user-1", date(), "old review", "model", "v1")
            .await
            .unwrap();
        backdate_review_updated_at(&journal_entries, "2026-04-28T09:30:00.000Z").await;
        journal_entries
            .store(&incoming("user-1", "2", "new entry", at(11, 0)))
            .await
            .unwrap();

        service.review_day("user-1", date()).await.unwrap();

        let stored = daily_reviews
            .find_by_user_and_date("user-1", date())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.status, DailyReviewStatus::Completed);
        assert_eq!(stored.review_text, Some("old review".to_string()));
        assert_eq!(stored.error_message, None);
    }

    #[tokio::test]
    async fn stale_review_regeneration_failure_returns_generation_failed_result() {
        let (service, daily_reviews, journal_entries, _) =
            setup(FakeReviewGenerator::failing("provider down")).await;
        journal_entries
            .store(&incoming("user-1", "1", "early entry", at(9, 0)))
            .await
            .unwrap();
        daily_reviews
            .upsert_completed("user-1", date(), "old review", "model", "v1")
            .await
            .unwrap();
        backdate_review_updated_at(&journal_entries, "2026-04-28T09:30:00.000Z").await;
        journal_entries
            .store(&incoming("user-1", "2", "new entry", at(11, 0)))
            .await
            .unwrap();

        let result = service.review_day("user-1", date()).await.unwrap();

        assert!(matches!(
            result,
            DailyReviewResult::GenerationFailed(DailyReviewFailure {
                ref error_message,
                ..
            }) if error_message == "provider down"
        ));
    }

    #[tokio::test]
    async fn freshness_check_is_scoped_by_user() {
        let (service, daily_reviews, journal_entries, generator) =
            setup(FakeReviewGenerator::succeeding("regen")).await;
        journal_entries
            .store(&incoming("user-1", "1", "user one early", at(9, 0)))
            .await
            .unwrap();
        daily_reviews
            .upsert_completed("user-1", date(), "user one review", "model", "v1")
            .await
            .unwrap();
        // user-1's review is fresh (wall-clock updated_at > entry received_at).
        // user-2 has a new entry after their review — user-2's staleness must not affect user-1.
        journal_entries
            .store(&incoming("user-2", "2", "user two entry", at(9, 0)))
            .await
            .unwrap();
        daily_reviews
            .upsert_completed("user-2", date(), "user two review", "model", "v1")
            .await
            .unwrap();
        journal_entries
            .store(&incoming("user-2", "3", "user two new entry", at(11, 0)))
            .await
            .unwrap();

        let result_user_one = service.review_day("user-1", date()).await.unwrap();

        assert!(matches!(result_user_one, DailyReviewResult::Existing(_)));
        assert_eq!(generator.calls(), 0);
    }

    #[tokio::test]
    async fn freshness_check_uses_only_entries_from_requested_date() {
        let (service, daily_reviews, journal_entries, generator) =
            setup(FakeReviewGenerator::succeeding("regen")).await;
        journal_entries
            .store(&incoming("user-1", "1", "today entry", at(9, 0)))
            .await
            .unwrap();
        daily_reviews
            .upsert_completed("user-1", date(), "today review", "model", "v1")
            .await
            .unwrap();
        backdate_review_updated_at(&journal_entries, "2026-04-28T09:30:00.000Z").await;
        // Next-day entry must not cause staleness for date() = 2026-04-28.
        journal_entries
            .store(&incoming(
                "user-1",
                "2",
                "next day entry",
                Utc.with_ymd_and_hms(2026, 4, 29, 10, 0, 0).unwrap(),
            ))
            .await
            .unwrap();

        let result = service.review_day("user-1", date()).await.unwrap();

        assert!(matches!(result, DailyReviewResult::Existing(_)));
        assert_eq!(generator.calls(), 0);
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
            _entries: &[crate::journal::entry::JournalEntry],
        ) -> Result<String, ReviewGenerationError> {
            self.pool.close().await;
            Err(ReviewGenerationError::new("provider down"))
        }
    }
}
