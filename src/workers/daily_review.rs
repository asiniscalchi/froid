use async_trait::async_trait;
use chrono::{DateTime, Duration, NaiveDate, Utc};
use teloxide::{prelude::*, types::ChatId};
use tracing::{error, info, warn};

use crate::{
    journal::{
        repository::JournalRepository,
        responses::format_daily_review_for_date,
        review::{
            DailyReviewDeliveryWorkerConfig, DailyReviewResult,
            repository::{DailyReviewRepository, DailyReviewRepositoryError},
            service::{DailyReviewRunner, DailyReviewServiceError},
        },
    },
    messages::MessageSource,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DailyReviewDeliveryResult {
    pub attempted: usize,
    pub delivered: usize,
    pub skipped: usize,
    pub failed: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DailyReviewDeliveryWorkerError {
    Storage(String),
}

impl std::fmt::Display for DailyReviewDeliveryWorkerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Storage(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for DailyReviewDeliveryWorkerError {}

impl From<sqlx::Error> for DailyReviewDeliveryWorkerError {
    fn from(error: sqlx::Error) -> Self {
        Self::Storage(error.to_string())
    }
}

impl From<DailyReviewRepositoryError> for DailyReviewDeliveryWorkerError {
    fn from(error: DailyReviewRepositoryError) -> Self {
        Self::Storage(error.to_string())
    }
}

#[async_trait]
pub trait DailyReviewSender: Send + Sync {
    async fn send_daily_review(
        &self,
        source_conversation_id: &str,
        text: &str,
    ) -> Result<(), String>;
}

#[derive(Clone)]
pub struct TelegramDailyReviewSender {
    bot: Bot,
}

impl TelegramDailyReviewSender {
    pub fn new(bot_token: String) -> Self {
        Self {
            bot: Bot::new(bot_token),
        }
    }
}

#[async_trait]
impl DailyReviewSender for TelegramDailyReviewSender {
    async fn send_daily_review(
        &self,
        source_conversation_id: &str,
        text: &str,
    ) -> Result<(), String> {
        let chat_id = source_conversation_id
            .parse::<i64>()
            .map_err(|_| format!("invalid Telegram chat id: {source_conversation_id}"))?;

        self.bot
            .send_message(ChatId(chat_id), text.to_string())
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

pub struct DailyReviewDeliveryWorker<R, S> {
    journal_entries: JournalRepository,
    daily_reviews: DailyReviewRepository,
    review_runner: R,
    sender: S,
    config: DailyReviewDeliveryWorkerConfig,
}

impl<R, S> DailyReviewDeliveryWorker<R, S>
where
    R: DailyReviewRunner,
    S: DailyReviewSender,
{
    pub fn new(
        journal_entries: JournalRepository,
        daily_reviews: DailyReviewRepository,
        review_runner: R,
        sender: S,
        config: DailyReviewDeliveryWorkerConfig,
    ) -> Self {
        Self {
            journal_entries,
            daily_reviews,
            review_runner,
            sender,
            config,
        }
    }

    pub async fn run_once(
        &self,
        now: DateTime<Utc>,
    ) -> Result<DailyReviewDeliveryResult, DailyReviewDeliveryWorkerError> {
        let review_date = yesterday_utc(now);
        self.run_once_for_date(review_date).await
    }

    pub async fn run_once_for_date(
        &self,
        review_date: NaiveDate,
    ) -> Result<DailyReviewDeliveryResult, DailyReviewDeliveryWorkerError> {
        let targets = self
            .journal_entries
            .conversations_with_entries_for_date(&MessageSource::Telegram, review_date)
            .await?;

        let mut result = DailyReviewDeliveryResult {
            attempted: targets.len(),
            delivered: 0,
            skipped: 0,
            failed: 0,
        };

        for target in targets {
            let review = match self
                .review_runner
                .review_day(&target.user_id, review_date)
                .await
            {
                Ok(DailyReviewResult::Existing(review) | DailyReviewResult::Generated(review)) => {
                    review
                }
                Ok(DailyReviewResult::EmptyDay) => {
                    result.skipped += 1;
                    continue;
                }
                Ok(DailyReviewResult::GenerationFailed(failure)) => {
                    warn!(
                        user_id = %failure.user_id,
                        review_date = %failure.review_date,
                        error = %failure.error_message,
                        "daily review generation failed during delivery"
                    );
                    result.failed += 1;
                    continue;
                }
                Err(error) => {
                    self.record_review_runner_error(&target.user_id, review_date, error)
                        .await?;
                    result.failed += 1;
                    continue;
                }
            };

            if review.delivered_at.is_some() {
                result.skipped += 1;
                continue;
            }

            let text = format_daily_review_for_date(&review, review_date);
            match self
                .sender
                .send_daily_review(&target.source_conversation_id, &text)
                .await
            {
                Ok(()) => {
                    self.daily_reviews
                        .mark_delivered(&target.user_id, review_date)
                        .await?;
                    result.delivered += 1;
                }
                Err(error) => {
                    self.daily_reviews
                        .mark_delivery_failed(&target.user_id, review_date, &error)
                        .await?;
                    warn!(
                        user_id = %target.user_id,
                        source_conversation_id = %target.source_conversation_id,
                        review_date = %review_date,
                        error = %error,
                        "failed to deliver daily review"
                    );
                    result.failed += 1;
                }
            }
        }

        Ok(result)
    }

    async fn record_review_runner_error(
        &self,
        user_id: &str,
        review_date: NaiveDate,
        error: DailyReviewServiceError,
    ) -> Result<(), DailyReviewDeliveryWorkerError> {
        warn!(
            user_id = %user_id,
            review_date = %review_date,
            error = %error,
            "daily review runner failed during delivery"
        );
        self.daily_reviews
            .mark_delivery_failed(user_id, review_date, &error.to_string())
            .await?;
        Ok(())
    }

    pub async fn run_forever(self) {
        info!(
            enabled = self.config.enabled,
            interval_seconds = self.config.interval.as_secs(),
            "daily review delivery worker started"
        );

        loop {
            match self.run_once(Utc::now()).await {
                Ok(result) => {
                    if result.attempted > 0 && (result.delivered > 0 || result.failed > 0) {
                        info!(
                            attempted = result.attempted,
                            delivered = result.delivered,
                            skipped = result.skipped,
                            failed = result.failed,
                            "daily review delivery cycle completed"
                        );
                    }
                }
                Err(err) => {
                    error!(error = %err, "daily review delivery cycle failed");
                }
            }
            tokio::time::sleep(self.config.interval).await;
        }
    }
}

fn yesterday_utc(now: DateTime<Utc>) -> NaiveDate {
    (now - Duration::days(1)).date_naive()
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use chrono::TimeZone;

    use crate::{
        database,
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
    };
    use sqlx::SqlitePool;

    use super::*;

    #[derive(Debug, Clone)]
    struct FakeSender {
        sent: Arc<Mutex<Vec<(String, String)>>>,
        result: Result<(), String>,
    }

    impl FakeSender {
        fn succeeding() -> Self {
            Self {
                sent: Arc::new(Mutex::new(Vec::new())),
                result: Ok(()),
            }
        }

        fn failing(error: &str) -> Self {
            Self {
                sent: Arc::new(Mutex::new(Vec::new())),
                result: Err(error.to_string()),
            }
        }

        fn sent(&self) -> Vec<(String, String)> {
            self.sent.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl DailyReviewSender for FakeSender {
        async fn send_daily_review(
            &self,
            source_conversation_id: &str,
            text: &str,
        ) -> Result<(), String> {
            self.sent
                .lock()
                .unwrap()
                .push((source_conversation_id.to_string(), text.to_string()));
            self.result.clone()
        }
    }

    async fn setup(
        generator: FakeReviewGenerator,
        sender: FakeSender,
    ) -> (
        DailyReviewDeliveryWorker<DailyReviewService, FakeSender>,
        DailyReviewRepository,
        JournalRepository,
        FakeSender,
    ) {
        database::register_sqlite_vec_extension();
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();

        let journal_entries = JournalRepository::new(pool.clone());
        let daily_reviews = DailyReviewRepository::new(pool.clone());
        let extractions = JournalEntryExtractionRepository::new(pool);
        let service = DailyReviewService::new(
            daily_reviews.clone(),
            journal_entries.clone(),
            extractions,
            generator,
        );
        let worker = DailyReviewDeliveryWorker::new(
            journal_entries.clone(),
            daily_reviews.clone(),
            service,
            sender.clone(),
            DailyReviewDeliveryWorkerConfig {
                enabled: true,
                interval: std::time::Duration::from_secs(300),
            },
        );

        (worker, daily_reviews, journal_entries, sender)
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
            _user_id: &str,
            _utc_date: NaiveDate,
        ) -> Result<DailyReviewResult, DailyReviewServiceError> {
            self.result.clone()
        }

        async fn fetch_review(
            &self,
            _user_id: &str,
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
        database::register_sqlite_vec_extension();
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();

        let journal_entries = JournalRepository::new(pool.clone());
        let daily_reviews = DailyReviewRepository::new(pool.clone());
        let runner = FakeRunner::returning(runner_result);
        let worker = DailyReviewDeliveryWorker::new(
            journal_entries.clone(),
            daily_reviews.clone(),
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

    fn entry_for(
        user_id: &str,
        conversation_id: &str,
        message_id: &str,
        text: &str,
    ) -> IncomingMessage {
        IncomingMessage {
            source: MessageSource::Telegram,
            source_conversation_id: conversation_id.to_string(),
            source_message_id: message_id.to_string(),
            user_id: user_id.to_string(),
            text: text.to_string(),
            received_at: Utc.with_ymd_and_hms(2026, 4, 28, 12, 0, 0).unwrap(),
        }
    }

    fn at_date(source_message_id: &str, text: &str) -> IncomingMessage {
        IncomingMessage {
            source: MessageSource::Telegram,
            source_conversation_id: "42".to_string(),
            source_message_id: source_message_id.to_string(),
            user_id: "7".to_string(),
            text: text.to_string(),
            received_at: Utc.with_ymd_and_hms(2026, 4, 28, 12, 0, 0).unwrap(),
        }
    }

    #[tokio::test]
    async fn run_once_generates_sends_and_marks_yesterdays_review_delivered() {
        let sender = FakeSender::succeeding();
        let (worker, daily_reviews, journal_entries, sender) =
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
            DailyReviewDeliveryResult {
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
            .find_by_user_and_date("7", date())
            .await
            .unwrap()
            .unwrap();
        assert!(review.delivered_at.is_some());
        assert_eq!(review.delivery_error, None);
    }

    #[tokio::test]
    async fn run_once_skips_already_delivered_review() {
        let sender = FakeSender::succeeding();
        let (worker, daily_reviews, journal_entries, sender) =
            setup(FakeReviewGenerator::succeeding("generated review"), sender).await;
        journal_entries
            .store(&at_date("1", "first entry"))
            .await
            .unwrap();
        daily_reviews
            .upsert_completed("7", date(), "existing review", "model", "v1")
            .await
            .unwrap();
        daily_reviews.mark_delivered("7", date()).await.unwrap();

        let result = worker.run_once_for_date(date()).await.unwrap();

        assert_eq!(
            result,
            DailyReviewDeliveryResult {
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
        let (worker, daily_reviews, journal_entries, sender) =
            setup(FakeReviewGenerator::succeeding("generated review"), sender).await;
        journal_entries
            .store(&at_date("1", "first entry"))
            .await
            .unwrap();

        let result = worker.run_once_for_date(date()).await.unwrap();

        assert_eq!(
            result,
            DailyReviewDeliveryResult {
                attempted: 1,
                delivered: 0,
                skipped: 0,
                failed: 1,
            }
        );
        assert_eq!(sender.sent().len(), 1);

        let review = daily_reviews
            .find_by_user_and_date("7", date())
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
    async fn run_once_returns_empty_result_when_no_entries() {
        let (worker, _, _, _) = setup(
            FakeReviewGenerator::succeeding("irrelevant"),
            FakeSender::succeeding(),
        )
        .await;

        let result = worker.run_once_for_date(date()).await.unwrap();

        assert_eq!(
            result,
            DailyReviewDeliveryResult {
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

        let result = worker.run_once_for_date(date()).await.unwrap();

        assert_eq!(
            result,
            DailyReviewDeliveryResult {
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
        let (worker, _, journal_entries, sender) =
            setup(FakeReviewGenerator::failing("generator error"), sender).await;
        journal_entries
            .store(&at_date("1", "an entry"))
            .await
            .unwrap();

        let result = worker.run_once_for_date(date()).await.unwrap();

        assert_eq!(
            result,
            DailyReviewDeliveryResult {
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
            .upsert_completed("7", date(), "existing review", "model", "v1")
            .await
            .unwrap();

        let result = worker.run_once_for_date(date()).await.unwrap();

        assert_eq!(
            result,
            DailyReviewDeliveryResult {
                attempted: 1,
                delivered: 0,
                skipped: 0,
                failed: 1,
            }
        );
        let review = daily_reviews
            .find_by_user_and_date("7", date())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(review.delivery_error, Some("db error".to_string()));
        assert_eq!(review.delivered_at, None);
    }

    #[tokio::test]
    async fn run_once_delivers_to_multiple_conversations() {
        let sender = FakeSender::succeeding();
        let (worker, daily_reviews, journal_entries, sender) =
            setup(FakeReviewGenerator::succeeding("review text"), sender).await;
        journal_entries
            .store(&entry_for("7", "42", "1", "first"))
            .await
            .unwrap();
        journal_entries
            .store(&entry_for("8", "99", "2", "second"))
            .await
            .unwrap();

        let result = worker.run_once_for_date(date()).await.unwrap();

        assert_eq!(
            result,
            DailyReviewDeliveryResult {
                attempted: 2,
                delivered: 2,
                skipped: 0,
                failed: 0,
            }
        );
        let sent = sender.sent();
        assert_eq!(sent.len(), 2);
        let chat_ids: Vec<&str> = sent.iter().map(|(id, _)| id.as_str()).collect();
        assert!(chat_ids.contains(&"42"));
        assert!(chat_ids.contains(&"99"));
        assert!(
            daily_reviews
                .find_by_user_and_date("7", date())
                .await
                .unwrap()
                .unwrap()
                .delivered_at
                .is_some()
        );
        assert!(
            daily_reviews
                .find_by_user_and_date("8", date())
                .await
                .unwrap()
                .unwrap()
                .delivered_at
                .is_some()
        );
    }
}
