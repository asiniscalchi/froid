use std::{error::Error, fmt, sync::Arc};

use chrono::NaiveDate;
use tracing::{info, warn};

use crate::journal::{
    extraction::repository::JournalEntryExtractionRepository,
    repository::JournalRepository,
    review::{
        DailyReviewStatus,
        repository::DailyReviewRepository,
        signals::{
            generator::DailyReviewSignalGenerator,
            repository::{DailyReviewSignalRepository, DailyReviewSignalRepositoryError},
            types::DailyReviewSignal,
            validation::validate_signal,
        },
    },
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DailyReviewSignalServiceError {
    NoDailyReview {
        user_id: String,
        review_date: NaiveDate,
    },
    Storage(String),
}

impl fmt::Display for DailyReviewSignalServiceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoDailyReview {
                user_id,
                review_date,
            } => write!(
                f,
                "no completed daily review found for user '{user_id}' on {review_date}"
            ),
            Self::Storage(message) => write!(f, "{message}"),
        }
    }
}

impl Error for DailyReviewSignalServiceError {}

impl From<DailyReviewSignalRepositoryError> for DailyReviewSignalServiceError {
    fn from(error: DailyReviewSignalRepositoryError) -> Self {
        Self::Storage(error.to_string())
    }
}

impl From<sqlx::Error> for DailyReviewSignalServiceError {
    fn from(error: sqlx::Error) -> Self {
        Self::Storage(error.to_string())
    }
}

impl From<crate::journal::review::repository::DailyReviewRepositoryError>
    for DailyReviewSignalServiceError
{
    fn from(error: crate::journal::review::repository::DailyReviewRepositoryError) -> Self {
        Self::Storage(error.to_string())
    }
}

