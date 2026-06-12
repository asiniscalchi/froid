mod backfill;
mod config;
mod provider;
mod repository;
mod types;

pub use backfill::{BackfillResult, EmbeddingBackfillError, EmbeddingBackfillService};
pub use config::EmbeddingConfig;
pub use provider::RigOpenAiEmbedder;
pub use repository::{
    EmbeddingIndex, EmbeddingRepositoryError, EmbeddingSchema, PendingEmbeddingCounter,
    SqliteEmbeddingRepository, SqliteVectorIndex,
};
pub use types::{Embedder, EmbedderError, Embedding, EmbeddingCandidate, EmbeddingSearchResult};

pub const DEFAULT_EMBEDDING_MODEL: &str = "text-embedding-3-small";
pub const SUPPORTED_EMBEDDING_DIMENSIONS: usize = 1536;

/// Shared test doubles for code that depends on [`Embedder`] /
/// [`EmbeddingIndex`].
#[cfg(test)]
pub(crate) mod test_support {
    use async_trait::async_trait;
    use chrono::NaiveDate;

    use super::{
        Embedder, EmbedderError, Embedding, EmbeddingCandidate, EmbeddingIndex,
        EmbeddingRepositoryError, EmbeddingSearchResult, SUPPORTED_EMBEDDING_DIMENSIONS,
    };

    /// An embedding with a single nonzero dimension, giving each entry a
    /// distinct direction so cosine distances are meaningfully distinct.
    pub(crate) fn directional_embedding(nonzero_dim: usize) -> Embedding {
        let mut values = vec![0.0f32; SUPPORTED_EMBEDDING_DIMENSIONS];
        values[nonzero_dim] = 1.0;
        Embedding::new(values, SUPPORTED_EMBEDDING_DIMENSIONS).unwrap()
    }

    /// Embedder returning one fixed result (a directional embedding or an
    /// error) for every input.
    #[derive(Clone)]
    pub(crate) struct FakeEmbedder {
        model: String,
        result: Result<Embedding, EmbedderError>,
    }

    impl FakeEmbedder {
        pub(crate) fn succeeds(model: &str, dim: usize) -> Self {
            Self {
                model: model.to_string(),
                result: Ok(directional_embedding(dim)),
            }
        }

        pub(crate) fn fails(model: &str) -> Self {
            Self {
                model: model.to_string(),
                result: Err(EmbedderError::Provider("provider down".to_string())),
            }
        }
    }

    #[async_trait]
    impl Embedder for FakeEmbedder {
        fn model(&self) -> &str {
            &self.model
        }

        fn dimensions(&self) -> usize {
            SUPPORTED_EMBEDDING_DIMENSIONS
        }

        async fn embed(&self, _text: &str) -> Result<Embedding, EmbedderError> {
            self.result.clone()
        }
    }

    /// Index whose `search` returns a preset result list; every other method
    /// is out of scope for search-oriented tests and panics if reached.
    #[derive(Clone)]
    pub(crate) struct PresetIndex<ID> {
        pub(crate) results: Vec<EmbeddingSearchResult<ID>>,
    }

    #[async_trait]
    impl<ID: Clone + Send + Sync> EmbeddingIndex<ID> for PresetIndex<ID> {
        async fn store_embedding(
            &self,
            _id: ID,
            _embedding_model: &str,
            _embedding_dim: usize,
            _embedding: &Embedding,
        ) -> Result<bool, EmbeddingRepositoryError> {
            unreachable!("search tests do not store through PresetIndex")
        }

        async fn record_embedding_failure(
            &self,
            _id: ID,
            _embedding_model: &str,
            _error_message: &str,
        ) -> Result<(), EmbeddingRepositoryError> {
            unreachable!("search tests do not record failures through PresetIndex")
        }

        async fn delete_failed_embedding(
            &self,
            _id: ID,
            _embedding_model: &str,
        ) -> Result<bool, EmbeddingRepositoryError> {
            unreachable!("search tests do not delete through PresetIndex")
        }

