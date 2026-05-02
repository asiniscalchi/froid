use tracing::info;

use crate::{
    journal::extraction::{
        ExtractionBackfillError, ExtractionBackfillResult, ExtractionBackfillService,
        service::JournalEntryExtractionRunner,
    },
    workers::{
        ReconciliationWorker, config::ReconciliationWorkerConfig,
        reconciliation::ReconciliationCycle,
    },
};

pub struct ExtractionCycle<R> {
    backfill_service: ExtractionBackfillService<R>,
}

impl<R> ExtractionCycle<R> {
    pub fn new(backfill_service: ExtractionBackfillService<R>) -> Self {
        Self { backfill_service }
    }
}

impl<R> ReconciliationCycle for ExtractionCycle<R>
where
    R: JournalEntryExtractionRunner + Send + Sync + 'static,
{
    type Outcome = ExtractionBackfillResult;
    type Error = ExtractionBackfillError;

    fn worker_label(&self) -> &'static str {
        "extraction"
    }

    fn log_startup(&self, config: &ReconciliationWorkerConfig) {
        info!(
            enabled = config.enabled,
            model = self.backfill_service.model(),
            prompt_version = self.backfill_service.prompt_version(),
            batch_size = config.batch_size,
            interval_seconds = config.interval.as_secs(),
            "extraction reconciliation worker started",
        );
    }

    fn log_cycle_complete(&self, outcome: &Self::Outcome) {
        if outcome.attempted == 0 {
            return;
        }
        info!(
            attempted = outcome.attempted,
            errored = outcome.errored,
            remaining = outcome.remaining,
            "extraction reconciliation cycle completed",
        );
    }

    async fn run_once(&self, batch_size: u32) -> Result<Self::Outcome, Self::Error> {
        self.backfill_service
            .backfill_missing_or_failed_extractions(batch_size)
            .await
    }
}

pub type ExtractionReconciliationWorker<R> = ReconciliationWorker<ExtractionCycle<R>>;

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use chrono::TimeZone;
    use sqlx::SqlitePool;
    use std::time::Duration;

    use super::*;
    use crate::{
        database,
        journal::{
            extraction::{
                repository::JournalEntryExtractionRepository,
                service::{JournalEntryExtractionRunner, JournalEntryExtractionServiceError},
            },
            repository::JournalRepository,
        },
        messages::{IncomingMessage, MessageSource},
        workers::ReconciliationWorker,
    };

    async fn setup() -> (JournalRepository, JournalEntryExtractionRepository) {
        database::register_sqlite_vec_extension();
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        (
            JournalRepository::new(pool.clone()),
            JournalEntryExtractionRepository::new(pool),
        )
    }

    async fn store_entry(
        journal_repo: &JournalRepository,
        source_message_id: &str,
        text: &str,
        h: u32,
    ) -> i64 {
        let received_at = chrono::Utc.with_ymd_and_hms(2026, 1, 1, h, 0, 0).unwrap();
        let message = IncomingMessage {
            source: MessageSource::Telegram,
            source_conversation_id: "42".to_string(),
            source_message_id: source_message_id.to_string(),
            user_id: "7".to_string(),
            text: text.to_string(),
            received_at,
        };
        journal_repo.store(&message).await.unwrap().unwrap()
    }

    #[derive(Clone)]
    struct FakeRunner;

    #[async_trait]
    impl JournalEntryExtractionRunner for FakeRunner {
        fn model(&self) -> &str {
            "test-extraction-model"
        }

        fn prompt_version(&self) -> &str {
            "entry_extraction_v1"
        }

        async fn extract_entry(
            &self,
            _journal_entry_id: i64,
            _text: &str,
        ) -> Result<(), JournalEntryExtractionServiceError> {
            Ok(())
        }
    }

    fn worker(
        extraction_repo: JournalEntryExtractionRepository,
        batch_size: u32,
    ) -> ExtractionReconciliationWorker<FakeRunner> {
        let backfill = ExtractionBackfillService::new(extraction_repo, FakeRunner);
        ReconciliationWorker::new(
            ExtractionCycle::new(backfill),
            ReconciliationWorkerConfig {
                enabled: true,
                batch_size,
                interval: Duration::from_secs(300),
            },
        )
    }

    #[tokio::test]
    async fn run_once_returns_zero_when_no_entries_are_missing() {
        let (_, extraction_repo) = setup().await;
        let worker = worker(extraction_repo, 20);

        let result = worker.run_once().await.unwrap();

        assert_eq!(
            result,
            ExtractionBackfillResult {
                attempted: 0,
                errored: 0,
                remaining: 0,
            }
        );
    }

    #[tokio::test]
    async fn run_once_processes_missing_entries_up_to_batch_size() {
        let (journal_repo, extraction_repo) = setup().await;
        store_entry(&journal_repo, "1", "first", 10).await;
        store_entry(&journal_repo, "2", "second", 11).await;
        store_entry(&journal_repo, "3", "third", 12).await;
        let worker = worker(extraction_repo, 2);

        let result = worker.run_once().await.unwrap();

        assert_eq!(result.attempted, 2);
        assert_eq!(result.errored, 0);
    }

    #[tokio::test]
    async fn run_once_retries_failed_entries() {
        let (journal_repo, extraction_repo) = setup().await;
        let entry_id = store_entry(&journal_repo, "1", "first", 10).await;
        extraction_repo
            .insert_pending_if_absent(entry_id, "model-a", "v1")
            .await
            .unwrap();
        extraction_repo
            .mark_failed(entry_id, "model-a", "v1", "provider down")
            .await
            .unwrap();
        let worker = worker(extraction_repo, 20);

        let result = worker.run_once().await.unwrap();

        assert_eq!(result.attempted, 1);
        assert_eq!(result.errored, 0);
    }

    #[tokio::test]
    async fn repeated_run_once_does_not_reprocess_completed_entries() {
        let (journal_repo, extraction_repo) = setup().await;
        let entry_id = store_entry(&journal_repo, "1", "first", 10).await;
        extraction_repo
            .insert_pending_if_absent(entry_id, "model-a", "v1")
            .await
            .unwrap();
        extraction_repo
            .mark_completed(
                entry_id,
                r#"{"summary":"ok","domains":[],"emotions":[],"behaviors":[],"needs":[],"possible_patterns":[]}"#,
                "model-a",
                "v1",
            )
            .await
            .unwrap();
        let worker = worker(extraction_repo, 20);

        let first = worker.run_once().await.unwrap();
        let second = worker.run_once().await.unwrap();

        assert_eq!(first.attempted, 0);
        assert_eq!(second.attempted, 0);
    }
}
