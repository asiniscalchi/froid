use async_trait::async_trait;
use chrono::{DateTime, Datelike, Duration, NaiveDate, Utc, Weekday};
use teloxide::{prelude::*, types::ChatId};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::{
    journal::{
        repository::JournalRepository,
        responses::format_weekly_review_for_week,
        week_review::{
            WeeklyReviewDeliveryWorkerConfig,
            repository::{WeeklyReviewRepository, WeeklyReviewRepositoryError},
            service::{WeeklyReviewResult, WeeklyReviewRunner, WeeklyReviewServiceError},
        },
    },
    messages::MessageSource,
};

const DAYS_PER_WEEK: i64 = 7;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WeeklyReviewDeliveryResult {
    pub attempted: usize,
    pub delivered: usize,
    pub skipped: usize,
    pub failed: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WeeklyReviewDeliveryWorkerError {
    Storage(String),
}

impl std::fmt::Display for WeeklyReviewDeliveryWorkerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Storage(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for WeeklyReviewDeliveryWorkerError {}

impl From<sqlx::Error> for WeeklyReviewDeliveryWorkerError {
    fn from(error: sqlx::Error) -> Self {
        Self::Storage(error.to_string())
    }
}

impl From<WeeklyReviewRepositoryError> for WeeklyReviewDeliveryWorkerError {
    fn from(error: WeeklyReviewRepositoryError) -> Self {
        Self::Storage(error.to_string())
    }
}

#[async_trait]
pub trait WeeklyReviewSender: Send + Sync {
    async fn send_weekly_review(
        &self,
        source_conversation_id: &str,
        text: &str,
    ) -> Result<(), String>;
}

#[derive(Clone)]
pub struct TelegramWeeklyReviewSender {
    bot: Bot,
}

impl TelegramWeeklyReviewSender {
    pub fn new(bot_token: String) -> Self {
        Self {
            bot: Bot::new(bot_token),
        }
    }
}

#[async_trait]
impl WeeklyReviewSender for TelegramWeeklyReviewSender {
    async fn send_weekly_review(
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

pub struct WeeklyReviewDeliveryWorker<R, S> {
    journal_entries: JournalRepository,
    weekly_reviews: WeeklyReviewRepository,
    review_runner: R,
    sender: S,
    config: WeeklyReviewDeliveryWorkerConfig,
}

impl<R, S> WeeklyReviewDeliveryWorker<R, S>
where
    R: WeeklyReviewRunner,
    S: WeeklyReviewSender,
{
    pub fn new(
        journal_entries: JournalRepository,
        weekly_reviews: WeeklyReviewRepository,
        review_runner: R,
        sender: S,
        config: WeeklyReviewDeliveryWorkerConfig,
    ) -> Self {
        Self {
            journal_entries,
            weekly_reviews,
            review_runner,
            sender,
            config,
        }
    }

    pub async fn run_once(
        &self,
        now: DateTime<Utc>,
    ) -> Result<WeeklyReviewDeliveryResult, WeeklyReviewDeliveryWorkerError> {
        if now.date_naive().weekday() != self.config.kickoff_weekday {
            return Ok(WeeklyReviewDeliveryResult {
                attempted: 0,
                delivered: 0,
                skipped: 0,
                failed: 0,
            });
        }

        let week_start = previous_iso_week_monday(now);
        self.run_once_for_week(week_start).await
    }

    pub async fn run_once_for_week(
        &self,
        week_start: NaiveDate,
    ) -> Result<WeeklyReviewDeliveryResult, WeeklyReviewDeliveryWorkerError> {
        let week_end = week_start + Duration::days(DAYS_PER_WEEK);
        let targets = self
            .journal_entries
            .conversations_with_entries_in_range(&MessageSource::Telegram, week_start, week_end)
            .await?;

        let mut result = WeeklyReviewDeliveryResult {
            attempted: targets.len(),
            delivered: 0,
            skipped: 0,
            failed: 0,
        };

        for target in targets {
            let review = match self
                .review_runner
                .review_week(&target.user_id, week_start)
                .await
            {
                Ok(
                    WeeklyReviewResult::Existing(review) | WeeklyReviewResult::Generated(review),
                ) => review,
                Ok(WeeklyReviewResult::SparseWeek) => {
                    result.skipped += 1;
                    continue;
                }
                Ok(WeeklyReviewResult::GenerationFailed(failure)) => {
                    warn!(
                        user_id = %failure.user_id,
                        week_start = %failure.week_start_date,
                        error = %failure.error_message,
                        "weekly review generation failed during delivery"
                    );
                    result.failed += 1;
                    continue;
                }
                Err(error) => {
                    self.record_review_runner_error(&target.user_id, week_start, error)
                        .await?;
                    result.failed += 1;
                    continue;
                }
            };

            if review.delivered_at.is_some() {
                result.skipped += 1;
                continue;
            }

            let text = format_weekly_review_for_week(&review, week_start);
            match self
                .sender
                .send_weekly_review(&target.source_conversation_id, &text)
                .await
            {
                Ok(()) => {
                    self.weekly_reviews
                        .mark_delivered(&target.user_id, week_start)
                        .await?;
                    result.delivered += 1;
                }
                Err(error) => {
                    self.weekly_reviews
                        .mark_delivery_failed(&target.user_id, week_start, &error)
                        .await?;
                    warn!(
                        user_id = %target.user_id,
                        source_conversation_id = %target.source_conversation_id,
                        week_start = %week_start,
                        error = %error,
                        "failed to deliver weekly review"
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
        week_start: NaiveDate,
        error: WeeklyReviewServiceError,
    ) -> Result<(), WeeklyReviewDeliveryWorkerError> {
        warn!(
            user_id = %user_id,
            week_start = %week_start,
            error = %error,
            "weekly review runner failed during delivery"
        );
        self.weekly_reviews
            .mark_delivery_failed(user_id, week_start, &error.to_string())
            .await?;
        Ok(())
    }

    pub async fn run_forever(self, shutdown: CancellationToken) {
        info!(
            enabled = self.config.enabled,
            interval_seconds = self.config.interval.as_secs(),
            kickoff_weekday = ?self.config.kickoff_weekday,
            "weekly review delivery worker started"
        );

        loop {
            if shutdown.is_cancelled() {
                return;
            }

            match self.run_once(Utc::now()).await {
                Ok(result) => {
                    if result.attempted > 0 && (result.delivered > 0 || result.failed > 0) {
                        info!(
                            attempted = result.attempted,
                            delivered = result.delivered,
                            skipped = result.skipped,
                            failed = result.failed,
                            "weekly review delivery cycle completed"
                        );
                    }
                }
                Err(err) => {
                    error!(error = %err, "weekly review delivery cycle failed");
                }
            }

            tokio::select! {
                _ = tokio::time::sleep(self.config.interval) => {}
                _ = shutdown.cancelled() => return,
            }
        }
    }
}

fn previous_iso_week_monday(now: DateTime<Utc>) -> NaiveDate {
    let today = now.date_naive();
    let days_since_monday = today.weekday().num_days_from_monday() as i64;
    let this_monday = today - Duration::days(days_since_monday);
    this_monday - Duration::days(DAYS_PER_WEEK)
}

pub fn weekday_from_str(value: &str) -> Option<Weekday> {
    match value.trim().to_ascii_lowercase().as_str() {
        "mon" | "monday" => Some(Weekday::Mon),
        "tue" | "tuesday" => Some(Weekday::Tue),
        "wed" | "wednesday" => Some(Weekday::Wed),
        "thu" | "thursday" => Some(Weekday::Thu),
        "fri" | "friday" => Some(Weekday::Fri),
        "sat" | "saturday" => Some(Weekday::Sat),
        "sun" | "sunday" => Some(Weekday::Sun),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use chrono::TimeZone;
    use sqlx::SqlitePool;

    use super::*;
    use crate::{
        database,
        journal::{
            repository::JournalRepository,
            review::{
                repository::DailyReviewRepository, signals::repository::DailyReviewSignalRepository,
            },
            week_review::{
                WeeklyReviewDeliveryWorkerConfig, generator::fake::FakeWeeklyReviewGenerator,
                repository::WeeklyReviewRepository, service::WeeklyReviewService,
            },
        },
        messages::IncomingMessage,
    };

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
    impl WeeklyReviewSender for FakeSender {
        async fn send_weekly_review(
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

    fn config() -> WeeklyReviewDeliveryWorkerConfig {
        WeeklyReviewDeliveryWorkerConfig {
            enabled: true,
            interval: std::time::Duration::from_secs(3600),
            kickoff_weekday: Weekday::Mon,
            min_daily_reviews: 3,
        }
    }

    async fn setup(
        generator: FakeWeeklyReviewGenerator,
        sender: FakeSender,
    ) -> (
        WeeklyReviewDeliveryWorker<WeeklyReviewService, FakeSender>,
        WeeklyReviewRepository,
        DailyReviewRepository,
        JournalRepository,
        FakeSender,
    ) {
        database::register_sqlite_vec_extension();
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();

        let journal_entries = JournalRepository::new(pool.clone());
        let weekly_reviews = WeeklyReviewRepository::new(pool.clone());
        let daily_reviews = DailyReviewRepository::new(pool.clone());
        let signals = DailyReviewSignalRepository::new(pool.clone());

        let service = WeeklyReviewService::new(
            weekly_reviews.clone(),
            daily_reviews.clone(),
            signals,
            generator,
            3,
        );

        let worker = WeeklyReviewDeliveryWorker::new(
            journal_entries.clone(),
            weekly_reviews.clone(),
            service,
            sender.clone(),
            config(),
        );

        (
            worker,
            weekly_reviews,
            daily_reviews,
            journal_entries,
            sender,
        )
    }

    fn week_start() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 4, 27).unwrap()
    }

    fn day_within_week(offset: i64) -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 4, 27, 12, 0, 0).unwrap() + Duration::days(offset)
    }

    fn entry(
        user_id: &str,
        conversation_id: &str,
        message_id: &str,
        day_offset: i64,
    ) -> IncomingMessage {
        IncomingMessage {
            source: MessageSource::Telegram,
            source_conversation_id: conversation_id.to_string(),
            source_message_id: message_id.to_string(),
            user_id: user_id.to_string(),
            text: format!("entry on day {day_offset}"),
            received_at: day_within_week(day_offset),
        }
    }

    async fn seed_three_daily_reviews(daily_reviews: &DailyReviewRepository, user_id: &str) {
        for offset in 0..3 {
            let date = week_start() + Duration::days(offset);
            daily_reviews
                .upsert_completed(user_id, date, "daily text", "model", "v1")
                .await
                .unwrap();
        }
    }

    #[test]
    fn previous_iso_week_monday_returns_last_monday_for_any_weekday() {
        // Tue 2026-05-05 → previous Mon = 2026-04-27
        let now = Utc.with_ymd_and_hms(2026, 5, 5, 12, 0, 0).unwrap();
        assert_eq!(
            previous_iso_week_monday(now),
            NaiveDate::from_ymd_opt(2026, 4, 27).unwrap()
        );

        // Mon 2026-05-04 (kickoff day) → previous Mon = 2026-04-27
        let now = Utc.with_ymd_and_hms(2026, 5, 4, 0, 5, 0).unwrap();
        assert_eq!(
            previous_iso_week_monday(now),
            NaiveDate::from_ymd_opt(2026, 4, 27).unwrap()
        );

        // Sun 2026-05-10 → previous Mon = 2026-04-27 (still last week relative to its own Monday)
        let now = Utc.with_ymd_and_hms(2026, 5, 3, 23, 0, 0).unwrap();
        assert_eq!(
            previous_iso_week_monday(now),
            NaiveDate::from_ymd_opt(2026, 4, 20).unwrap()
        );
    }

    #[tokio::test]
    async fn run_once_no_ops_on_non_kickoff_weekday() {
        let (worker, _weekly, daily, journal, sender) = setup(
            FakeWeeklyReviewGenerator::succeeding("week review"),
            FakeSender::succeeding(),
        )
        .await;
        seed_three_daily_reviews(&daily, "7").await;
        journal.store(&entry("7", "42", "1", 0)).await.unwrap();

        // Tuesday — not the configured kickoff day (Monday).
        let tuesday = Utc.with_ymd_and_hms(2026, 5, 5, 9, 0, 0).unwrap();

        let result = worker.run_once(tuesday).await.unwrap();

        assert_eq!(
            result,
            WeeklyReviewDeliveryResult {
                attempted: 0,
                delivered: 0,
                skipped: 0,
                failed: 0,
            }
        );
        assert!(sender.sent().is_empty());
    }

    #[tokio::test]
    async fn run_once_generates_sends_and_marks_last_weeks_review_delivered() {
        let sender = FakeSender::succeeding();
        let (worker, weekly_reviews, daily, journal, sender) = setup(
            FakeWeeklyReviewGenerator::succeeding("generated week review"),
            sender,
        )
        .await;
        seed_three_daily_reviews(&daily, "7").await;
        journal.store(&entry("7", "42", "1", 0)).await.unwrap();

        // Monday after the target week.
        let monday = Utc.with_ymd_and_hms(2026, 5, 4, 6, 0, 0).unwrap();

        let result = worker.run_once(monday).await.unwrap();

        assert_eq!(
            result,
            WeeklyReviewDeliveryResult {
                attempted: 1,
                delivered: 1,
                skipped: 0,
                failed: 0,
            }
        );
        let sent = sender.sent();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].0, "42");
        assert!(sent[0].1.contains("Weekly review for week of 2026-04-27"));
        assert!(sent[0].1.contains("generated week review"));

        let stored = weekly_reviews
            .find_by_user_and_week("7", week_start())
            .await
            .unwrap()
            .unwrap();
        assert!(stored.delivered_at.is_some());
        assert_eq!(stored.delivery_error, None);
    }

    #[tokio::test]
    async fn run_once_skips_when_sparse_week() {
        let sender = FakeSender::succeeding();
        let (worker, _weekly, daily, journal, sender) =
            setup(FakeWeeklyReviewGenerator::succeeding("ignored"), sender).await;
        // Only two daily reviews — below the threshold of three.
        for offset in 0..2 {
            let date = week_start() + Duration::days(offset);
            daily
                .upsert_completed("7", date, "text", "m", "v1")
                .await
                .unwrap();
        }
        journal.store(&entry("7", "42", "1", 0)).await.unwrap();

        let result = worker.run_once_for_week(week_start()).await.unwrap();

        assert_eq!(
            result,
            WeeklyReviewDeliveryResult {
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
        let (worker, weekly_reviews, daily, journal, sender) =
            setup(FakeWeeklyReviewGenerator::succeeding("week review"), sender).await;
        seed_three_daily_reviews(&daily, "7").await;
        journal.store(&entry("7", "42", "1", 0)).await.unwrap();

        let result = worker.run_once_for_week(week_start()).await.unwrap();

        assert_eq!(
            result,
            WeeklyReviewDeliveryResult {
                attempted: 1,
                delivered: 0,
                skipped: 0,
                failed: 1,
            }
        );
        assert_eq!(sender.sent().len(), 1);

        let stored = weekly_reviews
            .find_by_user_and_week("7", week_start())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.delivered_at, None);
        assert_eq!(
            stored.delivery_error,
            Some("telegram unavailable".to_string())
        );
    }

    #[tokio::test]
    async fn run_once_skips_already_delivered_review() {
        let sender = FakeSender::succeeding();
        let (worker, weekly_reviews, daily, journal, sender) =
            setup(FakeWeeklyReviewGenerator::succeeding("ignored"), sender).await;
        seed_three_daily_reviews(&daily, "7").await;
        journal.store(&entry("7", "42", "1", 0)).await.unwrap();
        weekly_reviews
            .upsert_completed("7", week_start(), "existing", "m", "v1", "{}")
            .await
            .unwrap();
        weekly_reviews
            .mark_delivered("7", week_start())
            .await
            .unwrap();

        let result = worker.run_once_for_week(week_start()).await.unwrap();

        assert_eq!(
            result,
            WeeklyReviewDeliveryResult {
                attempted: 1,
                delivered: 0,
                skipped: 1,
                failed: 0,
            }
        );
        assert!(sender.sent().is_empty());
    }

    #[tokio::test]
    async fn run_once_returns_empty_result_when_no_entries_in_week() {
        let (worker, _, _, _, _) = setup(
            FakeWeeklyReviewGenerator::succeeding("irrelevant"),
            FakeSender::succeeding(),
        )
        .await;

        let result = worker.run_once_for_week(week_start()).await.unwrap();

        assert_eq!(
            result,
            WeeklyReviewDeliveryResult {
                attempted: 0,
                delivered: 0,
                skipped: 0,
                failed: 0,
            }
        );
    }

    #[tokio::test]
    async fn run_once_counts_as_failed_when_generation_fails() {
        let sender = FakeSender::succeeding();
        let (worker, _weekly, daily, journal, sender) = setup(
            FakeWeeklyReviewGenerator::failing("generator error"),
            sender,
        )
        .await;
        seed_three_daily_reviews(&daily, "7").await;
        journal.store(&entry("7", "42", "1", 0)).await.unwrap();

        let result = worker.run_once_for_week(week_start()).await.unwrap();

        assert_eq!(
            result,
            WeeklyReviewDeliveryResult {
                attempted: 1,
                delivered: 0,
                skipped: 0,
                failed: 1,
            }
        );
        assert!(sender.sent().is_empty());
    }

    #[test]
    fn weekday_from_str_accepts_short_and_long_forms() {
        assert_eq!(weekday_from_str("Mon"), Some(Weekday::Mon));
        assert_eq!(weekday_from_str("Monday"), Some(Weekday::Mon));
        assert_eq!(weekday_from_str("FRIDAY"), Some(Weekday::Fri));
        assert_eq!(weekday_from_str("nope"), None);
    }
}
