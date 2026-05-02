mod backfill;
mod config;
mod provider;
mod repository;
mod types;
mod worker_config;

pub use backfill::{BackfillResult, EmbeddingBackfillError, EmbeddingBackfillService};
pub use config::EmbeddingConfig;
pub use provider::RigOpenAiEmbedder;
pub use repository::{
    EmbeddingIndex, EmbeddingRepositoryError, PendingEmbeddingCounter, SqliteEmbeddingRepository,
};
pub use types::{
    Embedder, EmbedderError, Embedding, EmbeddingCandidate, EmbeddingSearchResult,
};
pub use worker_config::EmbeddingWorkerConfig;

pub const DEFAULT_EMBEDDING_MODEL: &str = "text-embedding-3-small";
pub const SUPPORTED_EMBEDDING_DIMENSIONS: usize = 1536;

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use chrono::{TimeZone, Utc};
    use sqlx::SqlitePool;

    use super::*;
    use crate::{
        database,
        journal::repository::JournalRepository,
        messages::{IncomingMessage, MessageSource},
    };

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

    #[derive(Debug, Clone)]
    struct StorageFailingIndex {
        inner: SqliteEmbeddingRepository,
        failing_journal_entry_id: i64,
    }

    #[async_trait]
    impl EmbeddingIndex<i64> for StorageFailingIndex {
        async fn store_embedding(
            &self,
            journal_entry_id: i64,
            embedding_model: &str,
            embedding_dim: usize,
            embedding: &Embedding,
        ) -> Result<bool, EmbeddingRepositoryError> {
            if journal_entry_id == self.failing_journal_entry_id {
                return Err(EmbeddingRepositoryError::Database(
                    "forced storage failure".to_string(),
                ));
            }

            self.inner
                .store_embedding(journal_entry_id, embedding_model, embedding_dim, embedding)
                .await
                .map_err(Into::into)
        }

        async fn record_embedding_failure(
            &self,
            journal_entry_id: i64,
            embedding_model: &str,
            error_message: &str,
        ) -> Result<(), EmbeddingRepositoryError> {
            self.inner
                .record_embedding_failure(journal_entry_id, embedding_model, error_message)
                .await
                .map_err(Into::into)
        }

        async fn delete_failed_embedding(
            &self,
            journal_entry_id: i64,
            embedding_model: &str,
        ) -> Result<bool, EmbeddingRepositoryError> {
            self.inner
                .delete_failed_embedding(journal_entry_id, embedding_model)
                .await
                .map_err(Into::into)
        }

        async fn find_entries_missing_or_failed_embedding(
            &self,
            embedding_model: &str,
            limit: u32,
        ) -> Result<Vec<EmbeddingCandidate<i64>>, EmbeddingRepositoryError> {
            self.inner
                .find_entries_missing_or_failed_embedding(embedding_model, limit)
                .await
                .map_err(Into::into)
        }

        async fn count_entries_missing_or_failed_embedding(
            &self,
            embedding_model: &str,
        ) -> Result<u32, EmbeddingRepositoryError> {
            self.inner
                .count_entries_missing_or_failed_embedding(embedding_model)
                .await
                .map_err(Into::into)
        }

        async fn search_for_user(
            &self,
            user_id: &str,
            embedding: &Embedding,
            embedding_model: &str,
            limit: usize,
        ) -> Result<Vec<EmbeddingSearchResult<i64>>, EmbeddingRepositoryError> {
            self.inner
                .search_for_user(user_id, embedding, embedding_model, limit)
                .await
                .map_err(Into::into)
        }
    }

    #[tokio::test]
    async fn backfill_generates_missing_embeddings_with_limit_oldest_first() {
        let (journal_repository, embedding_repository) = setup().await;
        let first = store_entry(&journal_repository, "1", "first", at(10, 0)).await;
        let second = store_entry(&journal_repository, "2", "second", at(11, 0)).await;
        let third = store_entry(&journal_repository, "3", "third", at(12, 0)).await;

        let service = EmbeddingBackfillService::new(embedding_repository.clone(), FakeEmbedder);

        let result = service
            .backfill_missing_or_failed_embeddings(2)
            .await
            .unwrap();

        assert_eq!(
            result,
            BackfillResult {
                attempted: 2,
                created: 2,
                failed: 0,
                remaining: 1,
            }
        );
        assert!(
            embedding_repository
                .has_embedding(first, TEST_EMBEDDING_MODEL)
                .await
                .unwrap()
        );
        let first_stored = embedding_repository
            .stored_embedding(first, TEST_EMBEDDING_MODEL)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(first_stored.embedding_model, TEST_EMBEDDING_MODEL);
        assert_eq!(first_stored.embedding_dim, TEST_EMBEDDING_DIMENSIONS as i64);
        assert!(
            embedding_repository
                .has_embedding(second, TEST_EMBEDDING_MODEL)
                .await
                .unwrap()
        );
        assert!(
            !embedding_repository
                .has_embedding(third, TEST_EMBEDDING_MODEL)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn repeated_backfill_does_not_create_duplicates() {
        let (journal_repository, embedding_repository) = setup().await;
        store_entry(&journal_repository, "1", "first", at(10, 0)).await;
        store_entry(&journal_repository, "2", "second", at(11, 0)).await;

        let service = EmbeddingBackfillService::new(embedding_repository.clone(), FakeEmbedder);

        let first_result = service
            .backfill_missing_or_failed_embeddings(50)
            .await
            .unwrap();
        let second_result = service
            .backfill_missing_or_failed_embeddings(50)
            .await
            .unwrap();

        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM journal_entry_embedding_metadata")
                .fetch_one(&embedding_repository.pool)
                .await
                .unwrap();

        assert_eq!(
            first_result,
            BackfillResult {
                attempted: 2,
                created: 2,
                failed: 0,
                remaining: 0,
            }
        );
        assert_eq!(
            second_result,
            BackfillResult {
                attempted: 0,
                created: 0,
                failed: 0,
                remaining: 0,
            }
        );
        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn backfill_continues_after_embedder_failure() {
        let (journal_repository, embedding_repository) = setup().await;
        store_entry(&journal_repository, "1", "fail embedding", at(10, 0)).await;
        let second = store_entry(&journal_repository, "2", "second", at(11, 0)).await;

        let service = EmbeddingBackfillService::new(embedding_repository.clone(), FakeEmbedder);

        let result = service
            .backfill_missing_or_failed_embeddings(50)
            .await
            .unwrap();

        assert_eq!(
            result,
            BackfillResult {
                attempted: 2,
                created: 1,
                failed: 1,
                remaining: 1,
            }
        );
        assert!(
            embedding_repository
                .has_embedding(second, TEST_EMBEDDING_MODEL)
                .await
                .unwrap()
        );
        assert_eq!(
            embedding_repository
                .count_entries_missing_or_failed_embedding(TEST_EMBEDDING_MODEL)
                .await
                .unwrap(),
            1
        );
    }

    #[tokio::test]
    async fn backfill_continues_after_storage_failure() {
        let (journal_repository, embedding_repository) = setup().await;
        let first = store_entry(&journal_repository, "1", "first", at(10, 0)).await;
        let second = store_entry(&journal_repository, "2", "second", at(11, 0)).await;
        let failing_index = StorageFailingIndex {
            inner: embedding_repository.clone(),
            failing_journal_entry_id: first,
        };
        let service = EmbeddingBackfillService::new(failing_index, FakeEmbedder);

        let result = service
            .backfill_missing_or_failed_embeddings(50)
            .await
            .unwrap();

        assert_eq!(
            result,
            BackfillResult {
                attempted: 2,
                created: 1,
                failed: 1,
                remaining: 1,
            }
        );
        // first has a failed row (storage error recorded), so still counts as pending
        assert_eq!(
            embedding_repository
                .count_entries_missing_or_failed_embedding(TEST_EMBEDDING_MODEL)
                .await
                .unwrap(),
            1
        );
        assert!(
            embedding_repository
                .stored_embedding(first, TEST_EMBEDDING_MODEL)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            embedding_repository
                .has_embedding(second, TEST_EMBEDDING_MODEL)
                .await
                .unwrap()
        );
    }
}
