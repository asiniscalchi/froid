use async_trait::async_trait;
use chrono::{DateTime, Duration, NaiveDate, Utc};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::workers::{
    ReconciliationWorker,
    config::ReconciliationWorkerConfig,
    review_delivery::{
        ReviewDeliveryCycle, ReviewDeliveryError, ReviewDeliveryKind, ReviewSender, ReviewStep,
    },
};

use crate::{
    errors::from_error_string,
    journal::{
        repository::{JournalConversation, JournalRepository},
        responses::format_daily_review_for_date,
        review::{
            DailyReview, DailyReviewDeliveryWorkerConfig, DailyReviewResult,
            repository::{DailyReviewRepository, DailyReviewRepositoryError},
            service::DailyReviewRunner,
        },
        review_preferences::ReviewPreferenceRepository,
    },
    messages::MessageSource,
};

from_error_string!(ReviewDeliveryError::Storage, DailyReviewRepositoryError);

/// Daily half of [`ReviewDeliveryKind`]: delivers yesterday's review.
pub struct DailyReviewDelivery<R> {
    journal_entries: JournalRepository,
    daily_reviews: DailyReviewRepository,
    review_preferences: ReviewPreferenceRepository,
    review_runner: R,
    config: DailyReviewDeliveryWorkerConfig,
}

pub type DailyReviewDeliveryWorker<R, S> = ReviewDeliveryCycle<DailyReviewDelivery<R>, S>;

impl<R, S> DailyReviewDeliveryWorker<R, S>
where
    R: DailyReviewRunner,
    S: ReviewSender,
{
    pub fn new(
        journal_entries: JournalRepository,
        daily_reviews: DailyReviewRepository,
        review_preferences: ReviewPreferenceRepository,
        review_runner: R,
        sender: S,
        config: DailyReviewDeliveryWorkerConfig,
    ) -> Self {
        ReviewDeliveryCycle::from_parts(
            DailyReviewDelivery {
                journal_entries,
                daily_reviews,
                review_preferences,
                review_runner,
                config,
            },
            sender,
        )
    }

    pub async fn run_forever(self, shutdown: CancellationToken)
    where
        R: Send + Sync + 'static,
        S: Send + Sync + 'static,
    {
        let config = &self.kind().config;
        let worker_config = ReconciliationWorkerConfig {
            enabled: config.enabled,
            batch_size: 1,
            interval: config.interval,
        };
        ReconciliationWorker::new(self, worker_config)
            .run_forever(shutdown)
            .await;
    }
}

#[async_trait]
impl<R> ReviewDeliveryKind for DailyReviewDelivery<R>
where
    R: DailyReviewRunner + Send + Sync,
{
    type Review = DailyReview;

    const WORKER_LABEL: &'static str = "daily_review_delivery";
    const KIND: &'static str = "daily review";

    fn due_period(&self, now: DateTime<Utc>) -> Option<NaiveDate> {
        Some(yesterday_utc(now))
    }

    fn log_startup(&self, config: &ReconciliationWorkerConfig) {
        info!(
            enabled = config.enabled,
            interval_seconds = config.interval.as_secs(),
            "daily review delivery worker started"
        );
    }

    async fn targets(
        &self,
        period: NaiveDate,
    ) -> Result<Vec<JournalConversation>, ReviewDeliveryError> {
        if !self.review_preferences.is_daily_enabled().await? {
            return Ok(Vec::new());
        }

        Ok(self
            .journal_entries
            .conversations_with_entries_for_date(&MessageSource::Telegram, period)
            .await?)
    }

    async fn run_review(
        &self,
        period: NaiveDate,
    ) -> Result<ReviewStep<DailyReview>, ReviewDeliveryError> {
        match self.review_runner.review_day(period).await {
            Ok(DailyReviewResult::Existing(review) | DailyReviewResult::Generated(review)) => {
                Ok(ReviewStep::Ready(review))
            }
            Ok(DailyReviewResult::EmptyDay) => Ok(ReviewStep::Skipped),
            Ok(DailyReviewResult::GenerationFailed(failure)) => {
                warn!(
                    review_date = %failure.review_date,
                    error = %failure.error_message,
                    "daily review generation failed during delivery"
                );
                Ok(ReviewStep::Failed)
            }
            Err(error) => {
                warn!(
                    review_date = %period,
                    error = %error,
                    "daily review runner failed during delivery"
                );
                self.mark_delivery_failed(period, &error.to_string())
                    .await?;
                Ok(ReviewStep::Failed)
            }
        }
    }

    fn delivered_at(review: &DailyReview) -> Option<DateTime<Utc>> {
        review.delivered_at
    }

    fn format(review: &DailyReview, period: NaiveDate) -> String {
        format_daily_review_for_date(review, period)
    }

    async fn mark_delivered(&self, period: NaiveDate) -> Result<(), ReviewDeliveryError> {
        Ok(self.daily_reviews.mark_delivered(period).await?)
    }

    async fn mark_delivery_failed(
        &self,
        period: NaiveDate,
        error: &str,
    ) -> Result<(), ReviewDeliveryError> {
        Ok(self
            .daily_reviews
            .mark_delivery_failed(period, error)
            .await?)
    }
}

