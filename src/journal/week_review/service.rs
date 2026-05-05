use std::{collections::HashMap, error::Error, fmt, sync::Arc};

use chrono::{Duration, NaiveDate};

use crate::journal::{
    review::{
        repository::{DailyReviewRepository, DailyReviewRepositoryError},
        signals::repository::{DailyReviewSignalRepository, DailyReviewSignalRepositoryError},
    },
    week_review::{
        DailyReviewSlice, WeeklyReview, WeeklyReviewInput, WeeklyReviewStatus,
        generator::WeeklyReviewGenerator,
        repository::{WeeklyReviewRepository, WeeklyReviewRepositoryError},
    },
};

pub const DAYS_PER_WEEK: i64 = 7;
pub const DEFAULT_MIN_DAILY_REVIEWS: usize = 3;

const EMPTY_REVIEW_ERROR: &str = "weekly review generator returned an empty review";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WeeklyReviewServiceError {
    Storage(String),
}

impl fmt::Display for WeeklyReviewServiceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Storage(message) => write!(f, "{message}"),
        }
    }
}

impl Error for WeeklyReviewServiceError {}

impl From<sqlx::Error> for WeeklyReviewServiceError {
    fn from(error: sqlx::Error) -> Self {
        Self::Storage(error.to_string())
    }
}

impl From<WeeklyReviewRepositoryError> for WeeklyReviewServiceError {
    fn from(error: WeeklyReviewRepositoryError) -> Self {
        Self::Storage(error.to_string())
    }
}

impl From<DailyReviewRepositoryError> for WeeklyReviewServiceError {
    fn from(error: DailyReviewRepositoryError) -> Self {
        Self::Storage(error.to_string())
    }
}