        async fn find_entries_missing_or_failed_embedding(
            &self,
            _embedding_model: &str,
            _limit: u32,
        ) -> Result<Vec<EmbeddingCandidate<ID>>, EmbeddingRepositoryError> {
            unreachable!("search tests do not backfill through PresetIndex")
        }

        async fn count_entries_missing_or_failed_embedding(
            &self,
            _embedding_model: &str,
        ) -> Result<u32, EmbeddingRepositoryError> {
            unreachable!("search tests do not count missing through PresetIndex")
        }

        async fn search(
            &self,
            _embedding: &Embedding,
            _embedding_model: &str,
            _from_date: Option<NaiveDate>,
            _to_date_exclusive: Option<NaiveDate>,
            _limit: usize,
        ) -> Result<Vec<EmbeddingSearchResult<ID>>, EmbeddingRepositoryError> {
            Ok(self.results.clone())
        }
    }
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use chrono::{TimeZone, Utc};

    use super::*;
    use crate::{
        journal::repository::JournalRepository,
        messages::{IncomingMessage, MessageSource},
    };

    async fn setup() -> (JournalRepository, SqliteEmbeddingRepository) {
        let pool = crate::database::test_pool().await;

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
    ) -> String {
        journal_repository
            .store(&incoming(source_message_id, text, received_at))
            .await
            .unwrap();

        sqlx::query_scalar::<_, String>(
            "SELECT id FROM journal_entries WHERE source = \'telegram\' AND source_message_id = ?",
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
        failing_journal_entry_id: String,
    }

    #[async_trait]
    impl EmbeddingIndex<String> for StorageFailingIndex {
        async fn store_embedding(
            &self,
            journal_entry_id: String,
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
                .store_embedding(&journal_entry_id, embedding_model, embedding_dim, embedding)
                .await
                .map_err(Into::into)
        }

        async fn record_embedding_failure(
            &self,
            journal_entry_id: String,
            embedding_model: &str,
            error_message: &str,
        ) -> Result<(), EmbeddingRepositoryError> {
            self.inner
                .record_embedding_failure(&journal_entry_id, embedding_model, error_message)
                .await
                .map_err(Into::into)
        }

        async fn delete_failed_embedding(
            &self,
            journal_entry_id: String,
            embedding_model: &str,
        ) -> Result<bool, EmbeddingRepositoryError> {
            self.inner
                .delete_failed_embedding(&journal_entry_id, embedding_model)
                .await
                .map_err(Into::into)
        }

        async fn find_entries_missing_or_failed_embedding(
            &self,
            embedding_model: &str,
            limit: u32,
        ) -> Result<Vec<EmbeddingCandidate<String>>, EmbeddingRepositoryError> {
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

        async fn search(
            &self,
            embedding: &Embedding,
            embedding_model: &str,
            from_date: Option<chrono::NaiveDate>,
            to_date_exclusive: Option<chrono::NaiveDate>,
            limit: usize,
        ) -> Result<Vec<EmbeddingSearchResult<String>>, EmbeddingRepositoryError> {
            self.inner
                .search(
                    embedding,
                    embedding_model,
                    from_date,
                    to_date_exclusive,
                    limit,
                )
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
                .has_embedding(&first, TEST_EMBEDDING_MODEL)
                .await
                .unwrap()
        );
        let first_stored = embedding_repository
            .stored_embedding(&first, TEST_EMBEDDING_MODEL)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(first_stored.embedding_model, TEST_EMBEDDING_MODEL);
        assert_eq!(first_stored.embedding_dim, TEST_EMBEDDING_DIMENSIONS as i64);
        assert!(
            embedding_repository
                .has_embedding(&second, TEST_EMBEDDING_MODEL)
                .await
                .unwrap()
        );
        assert!(
            !embedding_repository
                .has_embedding(&third, TEST_EMBEDDING_MODEL)
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
                .has_embedding(&second, TEST_EMBEDDING_MODEL)
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
            failing_journal_entry_id: first.clone(),
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
                .stored_embedding(&first, TEST_EMBEDDING_MODEL)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            embedding_repository
                .has_embedding(&second, TEST_EMBEDDING_MODEL)
                .await
                .unwrap()
        );
    }
}
