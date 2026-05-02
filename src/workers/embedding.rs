use tracing::info;

use crate::{
    journal::embedding::{
        BackfillResult, Embedder, EmbeddingBackfillError, EmbeddingBackfillService, EmbeddingIndex,
    },
    workers::{
        ReconciliationWorker, config::ReconciliationWorkerConfig,
        reconciliation::ReconciliationCycle,
    },
};

pub struct EmbeddingCycle<ID, I, E> {
    backfill_service: EmbeddingBackfillService<ID, I, E>,
}

impl<ID, I, E> EmbeddingCycle<ID, I, E> {
    pub fn new(backfill_service: EmbeddingBackfillService<ID, I, E>) -> Self {
        Self { backfill_service }
    }
}

impl<ID, I, E> ReconciliationCycle for EmbeddingCycle<ID, I, E>
where
    I: EmbeddingIndex<ID> + Send + Sync + 'static,
    E: Embedder + Send + Sync + 'static,
    ID: Send + Sync + Copy + 'static,
{
    type Outcome = BackfillResult;
    type Error = EmbeddingBackfillError;

    fn worker_label(&self) -> &'static str {
        "embedding"
    }

    fn log_startup(&self, config: &ReconciliationWorkerConfig) {
        info!(
            enabled = config.enabled,
            model = self.backfill_service.model(),
            dimensions = self.backfill_service.dimensions(),
            batch_size = config.batch_size,
            interval_seconds = config.interval.as_secs(),
            "embedding reconciliation worker started",
        );
    }

    fn log_cycle_complete(&self, outcome: &Self::Outcome) {
        if outcome.attempted == 0 {
            return;
        }
        info!(
            attempted = outcome.attempted,
            created = outcome.created,
            failed = outcome.failed,
            remaining = outcome.remaining,
            "embedding reconciliation cycle completed",
        );
    }

    async fn run_once(&self, batch_size: u32) -> Result<Self::Outcome, Self::Error> {
        self.backfill_service
            .backfill_missing_or_failed_embeddings(batch_size)
            .await
    }
}