fn yesterday_utc(now: DateTime<Utc>) -> NaiveDate {
    (now - Duration::days(1)).date_naive()
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use crate::{
        journal::{
            extraction::repository::JournalEntryExtractionRepository,
            repository::JournalRepository,
            review::{
                DailyReview, DailyReviewResult,
                generator::fake::FakeReviewGenerator,
                repository::DailyReviewRepository,
                service::{DailyReviewService, DailyReviewServiceError},
            },
        },
        messages::IncomingMessage,
        workers::{review_delivery::ReviewDeliveryResult, test_support::FakeSender},
    };

    use super::*;

    async fn setup(
        generator: FakeReviewGenerator,
        sender: FakeSender,
    ) -> (
        DailyReviewDeliveryWorker<DailyReviewService, FakeSender>,
        DailyReviewRepository,
        JournalRepository,
        ReviewPreferenceRepository,
        FakeSender,
    ) {
        let pool = crate::database::test_pool().await;

        let journal_entries = JournalRepository::new(pool.clone());
        let daily_reviews = DailyReviewRepository::new(pool.clone());
        let review_preferences = ReviewPreferenceRepository::new(pool.clone());
        let extractions = JournalEntryExtractionRepository::new(pool.clone());
        let service = DailyReviewService::new(
            daily_reviews.clone(),
            journal_entries.clone(),
            extractions,
            crate::journal::review::signals::repository::DailyReviewSignalRepository::new(
                pool.clone(),
            ),
            generator,
        );
        let worker = DailyReviewDeliveryWorker::new(
            journal_entries.clone(),
            daily_reviews.clone(),
            review_preferences.clone(),
            service,
            sender.clone(),
            DailyReviewDeliveryWorkerConfig {
                enabled: true,
                interval: std::time::Duration::from_secs(300),
            },
        );

        (
            worker,
            daily_reviews,
            journal_entries,
            review_preferences,
            sender,
        )
    }

    #[derive(Clone)]
    struct FakeRunner {
        result: Result<DailyReviewResult, DailyReviewServiceError>,
    }

    impl FakeRunner {
        fn returning(result: Result<DailyReviewResult, DailyReviewServiceError>) -> Self {
            Self { result }
        }
    }

    #[async_trait]
    impl DailyReviewRunner for FakeRunner {
        async fn review_day(
            &self,
            _utc_date: NaiveDate,
        ) -> Result<DailyReviewResult, DailyReviewServiceError> {
            self.result.clone()
        }

        async fn fetch_review(
            &self,
            _utc_date: NaiveDate,
        ) -> Result<Option<DailyReview>, DailyReviewServiceError> {
            Ok(None)
        }
    }

    async fn setup_with_fake_runner(
        runner_result: Result<DailyReviewResult, DailyReviewServiceError>,
        sender: FakeSender,
    ) -> (
        DailyReviewDeliveryWorker<FakeRunner, FakeSender>,
        DailyReviewRepository,
        JournalRepository,
        FakeSender,
    ) {
        let pool = crate::database::test_pool().await;

        let journal_entries = JournalRepository::new(pool.clone());
        let daily_reviews = DailyReviewRepository::new(pool.clone());
        let review_preferences = ReviewPreferenceRepository::new(pool.clone());
        let runner = FakeRunner::returning(runner_result);
        let worker = DailyReviewDeliveryWorker::new(
            journal_entries.clone(),
            daily_reviews.clone(),
            review_preferences,
            runner,
            sender.clone(),
            DailyReviewDeliveryWorkerConfig {
                enabled: true,
                interval: std::time::Duration::from_secs(300),
            },
        );

        (worker, daily_reviews, journal_entries, sender)
    }

    fn date() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 4, 28).unwrap()
    }

    fn entry_for(conversation_id: &str, message_id: &str, text: &str) -> IncomingMessage {
        IncomingMessage {
            source: MessageSource::Telegram,
            source_conversation_id: conversation_id.to_string(),
            source_message_id: message_id.to_string(),
            text: text.to_string(),
            received_at: Utc.with_ymd_and_hms(2026, 4, 28, 12, 0, 0).unwrap(),
        }
    }

    fn at_date(source_message_id: &str, text: &str) -> IncomingMessage {
        entry_for("42", source_message_id, text)
    }

    #[tokio::test]
    async fn run_once_generates_sends_and_marks_yesterdays_review_delivered() {
        let sender = FakeSender::succeeding();
        let (worker, daily_reviews, journal_entries, _review_preferences, sender) =
            setup(FakeReviewGenerator::succeeding("generated review"), sender).await;
        journal_entries
            .store(&at_date("1", "first entry"))
            .await
            .unwrap();

        let result = worker
            .run_once(Utc.with_ymd_and_hms(2026, 4, 29, 0, 5, 0).unwrap())
            .await
            .unwrap();

        assert_eq!(
            result,
            ReviewDeliveryResult {
                attempted: 1,
                delivered: 1,
                skipped: 0,
                failed: 0,
            }
        );
        let sent = sender.sent();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].0, "42");
        assert!(sent[0].1.contains("Daily review for 2026-04-28"));
        assert!(sent[0].1.contains("generated review"));

        let review = daily_reviews
            .find_by_user_and_date(date())
            .await
            .unwrap()
            .unwrap();
        assert!(review.delivered_at.is_some());
        assert_eq!(review.delivery_error, None);
    }

    #[tokio::test]
    async fn run_once_skips_delivery_when_user_opted_out_of_reviews() {
        let sender = FakeSender::succeeding();
        let (worker, daily_reviews, journal_entries, review_preferences, sender) =
            setup(FakeReviewGenerator::succeeding("generated review"), sender).await;
        journal_entries
            .store(&at_date("1", "first entry"))
            .await
            .unwrap();
        review_preferences.set_daily_enabled(false).await.unwrap();

        let result = worker
            .run_once(Utc.with_ymd_and_hms(2026, 4, 29, 0, 5, 0).unwrap())
            .await
            .unwrap();

        assert_eq!(
            result,
            ReviewDeliveryResult {
                attempted: 0,
                delivered: 0,
                skipped: 0,
                failed: 0,
            }
        );
        assert!(sender.sent().is_empty());
        assert!(
            daily_reviews
                .find_by_user_and_date(date())
                .await
                .unwrap()
                .is_none(),
            "opted-out user must not trigger review generation either"
        );
    }

    #[tokio::test]
    async fn run_once_still_delivers_when_only_the_weekly_review_is_off() {
        let sender = FakeSender::succeeding();
        let (worker, _daily_reviews, journal_entries, review_preferences, sender) =
            setup(FakeReviewGenerator::succeeding("generated review"), sender).await;
        journal_entries
            .store(&at_date("1", "first entry"))
            .await
            .unwrap();
        review_preferences.set_weekly_enabled(false).await.unwrap();

        let result = worker
            .run_once(Utc.with_ymd_and_hms(2026, 4, 29, 0, 5, 0).unwrap())
            .await
            .unwrap();

        assert_eq!(
            result,
            ReviewDeliveryResult {
                attempted: 1,
                delivered: 1,
                skipped: 0,
                failed: 0,
            }
        );
        assert_eq!(sender.sent().len(), 1);
    }

    #[tokio::test]
    async fn run_once_skips_already_delivered_review() {
        let sender = FakeSender::succeeding();
        let (worker, daily_reviews, journal_entries, _review_preferences, sender) =
            setup(FakeReviewGenerator::succeeding("generated review"), sender).await;
        journal_entries
            .store(&at_date("1", "first entry"))
            .await
            .unwrap();
        daily_reviews
            .upsert_completed(date(), "existing review", "model", "v1")
            .await
            .unwrap();
        daily_reviews.mark_delivered(date()).await.unwrap();

        let result = worker.run_once_for(date()).await.unwrap();

        assert_eq!(
            result,
            ReviewDeliveryResult {
                attempted: 1,
                delivered: 0,
                skipped: 1,
                failed: 0,
            }
        );
        assert!(sender.sent().is_empty());
    }

    #[tokio::test]
    async fn run_once_records_delivery_failure_for_retry() {
        let sender = FakeSender::failing("telegram unavailable");
        let (worker, daily_reviews, journal_entries, _review_preferences, sender) =
            setup(FakeReviewGenerator::succeeding("generated review"), sender).await;
        journal_entries
            .store(&at_date("1", "first entry"))
            .await
            .unwrap();

        let result = worker.run_once_for(date()).await.unwrap();

        assert_eq!(
            result,
            ReviewDeliveryResult {
                attempted: 1,
                delivered: 0,
                skipped: 0,
                failed: 1,
            }
        );
        assert_eq!(sender.sent().len(), 1);

        let review = daily_reviews
            .find_by_user_and_date(date())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(review.delivered_at, None);
        assert_eq!(
            review.delivery_error,
            Some("telegram unavailable".to_string())
        );
    }

    #[tokio::test]
    async fn run_once_does_not_mark_skipped_delivery_as_delivered() {
        let sender = FakeSender::skipped();
        let (worker, daily_reviews, journal_entries, _review_preferences, sender) =
            setup(FakeReviewGenerator::succeeding("generated review"), sender).await;
        journal_entries
            .store(&at_date("1", "first entry"))
            .await
            .unwrap();

        let result = worker.run_once_for(date()).await.unwrap();

        assert_eq!(
            result,
            ReviewDeliveryResult {
                attempted: 1,
                delivered: 0,
                skipped: 1,
                failed: 0,
            }
        );
        assert_eq!(sender.sent().len(), 1);

        let review = daily_reviews
            .find_by_user_and_date(date())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(review.delivered_at, None);
        assert_eq!(review.delivery_error, None);
    }

    #[tokio::test]
    async fn run_once_returns_empty_result_when_no_entries() {
        let (worker, _, _, _, _) = setup(
            FakeReviewGenerator::succeeding("irrelevant"),
            FakeSender::succeeding(),
        )
        .await;

        let result = worker.run_once_for(date()).await.unwrap();

        assert_eq!(
            result,
            ReviewDeliveryResult {
                attempted: 0,
                delivered: 0,
                skipped: 0,
                failed: 0,
            }
        );
    }

    #[tokio::test]
    async fn run_once_skips_when_runner_returns_empty_day() {
        let sender = FakeSender::succeeding();
        let (worker, _, journal_entries, sender) =
            setup_with_fake_runner(Ok(DailyReviewResult::EmptyDay), sender).await;
        journal_entries
            .store(&at_date("1", "an entry"))
            .await
            .unwrap();

        let result = worker.run_once_for(date()).await.unwrap();

        assert_eq!(
            result,
            ReviewDeliveryResult {
                attempted: 1,
                delivered: 0,
                skipped: 1,
                failed: 0,
            }
        );
        assert!(sender.sent().is_empty());
    }

    #[tokio::test]
    async fn run_once_counts_as_failed_when_generation_fails() {
        let sender = FakeSender::succeeding();
        let (worker, _, journal_entries, _review_preferences, sender) =
            setup(FakeReviewGenerator::failing("generator error"), sender).await;
        journal_entries
            .store(&at_date("1", "an entry"))
            .await
            .unwrap();

        let result = worker.run_once_for(date()).await.unwrap();

        assert_eq!(
            result,
            ReviewDeliveryResult {
                attempted: 1,
                delivered: 0,
                skipped: 0,
                failed: 1,
            }
        );
        assert!(sender.sent().is_empty());
    }

    #[tokio::test]
    async fn run_once_records_runner_error_as_delivery_failure() {
        let sender = FakeSender::succeeding();
        let (worker, daily_reviews, journal_entries, _) = setup_with_fake_runner(
            Err(DailyReviewServiceError::Storage("db error".to_string())),
            sender,
        )
        .await;
        journal_entries
            .store(&at_date("1", "an entry"))
            .await
            .unwrap();
        daily_reviews
            .upsert_completed(date(), "existing review", "model", "v1")
            .await
            .unwrap();

        let result = worker.run_once_for(date()).await.unwrap();

        assert_eq!(
            result,
            ReviewDeliveryResult {
                attempted: 1,
                delivered: 0,
                skipped: 0,
                failed: 1,
            }
        );
        let review = daily_reviews
            .find_by_user_and_date(date())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(review.delivery_error, Some("db error".to_string()));
        assert_eq!(review.delivered_at, None);
    }

    #[tokio::test]
    async fn run_once_delivers_single_review_once_for_multiple_conversations() {
        let sender = FakeSender::succeeding();
        let (worker, daily_reviews, journal_entries, _review_preferences, sender) =
            setup(FakeReviewGenerator::succeeding("review text"), sender).await;
        journal_entries
            .store(&entry_for("42", "1", "first"))
            .await
            .unwrap();
        journal_entries
            .store(&entry_for("99", "2", "second"))
            .await
            .unwrap();

        let result = worker.run_once_for(date()).await.unwrap();

        assert_eq!(
            result,
            ReviewDeliveryResult {
                attempted: 2,
                delivered: 1,
                skipped: 1,
                failed: 0,
            }
        );
        let sent = sender.sent();
        assert_eq!(sent.len(), 1);
        let chat_ids: Vec<&str> = sent.iter().map(|(id, _)| id.as_str()).collect();
        assert!(chat_ids.contains(&"42"));
        assert!(
            daily_reviews
                .find_by_user_and_date(date())
                .await
                .unwrap()
                .unwrap()
                .delivered_at
                .is_some()
        );
    }

    #[tokio::test]
    async fn run_forever_exits_when_cancellation_token_fires() {
        let pool = crate::database::test_pool().await;

        let worker = DailyReviewDeliveryWorker::new(
            JournalRepository::new(pool.clone()),
            DailyReviewRepository::new(pool.clone()),
            ReviewPreferenceRepository::new(pool.clone()),
            FakeRunner::returning(Ok(DailyReviewResult::EmptyDay)),
            FakeSender::succeeding(),
            DailyReviewDeliveryWorkerConfig {
                enabled: true,
                interval: std::time::Duration::from_millis(1),
            },
        );

        let shutdown = CancellationToken::new();
        let handle = tokio::spawn({
            let shutdown = shutdown.clone();
            async move {
                worker.run_forever(shutdown).await;
            }
        });

        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        shutdown.cancel();
        handle.await.expect("worker task ran to completion");
    }
}
