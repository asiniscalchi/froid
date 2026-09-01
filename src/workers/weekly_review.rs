use async_trait::async_trait;
use chrono::{DateTime, Datelike, Duration, NaiveDate, Utc, Weekday};
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
        responses::format_weekly_review_for_week,
        review_preferences::ReviewPreferenceRepository,
        week_review::{
            WeeklyReview, WeeklyReviewDeliveryWorkerConfig,
            repository::{WeeklyReviewRepository, WeeklyReviewRepositoryError},
            service::{WeeklyReviewResult, WeeklyReviewRunner},
        },
    },
    messages::MessageSource,
};

const DAYS_PER_WEEK: i64 = 7;

from_error_string!(ReviewDeliveryError::Storage, WeeklyReviewRepositoryError);

/// Weekly half of [`ReviewDeliveryKind`]: delivers last ISO week's review on
/// the configured kickoff weekday.
pub struct WeeklyReviewDelivery<R> {
    journal_entries: JournalRepository,
    weekly_reviews: WeeklyReviewRepository,
    review_preferences: ReviewPreferenceRepository,
    review_runner: R,
    config: WeeklyReviewDeliveryWorkerConfig,
}

pub type WeeklyReviewDeliveryWorker<R, S> = ReviewDeliveryCycle<WeeklyReviewDelivery<R>, S>;