impl From<DailyReviewSignalRepositoryError> for WeeklyReviewServiceError {
    fn from(error: DailyReviewSignalRepositoryError) -> Self {
        Self::Storage(error.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WeeklyReviewResult {
    Existing(WeeklyReview),
    Generated(WeeklyReview),
    SparseWeek,
    GenerationFailed(WeeklyReviewFailure),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WeeklyReviewFailure {
    pub user_id: String,
    pub week_start_date: NaiveDate,
    pub model: String,
    pub prompt_version: String,
    pub error_message: String,
}

#[derive(Clone)]
pub struct WeeklyReviewService {
    weekly_reviews: WeeklyReviewRepository,
    daily_reviews: DailyReviewRepository,
    signals: DailyReviewSignalRepository,
    generator: Arc<dyn WeeklyReviewGenerator>,
    min_daily_reviews: usize,
}

#[async_trait::async_trait]
pub trait WeeklyReviewRunner: Send + Sync {
    async fn review_week(
        &self,
        user_id: &str,
        week_start: NaiveDate,
    ) -> Result<WeeklyReviewResult, WeeklyReviewServiceError>;

    async fn fetch_review(
        &self,
        user_id: &str,
        week_start: NaiveDate,
    ) -> Result<Option<WeeklyReview>, WeeklyReviewServiceError>;
}

impl WeeklyReviewService {
    pub fn new<G>(
        weekly_reviews: WeeklyReviewRepository,
        daily_reviews: DailyReviewRepository,
        signals: DailyReviewSignalRepository,
        generator: G,
        min_daily_reviews: usize,
    ) -> Self
    where
        G: WeeklyReviewGenerator + 'static,
    {
        Self {
            weekly_reviews,
            daily_reviews,
            signals,
            generator: Arc::new(generator),
            min_daily_reviews,
        }
    }

    pub async fn review_week(
        &self,
        user_id: &str,
        week_start: NaiveDate,
    ) -> Result<WeeklyReviewResult, WeeklyReviewServiceError> {
        let existing = self
            .weekly_reviews
            .find_by_user_and_week(user_id, week_start)
            .await?;

        if let Some(review) = &existing
            && review.status == WeeklyReviewStatus::Completed
            && review
                .review_text
                .as_deref()
                .is_some_and(|text| !text.trim().is_empty())
        {
            return Ok(WeeklyReviewResult::Existing(review.clone()));
        }

        let week_end = week_start + Duration::days(DAYS_PER_WEEK);
        let dailies = self
            .daily_reviews
            .fetch_completed_in_range(user_id, week_start, week_end)
            .await?;

        if dailies.len() < self.min_daily_reviews {
            return Ok(WeeklyReviewResult::SparseWeek);
        }

        let signals = self
            .signals
            .find_by_user_in_range(user_id, week_start, week_end)
            .await?;

        let mut signals_by_date: HashMap<NaiveDate, Vec<_>> = HashMap::new();
        for signal in signals {
            signals_by_date
                .entry(signal.review_date)
                .or_default()
                .push(signal);
        }

        let days: Vec<DailyReviewSlice> = dailies
            .into_iter()
            .filter_map(|daily| {
                daily.review_text.map(|text| DailyReviewSlice {
                    date: daily.review_date,
                    review_text: text,
                    signals: signals_by_date
                        .remove(&daily.review_date)
                        .unwrap_or_default(),
                })
            })
            .collect();

        let input = WeeklyReviewInput { week_start, days };

        let model = self.generator.model();
        let prompt_version = self.generator.prompt_version();

        match self.generator.generate_weekly_review(&input).await {
            Ok(review_text) => {
                let trimmed = review_text.trim();
                if trimmed.is_empty() {
                    return self
                        .store_failed(
                            user_id,
                            week_start,
                            model,
                            prompt_version,
                            EMPTY_REVIEW_ERROR,
                        )
                        .await;
                }

                let inputs_snapshot = serde_json::to_string(&input).map_err(|err| {
                    WeeklyReviewServiceError::Storage(format!(
                        "failed to serialize inputs snapshot: {err}"
                    ))
                })?;

                let review = self
                    .weekly_reviews
                    .upsert_completed(
                        user_id,
                        week_start,
                        trimmed,
                        model,
                        prompt_version,
                        &inputs_snapshot,
                    )
                    .await?;
                Ok(WeeklyReviewResult::Generated(review))
            }
            Err(error) => {
                let message = error.to_string();
                self.store_failed(user_id, week_start, model, prompt_version, &message)
                    .await
            }
        }
    }

    pub async fn fetch_review(
        &self,
        user_id: &str,
        week_start: NaiveDate,
    ) -> Result<Option<WeeklyReview>, WeeklyReviewServiceError> {
        let review = self
            .weekly_reviews
            .find_by_user_and_week(user_id, week_start)
            .await?;
        Ok(review.filter(|r| {
            r.status == WeeklyReviewStatus::Completed
                && r.review_text
                    .as_deref()
                    .is_some_and(|t| !t.trim().is_empty())
        }))
    }

    async fn store_failed(
        &self,
        user_id: &str,
        week_start: NaiveDate,
        model: &str,
        prompt_version: &str,
        error_message: &str,
    ) -> Result<WeeklyReviewResult, WeeklyReviewServiceError> {
        self.weekly_reviews
            .upsert_failed(user_id, week_start, model, prompt_version, error_message)
            .await?;
        Ok(WeeklyReviewResult::GenerationFailed(WeeklyReviewFailure {
            user_id: user_id.to_string(),
            week_start_date: week_start,
            model: model.to_string(),
            prompt_version: prompt_version.to_string(),
            error_message: error_message.to_string(),
        }))
    }
}

#[async_trait::async_trait]
impl WeeklyReviewRunner for WeeklyReviewService {
    async fn review_week(
        &self,
        user_id: &str,
        week_start: NaiveDate,
    ) -> Result<WeeklyReviewResult, WeeklyReviewServiceError> {
        WeeklyReviewService::review_week(self, user_id, week_start).await
    }

    async fn fetch_review(
        &self,
        user_id: &str,
        week_start: NaiveDate,
    ) -> Result<Option<WeeklyReview>, WeeklyReviewServiceError> {
        WeeklyReviewService::fetch_review(self, user_id, week_start).await
    }
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;
    use sqlx::SqlitePool;

    use super::*;
    use crate::{
        database,
        journal::{
            extraction::NeedStatus,
            review::{
                repository::DailyReviewRepository,
                signals::{
                    repository::DailyReviewSignalRepository,
                    types::{DailyReviewSignalCandidate, SignalType},
                },
            },
            week_review::{
                generator::{WeeklyReviewGenerationError, fake::FakeWeeklyReviewGenerator},
                repository::WeeklyReviewRepository,
            },
        },
    };

    async fn setup(
        generator: FakeWeeklyReviewGenerator,
    ) -> (
        WeeklyReviewService,
        WeeklyReviewRepository,
        DailyReviewRepository,
        DailyReviewSignalRepository,
        FakeWeeklyReviewGenerator,
    ) {
        database::register_sqlite_vec_extension();
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();

        let weekly_reviews = WeeklyReviewRepository::new(pool.clone());
        let daily_reviews = DailyReviewRepository::new(pool.clone());
        let signals = DailyReviewSignalRepository::new(pool);

        let service = WeeklyReviewService::new(
            weekly_reviews.clone(),
            daily_reviews.clone(),
            signals.clone(),
            generator.clone(),
            DEFAULT_MIN_DAILY_REVIEWS,
        );

        (service, weekly_reviews, daily_reviews, signals, generator)
    }

    fn week_start() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 4, 27).unwrap()
    }

    fn day(offset: i64) -> NaiveDate {
        week_start() + Duration::days(offset)
    }

    fn theme_candidate() -> DailyReviewSignalCandidate {
        DailyReviewSignalCandidate {
            signal_type: SignalType::Theme,
            label: "physical appearance".to_string(),
            status: None,
            valence: None,
            strength: 0.8,
            confidence: 0.9,
            evidence: "Review mentions training and diet.".to_string(),
        }
    }

    fn need_candidate() -> DailyReviewSignalCandidate {
        DailyReviewSignalCandidate {
            signal_type: SignalType::Need,
            label: "control".to_string(),
            status: Some(NeedStatus::Unmet),
            valence: None,
            strength: 0.7,
            confidence: 0.85,
            evidence: "Review notes repeated attempts to regain control.".to_string(),
        }
    }

    async fn seed_completed_daily(
        daily_reviews: &DailyReviewRepository,
        user_id: &str,
        date: NaiveDate,
        text: &str,
    ) -> i64 {
        daily_reviews
            .upsert_completed(user_id, date, text, "model", "v1")
            .await
            .unwrap()
            .id
    }

    fn generated(result: WeeklyReviewResult) -> WeeklyReview {
        match result {
            WeeklyReviewResult::Generated(review) => review,
            other => panic!("expected generated review, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn returns_existing_completed_review_without_calling_generator() {
        let (service, weekly_reviews, _daily, _signals, generator) =
            setup(FakeWeeklyReviewGenerator::succeeding("new")).await;
        let existing = weekly_reviews
            .upsert_completed("user-1", week_start(), "existing", "model", "v1", "{}")
            .await
            .unwrap();

        let result = service.review_week("user-1", week_start()).await.unwrap();

        assert_eq!(result, WeeklyReviewResult::Existing(existing));
        assert_eq!(generator.calls(), 0);
    }

    #[tokio::test]
    async fn sparse_week_skips_generation_when_below_min_daily_reviews() {
        let (service, _weekly, daily, _signals, generator) =
            setup(FakeWeeklyReviewGenerator::succeeding("ignored")).await;
        seed_completed_daily(&daily, "user-1", day(0), "monday").await;
        seed_completed_daily(&daily, "user-1", day(2), "wednesday").await;

        let result = service.review_week("user-1", week_start()).await.unwrap();

        assert_eq!(result, WeeklyReviewResult::SparseWeek);
        assert_eq!(generator.calls(), 0);
    }

    #[tokio::test]
    async fn generates_review_when_threshold_met() {
        let (service, weekly_reviews, daily, _signals, generator) =
            setup(FakeWeeklyReviewGenerator::succeeding("week review")).await;
        seed_completed_daily(&daily, "user-1", day(0), "monday").await;
        seed_completed_daily(&daily, "user-1", day(1), "tuesday").await;
        seed_completed_daily(&daily, "user-1", day(2), "wednesday").await;

        let review = generated(service.review_week("user-1", week_start()).await.unwrap());

        assert_eq!(review.review_text, Some("week review".to_string()));
        assert_eq!(review.status, WeeklyReviewStatus::Completed);
        assert_eq!(review.model, "fake-weekly-review-model");
        assert_eq!(review.prompt_version, "fake-weekly-prompt-v1");
        assert_eq!(generator.calls(), 1);

        let stored = weekly_reviews
            .find_by_user_and_week("user-1", week_start())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored, review);

        let inputs = generator.inputs_seen();
        assert_eq!(inputs.len(), 1);
        let dates: Vec<_> = inputs[0].days.iter().map(|d| d.date).collect();
        assert_eq!(dates, vec![day(0), day(1), day(2)]);
    }

    #[tokio::test]
    async fn passes_signals_grouped_by_day_to_generator() {
        let (service, _weekly, daily, signals_repo, generator) =
            setup(FakeWeeklyReviewGenerator::succeeding("text")).await;
        let monday_review = seed_completed_daily(&daily, "user-1", day(0), "monday").await;
        let tuesday_review = seed_completed_daily(&daily, "user-1", day(1), "tuesday").await;
        seed_completed_daily(&daily, "user-1", day(2), "wednesday").await;

        signals_repo
            .replace_in_transaction(
                monday_review,
                "user-1",
                day(0),
                &[theme_candidate()],
                "model",
                "v1",
            )
            .await
            .unwrap();
        signals_repo
            .replace_in_transaction(
                tuesday_review,
                "user-1",
                day(1),
                &[theme_candidate(), need_candidate()],
                "model",
                "v1",
            )
            .await
            .unwrap();

        service.review_week("user-1", week_start()).await.unwrap();

        let inputs = generator.inputs_seen();
        let monday_slice = inputs[0].days.iter().find(|d| d.date == day(0)).unwrap();
        let tuesday_slice = inputs[0].days.iter().find(|d| d.date == day(1)).unwrap();
        let wednesday_slice = inputs[0].days.iter().find(|d| d.date == day(2)).unwrap();
        assert_eq!(monday_slice.signals.len(), 1);
        assert_eq!(tuesday_slice.signals.len(), 2);
        assert_eq!(wednesday_slice.signals.len(), 0);
    }

    #[tokio::test]
    async fn excludes_dailies_and_signals_outside_the_week() {
        let (service, _weekly, daily, signals_repo, generator) =
            setup(FakeWeeklyReviewGenerator::succeeding("text")).await;
        let prior_review = seed_completed_daily(&daily, "user-1", day(-1), "previous sunday").await;
        seed_completed_daily(&daily, "user-1", day(0), "monday").await;
        seed_completed_daily(&daily, "user-1", day(1), "tuesday").await;
        seed_completed_daily(&daily, "user-1", day(2), "wednesday").await;
        seed_completed_daily(&daily, "user-1", day(7), "next monday").await;

        signals_repo
            .replace_in_transaction(
                prior_review,
                "user-1",
                day(-1),
                &[theme_candidate()],
                "m",
                "v1",
            )
            .await
            .unwrap();

        service.review_week("user-1", week_start()).await.unwrap();

        let inputs = generator.inputs_seen();
        let dates: Vec<_> = inputs[0].days.iter().map(|d| d.date).collect();
        assert_eq!(dates, vec![day(0), day(1), day(2)]);
        for day_slice in &inputs[0].days {
            assert!(day_slice.signals.is_empty());
        }
    }

    #[tokio::test]
    async fn blank_generated_review_is_persisted_as_failure() {
        let (service, weekly_reviews, daily, _signals, generator) =
            setup(FakeWeeklyReviewGenerator::succeeding("   \n\t")).await;
        for offset in 0..3 {
            seed_completed_daily(&daily, "user-1", day(offset), "text").await;
        }

        let result = service.review_week("user-1", week_start()).await.unwrap();

        assert_eq!(
            result,
            WeeklyReviewResult::GenerationFailed(WeeklyReviewFailure {
                user_id: "user-1".to_string(),
                week_start_date: week_start(),
                model: "fake-weekly-review-model".to_string(),
                prompt_version: "fake-weekly-prompt-v1".to_string(),
                error_message: EMPTY_REVIEW_ERROR.to_string(),
            })
        );
        assert_eq!(generator.calls(), 1);

        let stored = weekly_reviews
            .find_by_user_and_week("user-1", week_start())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.status, WeeklyReviewStatus::Failed);
        assert_eq!(stored.review_text, None);
        assert_eq!(stored.error_message, Some(EMPTY_REVIEW_ERROR.to_string()));
    }

    #[tokio::test]
    async fn generation_failure_is_persisted_and_returned_as_domain_result() {
        let (service, weekly_reviews, daily, _signals, _generator) =
            setup(FakeWeeklyReviewGenerator::failing("provider down")).await;
        for offset in 0..3 {
            seed_completed_daily(&daily, "user-1", day(offset), "text").await;
        }

        let result = service.review_week("user-1", week_start()).await.unwrap();

        assert_eq!(
            result,
            WeeklyReviewResult::GenerationFailed(WeeklyReviewFailure {
                user_id: "user-1".to_string(),
                week_start_date: week_start(),
                model: "fake-weekly-review-model".to_string(),
                prompt_version: "fake-weekly-prompt-v1".to_string(),
                error_message: "provider down".to_string(),
            })
        );

        let stored = weekly_reviews
            .find_by_user_and_week("user-1", week_start())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.status, WeeklyReviewStatus::Failed);
        assert_eq!(stored.error_message, Some("provider down".to_string()));
    }

    #[tokio::test]
    async fn failed_review_can_be_retried_and_updated_to_completed() {
        let generator = FakeWeeklyReviewGenerator::new(vec![
            Err(WeeklyReviewGenerationError::new("provider down")),
            Ok("retry review".to_string()),
        ]);
        let (service, weekly_reviews, daily, _signals, generator) = setup(generator).await;
        for offset in 0..3 {
            seed_completed_daily(&daily, "user-1", day(offset), "text").await;
        }

        assert!(matches!(
            service.review_week("user-1", week_start()).await.unwrap(),
            WeeklyReviewResult::GenerationFailed(_)
        ));
        let failed = weekly_reviews
            .find_by_user_and_week("user-1", week_start())
            .await
            .unwrap()
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));

