use std::{error::Error, fmt};

use chrono::NaiveDate;
use tracing::warn;

use super::{
    repository::{DailyReviewSignalRepository, DailyReviewSignalRepositoryError},
    service::{DailyReviewSignalResult, DailyReviewSignalService, DailyReviewSignalServiceError},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DailyReviewSignalBackfillResult {
    pub attempted: u32,
    pub errored: u32,
    pub remaining: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DailyReviewSignalBackfillError {
    Repository(DailyReviewSignalRepositoryError),
}

impl fmt::Display for DailyReviewSignalBackfillError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Repository(error) => write!(f, "{error}"),
        }
    }
}

impl Error for DailyReviewSignalBackfillError {}

impl From<DailyReviewSignalRepositoryError> for DailyReviewSignalBackfillError {
    fn from(error: DailyReviewSignalRepositoryError) -> Self {
        Self::Repository(error)
    }
}

#[derive(Clone)]
pub struct DailyReviewSignalBackfillService {
    repository: DailyReviewSignalRepository,
    service: DailyReviewSignalService,
}

impl DailyReviewSignalBackfillService {
    pub fn new(repository: DailyReviewSignalRepository, service: DailyReviewSignalService) -> Self {
        Self {
            repository,
            service,
        }
    }

    pub fn model(&self) -> &str {
        self.service.model()
    }

    pub fn prompt_version(&self) -> &str {
        self.service.prompt_version()
    }

    pub async fn backfill_missing_signals(
        &self,
        limit: u32,
    ) -> Result<DailyReviewSignalBackfillResult, DailyReviewSignalBackfillError> {
        let candidates = self
            .repository
            .find_completed_reviews_missing_signals(limit)
            .await?;

        let mut result = DailyReviewSignalBackfillResult {
            attempted: candidates.len() as u32,
            errored: 0,
            remaining: 0,
        };

        for (daily_review_id, user_id, review_date) in candidates {
            if let Err(error) = self
                .process_candidate(&user_id, review_date, daily_review_id)
                .await
            {
                result.errored += 1;
                warn!(
                    daily_review_id,
                    user_id = %user_id,
                    review_date = %review_date,
                    error = %error,
                    "signal backfill candidate failed"
                );
            }
        }

        result.remaining = self
            .repository
            .count_completed_reviews_missing_signals()
            .await?;

        Ok(result)
    }

    async fn process_candidate(
        &self,
        user_id: &str,
        review_date: NaiveDate,
        daily_review_id: i64,
    ) -> Result<(), ProcessCandidateError> {
        let result = self
            .service
            .generate_signals_for_review(user_id, review_date)
            .await
            .map_err(ProcessCandidateError::Service)?;

        match result {
            DailyReviewSignalResult::Generated { .. } | DailyReviewSignalResult::NoDailyReview => {
                Ok(())
            }
            DailyReviewSignalResult::GenerationFailed { error } => {
                Err(ProcessCandidateError::GenerationFailed {
                    daily_review_id,
                    error,
                })
            }
        }
    }
}

#[derive(Debug)]
enum ProcessCandidateError {
    Service(DailyReviewSignalServiceError),
    GenerationFailed { daily_review_id: i64, error: String },
}

