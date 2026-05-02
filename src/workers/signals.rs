use tracing::{error, info};

use crate::journal::review::signals::{
    backfill::{
        DailyReviewSignalBackfillError, DailyReviewSignalBackfillResult,
        DailyReviewSignalBackfillService,
    },
    worker_config::DailyReviewSignalWorkerConfig,
};

pub struct DailyReviewSignalReconciliationWorker {
    backfill_service: DailyReviewSignalBackfillService,
    config: DailyReviewSignalWorkerConfig,
}

impl DailyReviewSignalReconciliationWorker {
    pub fn new(
        backfill_service: DailyReviewSignalBackfillService,
        config: DailyReviewSignalWorkerConfig,
    ) -> Self {
        Self {
            backfill_service,
            config,
        }
    }

    pub async fn run_once(
        &self,
    ) -> Result<DailyReviewSignalBackfillResult, DailyReviewSignalBackfillError> {
        self.backfill_service
            .backfill_missing_signals(self.config.batch_size)
            .await
    }

    pub async fn run_forever(self) {
        info!(
            model = self.backfill_service.model(),
            prompt_version = self.backfill_service.prompt_version(),
            batch_size = self.config.batch_size,
            interval_seconds = self.config.interval.as_secs(),
            "signal reconciliation worker started"
        );

        loop {
            match self.run_once().await {
                Ok(result) => {
                    info!(
                        attempted = result.attempted,
                        errored = result.errored,
                        remaining = result.remaining,
                        "signal reconciliation cycle completed"
                    );
                }
                Err(err) => {
                    error!(error = %err, "signal reconciliation cycle failed");
                }
            }
            tokio::time::sleep(self.config.interval).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;
    use sqlx::SqlitePool;
    use std::time::Duration;

    use super::*;
    use crate::{
        database,
        journal::{
            extraction::repository::JournalEntryExtractionRepository,
            repository::JournalRepository,
            review::{
                repository::DailyReviewRepository,
                signals::{
                    backfill::DailyReviewSignalBackfillService,
                    generator::fake::FakeSignalGenerator,
                    repository::{DailyReviewSignalRepository},
                    service::DailyReviewSignalService,
                    types::{DailyReviewSignalCandidate, DailyReviewSignalsOutput, SignalType},
                    worker_config::DailyReviewSignalWorkerConfig,
                },
            },
        },
        messages::{IncomingMessage, MessageSource},
    };

    async fn setup(
        generator: FakeSignalGenerator,
    ) -> (
        DailyReviewSignalReconciliationWorker,
        DailyReviewRepository,
        JournalRepository,
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
        let backfill = DailyReviewSignalBackfillService::new(signals, service);
        let worker = DailyReviewSignalReconciliationWorker::new(
            backfill,
            DailyReviewSignalWorkerConfig {
                enabled: true,
                batch_size: 20,
                interval: Duration::from_secs(300),
            },
        );
        (worker, daily_reviews, journal_entries)
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
    async fn run_once_returns_zero_when_no_reviews_need_signals() {
        let (worker, _, _) = setup(FakeSignalGenerator::succeeding(theme_output())).await;

        let result = worker.run_once().await.unwrap();

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
    async fn run_once_processes_completed_review_missing_signals() {
        let generator = FakeSignalGenerator::succeeding(theme_output());
        let (worker, reviews, entries) = setup(generator.clone()).await;
        store_entry(&entries, "1", "entry text").await;
        reviews
            .upsert_completed("user-1", date(), "review text", "model", "v1")
            .await
            .unwrap();

        let result = worker.run_once().await.unwrap();

        assert_eq!(result.attempted, 1);
        assert_eq!(result.errored, 0);
        assert_eq!(result.remaining, 0);
        assert_eq!(generator.calls(), 1);
    }

    #[tokio::test]
    async fn repeated_run_once_does_not_reprocess_completed_reviews() {
        let generator = FakeSignalGenerator::succeeding(theme_output());
        let (worker, reviews, entries) = setup(generator.clone()).await;
        store_entry(&entries, "1", "entry text").await;
        reviews
            .upsert_completed("user-1", date(), "review text", "model", "v1")
            .await
            .unwrap();

        worker.run_once().await.unwrap();
        let second = worker.run_once().await.unwrap();

        assert_eq!(second.attempted, 0);
        assert_eq!(generator.calls(), 1);
    }
}