        let completed = generated(service.review_week("user-1", week_start()).await.unwrap());

        assert_eq!(generator.calls(), 2);
        assert_eq!(completed.id, failed.id);
        assert_eq!(completed.created_at, failed.created_at);
        assert!(completed.updated_at > failed.updated_at);
        assert_eq!(completed.status, WeeklyReviewStatus::Completed);
        assert_eq!(completed.review_text, Some("retry review".to_string()));
        assert_eq!(completed.error_message, None);
    }

    #[tokio::test]
    async fn blank_existing_completed_review_is_regenerated() {
        let (service, weekly_reviews, daily, _signals, generator) =
            setup(FakeWeeklyReviewGenerator::succeeding("regenerated")).await;
        for offset in 0..3 {
            seed_completed_daily(&daily, "user-1", day(offset), "text").await;
        }
        let existing = weekly_reviews
            .upsert_completed("user-1", week_start(), "", "old-model", "v0", "{}")
            .await
            .unwrap();

        let review = generated(service.review_week("user-1", week_start()).await.unwrap());

        assert_eq!(generator.calls(), 1);
        assert_eq!(review.id, existing.id);
        assert_eq!(review.review_text, Some("regenerated".to_string()));
        assert_eq!(review.status, WeeklyReviewStatus::Completed);
    }

    #[tokio::test]
    async fn same_week_reviews_share_single_user_scope() {
        let (service, _weekly, daily, _signals, generator) =
            setup(FakeWeeklyReviewGenerator::new(vec![
                Ok("user one review".to_string()),
                Ok("user two review".to_string()),
            ]))
            .await;
        for offset in 0..3 {
            seed_completed_daily(&daily, "user-1", day(offset), "user one").await;
            seed_completed_daily(&daily, "user-2", day(offset), "user two").await;
        }

        let one = generated(service.review_week("user-1", week_start()).await.unwrap());
        let two = match service.review_week("user-2", week_start()).await.unwrap() {
            WeeklyReviewResult::Existing(review) => review,
            other => panic!("expected existing review, got {other:?}"),
        };

        assert_eq!(one.review_text, Some("user one review".to_string()));
        assert_eq!(two.review_text, Some("user one review".to_string()));
        assert_eq!(one.user_id, two.user_id);
        assert_eq!(generator.calls(), 1);
    }

    #[tokio::test]
    async fn fetch_review_returns_completed_review() {
        let (service, weekly_reviews, _daily, _signals, _generator) =
            setup(FakeWeeklyReviewGenerator::succeeding("any")).await;
        weekly_reviews
            .upsert_completed("user-1", week_start(), "review text", "model", "v1", "{}")
            .await
            .unwrap();

        let result = service.fetch_review("user-1", week_start()).await.unwrap();

        assert_eq!(result.unwrap().review_text, Some("review text".to_string()));
    }

    #[tokio::test]
    async fn fetch_review_returns_none_when_no_review_exists() {
        let (service, _weekly, _daily, _signals, _generator) =
            setup(FakeWeeklyReviewGenerator::succeeding("any")).await;

        let result = service.fetch_review("user-1", week_start()).await.unwrap();

        assert!(result.is_none());
    }

    #[tokio::test]
    async fn fetch_review_returns_none_for_failed_review() {
        let (service, weekly_reviews, _daily, _signals, _generator) =
            setup(FakeWeeklyReviewGenerator::succeeding("any")).await;
        weekly_reviews
            .upsert_failed("user-1", week_start(), "model", "v1", "provider down")
            .await
            .unwrap();

        let result = service.fetch_review("user-1", week_start()).await.unwrap();

        assert!(result.is_none());
    }

    #[tokio::test]
    async fn generated_review_persists_serialized_inputs_snapshot() {
        let (service, weekly_reviews, daily, signals_repo, _generator) =
            setup(FakeWeeklyReviewGenerator::succeeding("week review")).await;
        let monday_review = seed_completed_daily(&daily, "user-1", day(0), "monday text").await;
        seed_completed_daily(&daily, "user-1", day(1), "tuesday text").await;
        seed_completed_daily(&daily, "user-1", day(2), "wednesday text").await;
        signals_repo
            .replace_in_transaction(
                monday_review,
                "user-1",
                day(0),
                &[theme_candidate()],
                "model",
                "v1",
            )
            .await
            .unwrap();

        service.review_week("user-1", week_start()).await.unwrap();

        let stored = weekly_reviews
            .find_by_user_and_week("user-1", week_start())
            .await
            .unwrap()
            .unwrap();
        let snapshot = stored.inputs_snapshot.expect("snapshot must be persisted");
        assert!(snapshot.contains("\"week_start\":\"2026-04-27\""));
        assert!(snapshot.contains("\"review_text\":\"monday text\""));
        assert!(snapshot.contains("\"review_text\":\"tuesday text\""));
        assert!(snapshot.contains("\"label\":\"physical appearance\""));
    }
}