impl From<crate::journal::extraction::repository::JournalEntryExtractionRepositoryError>
    for DailyReviewSignalServiceError
{
    fn from(
        error: crate::journal::extraction::repository::JournalEntryExtractionRepositoryError,
    ) -> Self {
        Self::Storage(error.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DailyReviewSignalResult {
    Generated { count: usize },
    NoDailyReview,
    GenerationFailed { error: String },
}

#[derive(Clone)]
pub struct DailyReviewSignalService {
    daily_reviews: DailyReviewRepository,
    journal_entries: JournalRepository,
    extractions: JournalEntryExtractionRepository,
    signals: DailyReviewSignalRepository,
    generator: Arc<dyn DailyReviewSignalGenerator>,
}

impl DailyReviewSignalService {
    pub fn new<G>(
        daily_reviews: DailyReviewRepository,
        journal_entries: JournalRepository,
        extractions: JournalEntryExtractionRepository,
        signals: DailyReviewSignalRepository,
        generator: G,
    ) -> Self
    where
        G: DailyReviewSignalGenerator + 'static,
    {
        Self {
            daily_reviews,
            journal_entries,
            extractions,
            signals,
            generator: Arc::new(generator),
        }
    }

    pub fn model(&self) -> &str {
        self.generator.model()
    }

    pub fn prompt_version(&self) -> &str {
        self.generator.prompt_version()
    }

    pub async fn generate_signals_for_review(
        &self,
        user_id: &str,
        review_date: NaiveDate,
    ) -> Result<DailyReviewSignalResult, DailyReviewSignalServiceError> {
        let review = self
            .daily_reviews
            .find_by_user_and_date(user_id, review_date)
            .await?;

        let Some(review) = review else {
            return Ok(DailyReviewSignalResult::NoDailyReview);
        };

        if review.status != DailyReviewStatus::Completed {
            return Ok(DailyReviewSignalResult::NoDailyReview);
        }

        let review_text = match &review.review_text {
            Some(text) if !text.trim().is_empty() => text.clone(),
            _ => return Ok(DailyReviewSignalResult::NoDailyReview),
        };

        self.daily_reviews
            .mark_signals_pending(
                review.id,
                self.generator.model(),
                self.generator.prompt_version(),
            )
            .await?;

        let entries = self
            .fetch_entries_with_extractions(user_id, review_date)
            .await?;

        let generation_result = self
            .generator
            .generate_signals(&review_text, &entries)
            .await;

        let output = match generation_result {
            Ok(output) => output,
            Err(error) => {
                let message = error.to_string();
                warn!(
                    daily_review_id = review.id,
                    error = %message,
                    "signal generation failed"
                );
                self.daily_reviews.mark_signals_failed(review.id, &message).await?;
                return Ok(DailyReviewSignalResult::GenerationFailed { error: message });
            }
        };

        let mut valid_candidates = Vec::with_capacity(output.signals.len());
        for candidate in &output.signals {
            match validate_signal(candidate) {
                Ok(()) => valid_candidates.push(candidate.clone()),
                Err(error) => {
                    warn!(
                        daily_review_id = review.id,
                        label = %candidate.label,
                        error = %error,
                        "signal validation failed, skipping signal"
                    );
                }
            }
        }

        let stored = self
            .signals
            .replace_in_transaction(
                review.id,
                user_id,
                review_date,
                &valid_candidates,
                self.generator.model(),
                self.generator.prompt_version(),
            )
            .await;

        match stored {
            Ok(signals) => {
                self.daily_reviews.mark_signals_completed(review.id).await?;
                let count = signals.len();
                info!(
                    daily_review_id = review.id,
                    count, "daily review signals generated and stored"
                );
                Ok(DailyReviewSignalResult::Generated { count })
            }
            Err(error) => {
                let message = error.to_string();
                self.daily_reviews.mark_signals_failed(review.id, &message).await?;
                Err(DailyReviewSignalServiceError::Storage(message))
            }
        }
    }

    pub async fn fetch_signals(
        &self,
        user_id: &str,
        review_date: NaiveDate,
    ) -> Result<Vec<DailyReviewSignal>, DailyReviewSignalServiceError> {
        Ok(self
            .signals
            .find_by_user_and_date(user_id, review_date)
            .await?)
    }

    async fn fetch_entries_with_extractions(
        &self,
        user_id: &str,
        date: NaiveDate,
    ) -> Result<
        Vec<crate::journal::review::JournalEntryWithExtraction>,
        DailyReviewSignalServiceError,
    > {
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
            .map(
                |stored| crate::journal::review::JournalEntryWithExtraction {
                    id: stored.id,
                    entry: stored.entry,
                    extraction: completed_extractions.remove(&stored.id),
                },
            )
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use chrono::{NaiveDate, TimeZone, Utc};
    use sqlx::SqlitePool;

    use super::*;
    use crate::{
        database,
        journal::{
            extraction::NeedStatus,
            repository::JournalRepository,
            review::{
                repository::DailyReviewRepository,
                signals::{
                    generator::fake::FakeSignalGenerator,
                    repository::DailyReviewSignalRepository,
                    types::{DailyReviewSignalCandidate, DailyReviewSignalsOutput, SignalType},
                },
            },
        },
        messages::{IncomingMessage, MessageSource},
    };

    async fn setup(
        generator: FakeSignalGenerator,
    ) -> (
        DailyReviewSignalService,
        DailyReviewRepository,
        JournalRepository,
    ) {
        database::register_sqlite_vec_extension();
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();

        let daily_reviews = DailyReviewRepository::new(pool.clone());
        let journal_entries = JournalRepository::new(pool.clone());
        let extractions =
            crate::journal::extraction::repository::JournalEntryExtractionRepository::new(
                pool.clone(),
            );
        let signals = DailyReviewSignalRepository::new(pool.clone());
        let service = DailyReviewSignalService::new(
            daily_reviews.clone(),
            journal_entries.clone(),
            extractions,
            signals,
            generator,
        );
        (service, daily_reviews, journal_entries)
    }

    fn date() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 4, 28).unwrap()
    }

    fn incoming(user_id: &str, text: &str) -> IncomingMessage {
        IncomingMessage {
            source: MessageSource::Telegram,
            source_conversation_id: "42".to_string(),
            source_message_id: "1".to_string(),
            user_id: user_id.to_string(),
            text: text.to_string(),
            received_at: Utc.with_ymd_and_hms(2026, 4, 28, 10, 0, 0).unwrap(),
        }
    }

    fn theme_signal() -> DailyReviewSignalCandidate {
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

    fn need_signal() -> DailyReviewSignalCandidate {
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

    fn output_with(signals: Vec<DailyReviewSignalCandidate>) -> DailyReviewSignalsOutput {
        DailyReviewSignalsOutput { signals }
    }

    #[tokio::test]
    async fn returns_no_daily_review_when_review_does_not_exist() {
        let (service, _, _) = setup(FakeSignalGenerator::succeeding(output_with(vec![]))).await;

        let result = service
            .generate_signals_for_review("user-1", date())
            .await
            .unwrap();

        assert_eq!(result, DailyReviewSignalResult::NoDailyReview);
    }

    #[tokio::test]
    async fn returns_no_daily_review_when_review_has_failed_status() {
        let (service, reviews, _) =
            setup(FakeSignalGenerator::succeeding(output_with(vec![]))).await;
        reviews
            .upsert_failed("user-1", date(), "model", "v1", "error")
            .await
            .unwrap();

        let result = service
            .generate_signals_for_review("user-1", date())
            .await
            .unwrap();

        assert_eq!(result, DailyReviewSignalResult::NoDailyReview);
    }

    #[tokio::test]
    async fn generates_and_stores_signals_for_completed_review() {
        let generator = FakeSignalGenerator::succeeding(output_with(vec![theme_signal()]));
        let (service, reviews, entries) = setup(generator).await;
        entries
            .store(&incoming("user-1", "entry text"))
            .await
            .unwrap();
        reviews
            .upsert_completed("user-1", date(), "review text", "model", "v1")
            .await
            .unwrap();

        let result = service
            .generate_signals_for_review("user-1", date())
            .await
            .unwrap();

        assert_eq!(result, DailyReviewSignalResult::Generated { count: 1 });

        let signals = service.fetch_signals("user-1", date()).await.unwrap();
        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].signal_type, SignalType::Theme);
        assert_eq!(signals[0].label, "physical appearance");
    }

    #[tokio::test]
    async fn generation_is_idempotent_for_same_daily_review() {
        let generator = FakeSignalGenerator::succeeding(output_with(vec![theme_signal()]));
        let (service, reviews, _) = setup(generator).await;
        reviews
            .upsert_completed("user-1", date(), "review text", "model", "v1")
            .await
            .unwrap();

        service
            .generate_signals_for_review("user-1", date())
            .await
            .unwrap();
        service
            .generate_signals_for_review("user-1", date())
            .await
            .unwrap();

        let signals = service.fetch_signals("user-1", date()).await.unwrap();
        assert_eq!(signals.len(), 1, "second run must not duplicate signals");
    }

    #[tokio::test]
    async fn generation_failure_is_recorded_and_returned() {
        let generator = FakeSignalGenerator::failing("provider down");
        let (service, reviews, _) = setup(generator).await;
        reviews
            .upsert_completed("user-1", date(), "review text", "model", "v1")
            .await
            .unwrap();

        let result = service
            .generate_signals_for_review("user-1", date())
            .await
            .unwrap();

        assert_eq!(
            result,
            DailyReviewSignalResult::GenerationFailed {
                error: "provider down".to_string()
            }
        );

        let signals = service.fetch_signals("user-1", date()).await.unwrap();
        assert!(signals.is_empty());
    }

    #[tokio::test]
    async fn invalid_signals_are_skipped_valid_ones_are_stored() {
        let invalid = DailyReviewSignalCandidate {
            label: "  ".to_string(), // empty label — invalid
            ..theme_signal()
        };
        let generator = FakeSignalGenerator::succeeding(output_with(vec![invalid, need_signal()]));
        let (service, reviews, _) = setup(generator).await;
        reviews
            .upsert_completed("user-1", date(), "review text", "model", "v1")
            .await
            .unwrap();

        let result = service
            .generate_signals_for_review("user-1", date())
            .await
            .unwrap();

        assert_eq!(result, DailyReviewSignalResult::Generated { count: 1 });

        let signals = service.fetch_signals("user-1", date()).await.unwrap();
        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].signal_type, SignalType::Need);
    }

    #[tokio::test]
    async fn signals_are_scoped_by_user_and_date() {
        let generator = FakeSignalGenerator::succeeding(output_with(vec![theme_signal()]));
        let (service, reviews, _) = setup(generator).await;
        reviews
            .upsert_completed("user-1", date(), "user one review", "model", "v1")
            .await
            .unwrap();
        reviews
            .upsert_completed("user-2", date(), "user two review", "model", "v1")
            .await
            .unwrap();

        service
            .generate_signals_for_review("user-1", date())
            .await
            .unwrap();
        service
            .generate_signals_for_review("user-2", date())
            .await
            .unwrap();

        let user_one = service.fetch_signals("user-1", date()).await.unwrap();
        let user_two = service.fetch_signals("user-2", date()).await.unwrap();
        let user_one_other_date = service
            .fetch_signals("user-1", NaiveDate::from_ymd_opt(2026, 4, 29).unwrap())
            .await
            .unwrap();

        assert_eq!(user_one.len(), 1);
        assert_eq!(user_two.len(), 1);
        assert!(user_one_other_date.is_empty());
    }

    #[tokio::test]
    async fn second_run_replaces_first_run_signals() {
        let generator = FakeSignalGenerator::succeeding(output_with(vec![theme_signal()]));
        let (service, reviews, _) = setup(generator.clone()).await;
        reviews
            .upsert_completed("user-1", date(), "review text", "model", "v1")
            .await
            .unwrap();

        service
            .generate_signals_for_review("user-1", date())
            .await
            .unwrap();
        assert_eq!(generator.calls(), 1);

        // Second run replaces with same signals — still only 1 total
        service
            .generate_signals_for_review("user-1", date())
            .await
            .unwrap();
        assert_eq!(generator.calls(), 2);

        let signals = service.fetch_signals("user-1", date()).await.unwrap();
        assert_eq!(signals.len(), 1);
    }
}