impl fmt::Display for ProcessCandidateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Service(error) => write!(f, "service error: {error}"),
            Self::GenerationFailed {
                daily_review_id,
                error,
            } => write!(
                f,
                "generation failed for daily_review_id={daily_review_id}: {error}"
            ),
        }
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
            extraction::repository::JournalEntryExtractionRepository,
            repository::JournalRepository,
            review::{
                repository::DailyReviewRepository,
                signals::{
                    generator::fake::FakeSignalGenerator,
                    repository::DailyReviewSignalRepository,
                    service::DailyReviewSignalService,
                    types::{DailyReviewSignalCandidate, DailyReviewSignalsOutput, SignalType},
                },
            },
        },
        messages::{IncomingMessage, MessageSource},
    };

    async fn setup(
        generator: FakeSignalGenerator,
    ) -> (
        DailyReviewSignalBackfillService,
        DailyReviewRepository,
        JournalRepository,
        DailyReviewSignalRepository,
    ) {
        database::register_sqlite_vec_extension();
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();

        let daily_reviews = DailyReviewRepository::new(pool.clone());
        let journal_entries = JournalRepository::new(pool.clone());
        let extractions = JournalEntryExtractionRepository::new(pool.clone());
        let signals = DailyReviewSignalRepository::new(pool.clone());
        let service = DailyReviewSignalService::new(
            daily_reviews.clone(),
            journal_entries.clone(),
            extractions,
            signals.clone(),
            generator,
        );
        let backfill = DailyReviewSignalBackfillService::new(signals.clone(), service);
        (backfill, daily_reviews, journal_entries, signals)
    }

    fn date() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 4, 28).unwrap()
    }

    fn theme_output() -> DailyReviewSignalsOutput {
        DailyReviewSignalsOutput {
            signals: vec![DailyReviewSignalCandidate {
                signal_type: SignalType::Theme,
                label: "routine".to_string(),
                status: None,
                valence: None,
                strength: 0.8,
                confidence: 0.9,
                evidence: "Mentions structure.".to_string(),
            }],
        }
    }

    async fn store_entry(journal_repo: &JournalRepository, msg_id: &str, text: &str) {
        let message = IncomingMessage {
            source: MessageSource::Telegram,
            source_conversation_id: "42".to_string(),
            source_message_id: msg_id.to_string(),
            user_id: "user-1".to_string(),
            text: text.to_string(),
            received_at: chrono::Utc::now(),
        };
        journal_repo.store(&message).await.unwrap();
    }

    #[tokio::test]
    async fn returns_zero_when_no_candidates() {
        let (backfill, _, _, _) = setup(FakeSignalGenerator::succeeding(theme_output())).await;

        let result = backfill.backfill_missing_signals(10).await.unwrap();

        assert_eq!(
            result,
            DailyReviewSignalBackfillResult {
                attempted: 0,
                errored: 0,
                remaining: 0,
            }
        );
    }

    #[tokio::test]
    async fn processes_completed_review_without_signals() {
        let generator = FakeSignalGenerator::succeeding(theme_output());
        let (backfill, reviews, entries, _) = setup(generator.clone()).await;
        store_entry(&entries, "1", "entry text").await;
        reviews
            .upsert_completed("user-1", date(), "review text", "model", "v1")
            .await
            .unwrap();

        let result = backfill.backfill_missing_signals(10).await.unwrap();

        assert_eq!(result.attempted, 1);
        assert_eq!(result.errored, 0);
        assert_eq!(result.remaining, 0);
        assert_eq!(generator.calls(), 1);
    }

    #[tokio::test]
    async fn counts_errored_when_generation_fails() {
        let generator = FakeSignalGenerator::failing("provider down");
        let (backfill, reviews, _, _) = setup(generator).await;
        reviews
            .upsert_completed("user-1", date(), "review text", "model", "v1")
            .await
            .unwrap();

        let result = backfill.backfill_missing_signals(10).await.unwrap();

        assert_eq!(result.attempted, 1);
        assert_eq!(result.errored, 1);
    }

    #[tokio::test]
    async fn does_not_reprocess_reviews_with_completed_signals() {
        let generator = FakeSignalGenerator::succeeding(theme_output());
        let (backfill, reviews, entries, _) = setup(generator.clone()).await;
        store_entry(&entries, "1", "entry text").await;
        reviews
            .upsert_completed("user-1", date(), "review text", "model", "v1")
            .await
            .unwrap();

        backfill.backfill_missing_signals(10).await.unwrap();
        let second = backfill.backfill_missing_signals(10).await.unwrap();

        assert_eq!(second.attempted, 0);
        assert_eq!(generator.calls(), 1);
    }
}