pub type EmbeddingReconciliationWorker<ID, I, E> = ReconciliationWorker<EmbeddingCycle<ID, I, E>>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        database,
        journal::{
            embedding::{
                EmbedderError, Embedding, EmbeddingBackfillService, SUPPORTED_EMBEDDING_DIMENSIONS,
                SqliteEmbeddingRepository,
            },
            repository::JournalRepository,
        },
        messages::{IncomingMessage, MessageSource},
        workers::ReconciliationWorker,
    };
    use async_trait::async_trait;
    use chrono::{TimeZone, Utc};
    use sqlx::SqlitePool;
    use std::time::Duration;

    async fn setup() -> (JournalRepository, SqliteEmbeddingRepository) {
        database::register_sqlite_vec_extension();
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        (
            JournalRepository::new(pool.clone()),
            SqliteEmbeddingRepository::new(pool),
        )
    }

    fn incoming(
        source_message_id: &str,
        text: &str,
        received_at: chrono::DateTime<Utc>,
    ) -> IncomingMessage {
        IncomingMessage {
            source: MessageSource::Telegram,
            source_conversation_id: "42".to_string(),
            source_message_id: source_message_id.to_string(),
            user_id: "7".to_string(),
            text: text.to_string(),
            received_at,
        }
    }

    fn at(h: u32, m: u32) -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 4, 28, h, m, 0).unwrap()
    }

    async fn store_entry(
        journal_repository: &JournalRepository,
        source_message_id: &str,
        text: &str,
        received_at: chrono::DateTime<Utc>,
    ) -> i64 {
        journal_repository
            .store(&incoming(source_message_id, text, received_at))
            .await
            .unwrap();

        sqlx::query_scalar(
            "SELECT id FROM journal_entries WHERE source = 'telegram' AND source_message_id = ?",
        )
        .bind(source_message_id)
        .fetch_one(journal_repository.pool())
        .await
        .unwrap()
    }

    const TEST_EMBEDDING_MODEL: &str = "test-model-v1";
    const TEST_EMBEDDING_DIMENSIONS: usize = SUPPORTED_EMBEDDING_DIMENSIONS;

    #[derive(Debug, Clone)]
    struct FakeEmbedder;

    #[async_trait]
    impl Embedder for FakeEmbedder {
        fn model(&self) -> &str {
            TEST_EMBEDDING_MODEL
        }

        fn dimensions(&self) -> usize {
            TEST_EMBEDDING_DIMENSIONS
        }

        async fn embed(&self, text: &str) -> Result<Embedding, EmbedderError> {
            if text == "fail embedding" {
                return Err(EmbedderError::Provider(text.to_string()));
            }
            Embedding::new(
                vec![text.len() as f32; TEST_EMBEDDING_DIMENSIONS],
                TEST_EMBEDDING_DIMENSIONS,
            )
        }
    }

    fn worker_with_batch_size(
        embedding_repository: SqliteEmbeddingRepository,
        batch_size: u32,
    ) -> EmbeddingReconciliationWorker<i64, SqliteEmbeddingRepository, FakeEmbedder> {
        let backfill_service = EmbeddingBackfillService::new(embedding_repository, FakeEmbedder);
        ReconciliationWorker::new(
            EmbeddingCycle::new(backfill_service),
            ReconciliationWorkerConfig {
                enabled: true,
                batch_size,
                interval: Duration::from_secs(300),
            },
        )
    }

    #[tokio::test]
    async fn run_once_returns_zero_when_no_entries_are_missing() {
        let (_, embedding_repository) = setup().await;
        let worker = worker_with_batch_size(embedding_repository, 20);

        let result = worker.run_once().await.unwrap();

        assert_eq!(
            result,
            BackfillResult {
                attempted: 0,
                created: 0,
                failed: 0,
                remaining: 0,
            }
        );
    }

    #[tokio::test]
    async fn run_once_processes_missing_entries_up_to_batch_size() {
        let (journal_repository, embedding_repository) = setup().await;
        store_entry(&journal_repository, "1", "first", at(10, 0)).await;
        store_entry(&journal_repository, "2", "second", at(11, 0)).await;
        store_entry(&journal_repository, "3", "third", at(12, 0)).await;
        let worker = worker_with_batch_size(embedding_repository.clone(), 2);

        let result = worker.run_once().await.unwrap();

        assert_eq!(result.attempted, 2);
        assert_eq!(result.created, 2);
        assert_eq!(result.failed, 0);
        assert_eq!(
            embedding_repository
                .count_entries_missing_or_failed_embedding(TEST_EMBEDDING_MODEL)
                .await
                .unwrap(),
            1
        );
    }

    #[tokio::test]
    async fn run_once_processes_all_entries_when_fewer_than_batch_size() {
        let (journal_repository, embedding_repository) = setup().await;
        store_entry(&journal_repository, "1", "first", at(10, 0)).await;
        store_entry(&journal_repository, "2", "second", at(11, 0)).await;
        let worker = worker_with_batch_size(embedding_repository.clone(), 20);

        let result = worker.run_once().await.unwrap();

        assert_eq!(result.attempted, 2);
        assert_eq!(result.created, 2);
        assert_eq!(result.failed, 0);
        assert_eq!(
            embedding_repository
                .count_entries_missing_or_failed_embedding(TEST_EMBEDDING_MODEL)
                .await
                .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn repeated_run_once_calls_do_not_create_duplicates() {
        let (journal_repository, embedding_repository) = setup().await;
        store_entry(&journal_repository, "1", "first", at(10, 0)).await;
        let worker = worker_with_batch_size(embedding_repository, 20);

        let first = worker.run_once().await.unwrap();
        let second = worker.run_once().await.unwrap();

        assert_eq!(
            first,
            BackfillResult {
                attempted: 1,
                created: 1,
                failed: 0,
                remaining: 0,
            }
        );
        assert_eq!(
            second,
            BackfillResult {
                attempted: 0,
                created: 0,
                failed: 0,
                remaining: 0,
            }
        );
    }

    #[tokio::test]
    async fn run_once_records_failure_and_continues_for_remaining_entries() {
        let (journal_repository, embedding_repository) = setup().await;
        store_entry(&journal_repository, "1", "fail embedding", at(10, 0)).await;
        store_entry(&journal_repository, "2", "second", at(11, 0)).await;
        let worker = worker_with_batch_size(embedding_repository, 20);

        let result = worker.run_once().await.unwrap();

        assert_eq!(
            result,
            BackfillResult {
                attempted: 2,
                created: 1,
                failed: 1,
                remaining: 1,
            }
        );
    }
}