impl<R, S> WeeklyReviewDeliveryWorker<R, S>
where
    R: WeeklyReviewRunner,
    S: ReviewSender,
{
    pub fn new(
        journal_entries: JournalRepository,
        weekly_reviews: WeeklyReviewRepository,
        review_preferences: ReviewPreferenceRepository,
        review_runner: R,
        sender: S,
        config: WeeklyReviewDeliveryWorkerConfig,
    ) -> Self {
        ReviewDeliveryCycle::from_parts(
            WeeklyReviewDelivery {
                journal_entries,
                weekly_reviews,
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
impl<R> ReviewDeliveryKind for WeeklyReviewDelivery<R>
where
    R: WeeklyReviewRunner + Send + Sync,
{
    type Review = WeeklyReview;

    const WORKER_LABEL: &'static str = "weekly_review_delivery";
    const KIND: &'static str = "weekly review";

    fn due_period(&self, now: DateTime<Utc>) -> Option<NaiveDate> {
        (now.date_naive().weekday() == self.config.kickoff_weekday)
            .then(|| previous_iso_week_monday(now))
    }

    fn log_startup(&self, config: &ReconciliationWorkerConfig) {
        info!(
            enabled = config.enabled,
            interval_seconds = config.interval.as_secs(),
            kickoff_weekday = ?self.config.kickoff_weekday,
            "weekly review delivery worker started"
        );
    }

    async fn targets(
        &self,
        period: NaiveDate,
    ) -> Result<Vec<JournalConversation>, ReviewDeliveryError> {
        if !self.review_preferences.is_enabled().await? {
            return Ok(Vec::new());
        }

        let week_end = period + Duration::days(DAYS_PER_WEEK);
        Ok(self
            .journal_entries
            .conversations_with_entries_in_range(&MessageSource::Telegram, period, week_end)
            .await?)
    }

    async fn run_review(
        &self,
        period: NaiveDate,
    ) -> Result<ReviewStep<WeeklyReview>, ReviewDeliveryError> {
        match self.review_runner.review_week(period).await {
            Ok(WeeklyReviewResult::Existing(review) | WeeklyReviewResult::Generated(review)) => {
                Ok(ReviewStep::Ready(review))
            }
            Ok(WeeklyReviewResult::SparseWeek) => Ok(ReviewStep::Skipped),
            Ok(WeeklyReviewResult::GenerationFailed(failure)) => {
                warn!(
                    week_start = %failure.week_start_date,
                    error = %failure.error_message,
                    "weekly review generation failed during delivery"
                );
                Ok(ReviewStep::Failed)
            }
            Err(error) => {
                warn!(
                    week_start = %period,
                    error = %error,
                    "weekly review runner failed during delivery"
                );
                self.mark_delivery_failed(period, &error.to_string())
                    .await?;
                Ok(ReviewStep::Failed)
            }
        }
    }

    fn delivered_at(review: &WeeklyReview) -> Option<DateTime<Utc>> {
        review.delivered_at
    }

    fn format(review: &WeeklyReview, period: NaiveDate) -> String {
        format_weekly_review_for_week(review, period)
    }

    async fn mark_delivered(&self, period: NaiveDate) -> Result<(), ReviewDeliveryError> {
        Ok(self.weekly_reviews.mark_delivered(period).await?)
    }

    async fn mark_delivery_failed(
        &self,
        period: NaiveDate,
        error: &str,
    ) -> Result<(), ReviewDeliveryError> {
        Ok(self
            .weekly_reviews
            .mark_delivery_failed(period, error)
            .await?)
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
    use chrono::TimeZone;

    use super::*;
    use crate::{
        journal::{
            repository::JournalRepository,
            review::{
                repository::DailyReviewRepository, signals::repository::DailyReviewSignalRepository,
            },
            review_preferences::ReviewPreferenceRepository,
            week_review::{
                WeeklyReviewDeliveryWorkerConfig, generator::fake::FakeWeeklyReviewGenerator,
                repository::WeeklyReviewRepository, service::WeeklyReviewService,
            },
        },
        messages::IncomingMessage,
        workers::{review_delivery::ReviewDeliveryResult, test_support::FakeSender},
    };

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
        ReviewPreferenceRepository,
        FakeSender,
    ) {
        let pool = crate::database::test_pool().await;

        let journal_entries = JournalRepository::new(pool.clone());
        let weekly_reviews = WeeklyReviewRepository::new(pool.clone());
        let daily_reviews = DailyReviewRepository::new(pool.clone());
        let review_preferences = ReviewPreferenceRepository::new(pool.clone());
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
            review_preferences.clone(),
            service,
            sender.clone(),
            config(),
        );

        (
            worker,
            weekly_reviews,
            daily_reviews,
            journal_entries,
            review_preferences,
            sender,
        )
    }

    fn week_start() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 4, 27).unwrap()
    }

    fn day_within_week(offset: i64) -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 4, 27, 12, 0, 0).unwrap() + Duration::days(offset)
    }

    fn entry(conversation_id: &str, message_id: &str, day_offset: i64) -> IncomingMessage {
        IncomingMessage {
            source: MessageSource::Telegram,
            source_conversation_id: conversation_id.to_string(),
            source_message_id: message_id.to_string(),
            text: format!("entry on day {day_offset}"),
            received_at: day_within_week(day_offset),
        }
    }

    async fn seed_three_daily_reviews(daily_reviews: &DailyReviewRepository) {
        for offset in 0..3 {
            let date = week_start() + Duration::days(offset);
            daily_reviews
                .upsert_completed(date, "daily text", "model", "v1")
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

        // Sun 2026-05-03 → previous Mon = 2026-04-20 (still last week relative
        // to its own Monday)
        let now = Utc.with_ymd_and_hms(2026, 5, 3, 23, 0, 0).unwrap();
        assert_eq!(
            previous_iso_week_monday(now),
            NaiveDate::from_ymd_opt(2026, 4, 20).unwrap()
        );
    }

    #[tokio::test]
    async fn run_once_no_ops_on_non_kickoff_weekday() {
        let (worker, _weekly, daily, journal, _review_preferences, sender) = setup(
            FakeWeeklyReviewGenerator::succeeding("week review"),
            FakeSender::succeeding(),
        )
        .await;
        seed_three_daily_reviews(&daily).await;
        journal.store(&entry("42", "1", 0)).await.unwrap();

        // Tuesday — not the configured kickoff day (Monday).
        let tuesday = Utc.with_ymd_and_hms(2026, 5, 5, 9, 0, 0).unwrap();

        let result = worker.run_once(tuesday).await.unwrap();

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
    }

    #[tokio::test]
    async fn run_once_generates_sends_and_marks_last_weeks_review_delivered() {
        let sender = FakeSender::succeeding();
        let (worker, weekly_reviews, daily, journal, _review_preferences, sender) = setup(
            FakeWeeklyReviewGenerator::succeeding("generated week review"),
            sender,
        )
        .await;
        seed_three_daily_reviews(&daily).await;
        journal.store(&entry("42", "1", 0)).await.unwrap();

        // Monday after the target week.
        let monday = Utc.with_ymd_and_hms(2026, 5, 4, 6, 0, 0).unwrap();

        let result = worker.run_once(monday).await.unwrap();

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
        assert!(sent[0].1.contains("Weekly review for week of 2026-04-27"));
        assert!(sent[0].1.contains("generated week review"));

        let stored = weekly_reviews
            .find_by_user_and_week(week_start())
            .await
            .unwrap()
            .unwrap();
        assert!(stored.delivered_at.is_some());
        assert_eq!(stored.delivery_error, None);
    }

    #[tokio::test]
    async fn run_once_skips_delivery_when_user_opted_out_of_reviews() {
        let sender = FakeSender::succeeding();
        let (worker, weekly_reviews, daily, journal, review_preferences, sender) = setup(
            FakeWeeklyReviewGenerator::succeeding("generated week review"),
            sender,
        )
        .await;
        seed_three_daily_reviews(&daily).await;
        journal.store(&entry("42", "1", 0)).await.unwrap();
        review_preferences.set_enabled(false).await.unwrap();

        // Monday after the target week.
        let monday = Utc.with_ymd_and_hms(2026, 5, 4, 6, 0, 0).unwrap();

        let result = worker.run_once(monday).await.unwrap();

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
            weekly_reviews
                .find_by_user_and_week(week_start())
                .await
                .unwrap()
                .is_none(),
            "opted-out user must not trigger review generation either"
        );
    }

    #[tokio::test]
    async fn run_once_skips_when_sparse_week() {
        let sender = FakeSender::succeeding();
        let (worker, _weekly, daily, journal, _review_preferences, sender) =
            setup(FakeWeeklyReviewGenerator::succeeding("ignored"), sender).await;
        // Only two daily reviews — below the threshold of three.
        for offset in 0..2 {
            let date = week_start() + Duration::days(offset);
            daily
                .upsert_completed(date, "text", "m", "v1")
                .await
                .unwrap();
        }
        journal.store(&entry("42", "1", 0)).await.unwrap();

        let result = worker.run_once_for(week_start()).await.unwrap();

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
        let (worker, weekly_reviews, daily, journal, _review_preferences, sender) =
            setup(FakeWeeklyReviewGenerator::succeeding("week review"), sender).await;
        seed_three_daily_reviews(&daily).await;
        journal.store(&entry("42", "1", 0)).await.unwrap();

        let result = worker.run_once_for(week_start()).await.unwrap();

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

        let stored = weekly_reviews
            .find_by_user_and_week(week_start())
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
    async fn run_once_does_not_mark_skipped_delivery_as_delivered() {
        let sender = FakeSender::skipped();
        let (worker, weekly_reviews, daily, journal, _review_preferences, sender) =
            setup(FakeWeeklyReviewGenerator::succeeding("week review"), sender).await;
        seed_three_daily_reviews(&daily).await;
        journal.store(&entry("42", "1", 0)).await.unwrap();

        let result = worker.run_once_for(week_start()).await.unwrap();

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

        let stored = weekly_reviews
            .find_by_user_and_week(week_start())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.delivered_at, None);
        assert_eq!(stored.delivery_error, None);
    }

    #[tokio::test]
    async fn run_once_skips_already_delivered_review() {
        let sender = FakeSender::succeeding();
        let (worker, weekly_reviews, daily, journal, _review_preferences, sender) =
            setup(FakeWeeklyReviewGenerator::succeeding("ignored"), sender).await;
        seed_three_daily_reviews(&daily).await;
        journal.store(&entry("42", "1", 0)).await.unwrap();
        weekly_reviews
            .upsert_completed(week_start(), "existing", "m", "v1", "{}")
            .await
            .unwrap();
        weekly_reviews.mark_delivered(week_start()).await.unwrap();

        let result = worker.run_once_for(week_start()).await.unwrap();

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
    async fn run_once_returns_empty_result_when_no_entries_in_week() {
        let (worker, _, _, _, _, _) = setup(
            FakeWeeklyReviewGenerator::succeeding("irrelevant"),
            FakeSender::succeeding(),
        )
        .await;

        let result = worker.run_once_for(week_start()).await.unwrap();

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
    async fn run_once_counts_as_failed_when_generation_fails() {
        let sender = FakeSender::succeeding();
        let (worker, _weekly, daily, journal, _review_preferences, sender) = setup(
            FakeWeeklyReviewGenerator::failing("generator error"),
            sender,
        )
        .await;
        seed_three_daily_reviews(&daily).await;
        journal.store(&entry("42", "1", 0)).await.unwrap();

        let result = worker.run_once_for(week_start()).await.unwrap();

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

    #[test]
    fn weekday_from_str_accepts_short_and_long_forms() {
        assert_eq!(weekday_from_str("Mon"), Some(Weekday::Mon));
        assert_eq!(weekday_from_str("Monday"), Some(Weekday::Mon));
        assert_eq!(weekday_from_str("FRIDAY"), Some(Weekday::Fri));
        assert_eq!(weekday_from_str("nope"), None);
    }
}
