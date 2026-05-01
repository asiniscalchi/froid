use std::{error::Error, fmt, mem::size_of_val};

use async_trait::async_trait;
use sqlx::{Row, SqlitePool, sqlite::SqliteRow};

use super::{Embedding, EmbeddingSearchResult, JournalEntryEmbeddingCandidate};

fn map_search_result(row: SqliteRow) -> EmbeddingSearchResult {
    EmbeddingSearchResult {
        journal_entry_id: row.get("journal_entry_id"),
        distance: row.get("distance"),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmbeddingRepositoryError {
    Database(String),
}

impl fmt::Display for EmbeddingRepositoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Database(message) => write!(f, "embedding repository database error: {message}"),
        }
    }
}

impl Error for EmbeddingRepositoryError {}

impl From<sqlx::Error> for EmbeddingRepositoryError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error.to_string())
    }
}

#[async_trait]
pub trait EmbeddingIndex: Send + Sync {
    async fn store_embedding(
        &self,
        journal_entry_id: i64,
        embedding_model: &str,
        embedding_dim: usize,
        embedding: &Embedding,
    ) -> Result<bool, EmbeddingRepositoryError>;

    async fn record_embedding_failure(
        &self,
        journal_entry_id: i64,
        embedding_model: &str,
        error_message: &str,
    ) -> Result<(), EmbeddingRepositoryError>;

    async fn delete_failed_embedding(
        &self,
        journal_entry_id: i64,
        embedding_model: &str,
    ) -> Result<bool, EmbeddingRepositoryError>;

    async fn find_entries_missing_or_failed_embedding(
        &self,
        embedding_model: &str,
        limit: u32,
    ) -> Result<Vec<JournalEntryEmbeddingCandidate>, EmbeddingRepositoryError>;

    async fn search_for_user(
        &self,
        user_id: &str,
        embedding: &Embedding,
        embedding_model: &str,
        limit: usize,
    ) -> Result<Vec<EmbeddingSearchResult>, EmbeddingRepositoryError>;
}

#[async_trait]
pub trait PendingEmbeddingCounter: Send + Sync {
    async fn count_entries_missing_embedding_for_user(
        &self,
        user_id: &str,
        embedding_model: &str,
    ) -> Result<i64, EmbeddingRepositoryError>;
}

#[derive(Debug, Clone)]
pub struct SqliteEmbeddingRepository {
    pub(crate) pool: SqlitePool,
}

impl SqliteEmbeddingRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn record_embedding_failure(
        &self,
        journal_entry_id: i64,
        embedding_model: &str,
        error_message: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"
            INSERT OR IGNORE INTO journal_entry_embedding_metadata
                (journal_entry_id, embedding_model, embedding_dim, status, error_message)
            VALUES (?, ?, 0, 'failed', ?)
            "#,
        )
        .bind(journal_entry_id)
        .bind(embedding_model)
        .bind(error_message)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn delete_failed_embedding(
        &self,
        journal_entry_id: i64,
        embedding_model: &str,
    ) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            r#"
            DELETE FROM journal_entry_embedding_metadata
            WHERE journal_entry_id = ? AND embedding_model = ? AND status = 'failed'
            "#,
        )
        .bind(journal_entry_id)
        .bind(embedding_model)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected() > 0)
    }

    pub async fn store_embedding(
        &self,
        journal_entry_id: i64,
        embedding_model: &str,
        embedding_dim: usize,
        embedding: &Embedding,
    ) -> Result<bool, sqlx::Error> {
        let mut tx = self.pool.begin().await?;

        let result = sqlx::query(
            r#"
            INSERT OR IGNORE INTO journal_entry_embedding_metadata
                (journal_entry_id, embedding_model, embedding_dim)
            VALUES (?, ?, ?)
            "#,
        )
        .bind(journal_entry_id)
        .bind(embedding_model)
        .bind(embedding_dim as i64)
        .execute(&mut *tx)
        .await?;

        if result.rows_affected() == 0 {
            tx.commit().await?;
            return Ok(false);
        }

        sqlx::query(
            r#"
            INSERT INTO journal_entry_embedding_vec(rowid, embedding)
            VALUES (?, ?)
            "#,
        )
        .bind(result.last_insert_rowid())
        .bind(embedding_to_blob(embedding))
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;

        Ok(true)
    }

    #[cfg(test)]
    pub(crate) async fn has_embedding(
        &self,
        journal_entry_id: i64,
        embedding_model: &str,
    ) -> Result<bool, sqlx::Error> {
        let exists: bool = sqlx::query_scalar(
            r#"
            SELECT EXISTS(
                SELECT 1
                FROM journal_entry_embedding_metadata
                WHERE journal_entry_id = ?
                  AND embedding_model = ?
            )
            "#,
        )
        .bind(journal_entry_id)
        .bind(embedding_model)
        .fetch_one(&self.pool)
        .await?;

        Ok(exists)
    }

    pub async fn find_entries_missing_or_failed_embedding(
        &self,
        embedding_model: &str,
        limit: u32,
    ) -> Result<Vec<JournalEntryEmbeddingCandidate>, sqlx::Error> {
        let rows = sqlx::query(
            r#"
            SELECT journal_entries.id, journal_entries.raw_text
            FROM journal_entries
            LEFT JOIN journal_entry_embedding_metadata
              ON journal_entry_embedding_metadata.journal_entry_id = journal_entries.id
             AND journal_entry_embedding_metadata.embedding_model = ?
            WHERE journal_entry_embedding_metadata.id IS NULL
               OR journal_entry_embedding_metadata.status = 'failed'
            ORDER BY journal_entries.received_at ASC, journal_entries.id ASC
            LIMIT ?
            "#,
        )
        .bind(embedding_model)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| JournalEntryEmbeddingCandidate {
                journal_entry_id: row.get("id"),
                raw_text: row.get("raw_text"),
            })
            .collect())
    }

    #[cfg(test)]
    pub(crate) async fn count_entries_missing_or_failed_embedding(
        &self,
        embedding_model: &str,
    ) -> Result<i64, sqlx::Error> {
        sqlx::query_scalar(
            r#"
            SELECT COUNT(*)
            FROM journal_entries
            LEFT JOIN journal_entry_embedding_metadata
              ON journal_entry_embedding_metadata.journal_entry_id = journal_entries.id
             AND journal_entry_embedding_metadata.embedding_model = ?
            WHERE journal_entry_embedding_metadata.id IS NULL
               OR journal_entry_embedding_metadata.status = 'failed'
            "#,
        )
        .bind(embedding_model)
        .fetch_one(&self.pool)
        .await
    }

    pub async fn count_entries_missing_embedding_for_user(
        &self,
        user_id: &str,
        embedding_model: &str,
    ) -> Result<i64, sqlx::Error> {
        sqlx::query_scalar(
            r#"
            SELECT COUNT(*)
            FROM journal_entries
            LEFT JOIN journal_entry_embedding_metadata
              ON journal_entry_embedding_metadata.journal_entry_id = journal_entries.id
             AND journal_entry_embedding_metadata.embedding_model = ?
            WHERE journal_entries.user_id = ?
              AND (journal_entry_embedding_metadata.id IS NULL
                   OR journal_entry_embedding_metadata.status = 'failed')
            "#,
        )
        .bind(embedding_model)
        .bind(user_id)
        .fetch_one(&self.pool)
        .await
    }

    #[cfg(test)]
    pub(crate) async fn search(
        &self,
        embedding: &Embedding,
        embedding_model: &str,
        limit: usize,
    ) -> Result<Vec<EmbeddingSearchResult>, sqlx::Error> {
        let rows = sqlx::query(
            r#"
            SELECT
                m.journal_entry_id,
                vec_distance_cosine(v.embedding, ?) AS distance
            FROM journal_entry_embedding_metadata m
            JOIN journal_entry_embedding_vec v ON v.rowid = m.id
            WHERE m.embedding_model = ?
            ORDER BY distance ASC
            LIMIT ?
            "#,
        )
        .bind(embedding_to_blob(embedding))
        .bind(embedding_model)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(map_search_result).collect())
    }

    pub async fn search_for_user(
        &self,
        user_id: &str,
        embedding: &Embedding,
        embedding_model: &str,
        limit: usize,
    ) -> Result<Vec<EmbeddingSearchResult>, sqlx::Error> {
        let rows = sqlx::query(
            r#"
            SELECT
                m.journal_entry_id,
                vec_distance_cosine(v.embedding, ?) AS distance
            FROM journal_entry_embedding_metadata m
            JOIN journal_entry_embedding_vec v ON v.rowid = m.id
            JOIN journal_entries j ON j.id = m.journal_entry_id
            WHERE m.embedding_model = ?
              AND j.user_id = ?
            ORDER BY distance ASC
            LIMIT ?
            "#,
        )
        .bind(embedding_to_blob(embedding))
        .bind(embedding_model)
        .bind(user_id)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(map_search_result).collect())
    }

    #[cfg(test)]
    pub(crate) async fn stored_embedding(
        &self,
        journal_entry_id: i64,
        embedding_model: &str,
    ) -> Result<Option<StoredEmbedding>, sqlx::Error> {
        let row = sqlx::query(
            r#"
            SELECT
                metadata.id,
                metadata.journal_entry_id,
                metadata.embedding_model,
                metadata.embedding_dim,
                vec.embedding
            FROM journal_entry_embedding_metadata metadata
            JOIN journal_entry_embedding_vec vec
              ON vec.rowid = metadata.id
            WHERE metadata.journal_entry_id = ?
              AND metadata.embedding_model = ?
            "#,
        )
        .bind(journal_entry_id)
        .bind(embedding_model)
        .fetch_optional(&self.pool)
        .await?;

        row.map(|row| {
            Ok(StoredEmbedding {
                metadata_id: row.get("id"),
                journal_entry_id: row.get("journal_entry_id"),
                embedding_model: row.get("embedding_model"),
                embedding_dim: row.get("embedding_dim"),
                embedding: Embedding::new(
                    blob_to_embedding_values(&row.get::<Vec<u8>, _>("embedding")),
                    row.get::<i64, _>("embedding_dim") as usize,
                )
                .map_err(|error| sqlx::Error::Decode(Box::new(error)))?,
            })
        })
        .transpose()
    }
}

#[async_trait]
impl EmbeddingIndex for SqliteEmbeddingRepository {
    async fn store_embedding(
        &self,
        journal_entry_id: i64,
        embedding_model: &str,
        embedding_dim: usize,
        embedding: &Embedding,
    ) -> Result<bool, EmbeddingRepositoryError> {
        SqliteEmbeddingRepository::store_embedding(
            self,
            journal_entry_id,
            embedding_model,
            embedding_dim,
            embedding,
        )
        .await
        .map_err(Into::into)
    }

    async fn record_embedding_failure(
        &self,
        journal_entry_id: i64,
        embedding_model: &str,
        error_message: &str,
    ) -> Result<(), EmbeddingRepositoryError> {
        SqliteEmbeddingRepository::record_embedding_failure(
            self,
            journal_entry_id,
            embedding_model,
            error_message,
        )
        .await
        .map_err(Into::into)
    }

    async fn delete_failed_embedding(
        &self,
        journal_entry_id: i64,
        embedding_model: &str,
    ) -> Result<bool, EmbeddingRepositoryError> {
        SqliteEmbeddingRepository::delete_failed_embedding(self, journal_entry_id, embedding_model)
            .await
            .map_err(Into::into)
    }

    async fn find_entries_missing_or_failed_embedding(
        &self,
        embedding_model: &str,
        limit: u32,
    ) -> Result<Vec<JournalEntryEmbeddingCandidate>, EmbeddingRepositoryError> {
        SqliteEmbeddingRepository::find_entries_missing_or_failed_embedding(
            self,
            embedding_model,
            limit,
        )
        .await
        .map_err(Into::into)
    }

    async fn search_for_user(
        &self,
        user_id: &str,
        embedding: &Embedding,
        embedding_model: &str,
        limit: usize,
    ) -> Result<Vec<EmbeddingSearchResult>, EmbeddingRepositoryError> {
        SqliteEmbeddingRepository::search_for_user(self, user_id, embedding, embedding_model, limit)
            .await
            .map_err(Into::into)
    }
}

#[async_trait]
impl PendingEmbeddingCounter for SqliteEmbeddingRepository {
    async fn count_entries_missing_embedding_for_user(
        &self,
        user_id: &str,
        embedding_model: &str,
    ) -> Result<i64, EmbeddingRepositoryError> {
        SqliteEmbeddingRepository::count_entries_missing_embedding_for_user(
            self,
            user_id,
            embedding_model,
        )
        .await
        .map_err(Into::into)
    }
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct StoredEmbedding {
    pub(crate) metadata_id: i64,
    pub(crate) journal_entry_id: i64,
    pub(crate) embedding_model: String,
    pub(crate) embedding_dim: i64,
    pub(crate) embedding: Embedding,
}

fn embedding_to_blob(embedding: &Embedding) -> Vec<u8> {
    let mut blob = Vec::with_capacity(size_of_val(embedding.values()));

    for value in embedding.values() {
        blob.extend_from_slice(&value.to_le_bytes());
    }

    blob
}

#[cfg(test)]
fn blob_to_embedding_values(blob: &[u8]) -> Vec<f32> {
    blob.chunks_exact(std::mem::size_of::<f32>())
        .map(|chunk| f32::from_le_bytes(chunk.try_into().unwrap()))
        .collect()
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use sqlx::SqlitePool;

    use super::*;
    use crate::{
        database,
        journal::{embedding::SUPPORTED_EMBEDDING_DIMENSIONS, repository::JournalRepository},
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

    fn embedding(seed: f32) -> Embedding {
        Embedding::new(
            vec![seed; TEST_EMBEDDING_DIMENSIONS],
            TEST_EMBEDDING_DIMENSIONS,
        )
        .unwrap()
    }

    // Creates an embedding with a single nonzero dimension, giving each entry a distinct direction
    // so vec_distance_cosine produces meaningful ordering.
    fn directional_embedding(nonzero_dim: usize, value: f32) -> Embedding {
        let mut values = vec![0.0f32; TEST_EMBEDDING_DIMENSIONS];
        values[nonzero_dim] = value;
        Embedding::new(values, TEST_EMBEDDING_DIMENSIONS).unwrap()
    }

    #[tokio::test]
    async fn migrated_schema_can_store_sqlite_vec_embeddings() {
        let (_, embedding_repository) = setup().await;

        let version: String = sqlx::query_scalar("SELECT vec_version()")
            .fetch_one(&embedding_repository.pool)
            .await
            .unwrap();

        assert!(!version.is_empty());
    }

    #[tokio::test]
    async fn stores_embedding_linked_to_journal_entry_and_model() {
        let (journal_repository, embedding_repository) = setup().await;
        let journal_entry_id = store_entry(&journal_repository, "100", "first", at(10, 0)).await;

        let created = embedding_repository
            .store_embedding(
                journal_entry_id,
                TEST_EMBEDDING_MODEL,
                TEST_EMBEDDING_DIMENSIONS,
                &embedding(1.0),
            )
            .await
            .unwrap();

        assert!(created);

        let stored = embedding_repository
            .stored_embedding(journal_entry_id, TEST_EMBEDDING_MODEL)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(stored.journal_entry_id, journal_entry_id);
        assert_eq!(stored.embedding_model, TEST_EMBEDDING_MODEL);
        assert_eq!(stored.embedding_dim, TEST_EMBEDDING_DIMENSIONS as i64);
        assert_eq!(stored.embedding, embedding(1.0));
    }

    #[tokio::test]
    async fn duplicate_embedding_storage_is_noop() {
        let (journal_repository, embedding_repository) = setup().await;
        let journal_entry_id = store_entry(&journal_repository, "100", "first", at(10, 0)).await;

        let first_created = embedding_repository
            .store_embedding(
                journal_entry_id,
                TEST_EMBEDDING_MODEL,
                TEST_EMBEDDING_DIMENSIONS,
                &embedding(1.0),
            )
            .await
            .unwrap();
        let second_created = embedding_repository
            .store_embedding(
                journal_entry_id,
                TEST_EMBEDDING_MODEL,
                TEST_EMBEDDING_DIMENSIONS,
                &embedding(4.0),
            )
            .await
            .unwrap();

        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM journal_entry_embedding_metadata WHERE journal_entry_id = ? AND embedding_model = ?",
        )
        .bind(journal_entry_id)
        .bind(TEST_EMBEDDING_MODEL)
        .fetch_one(&embedding_repository.pool)
        .await
        .unwrap();

        let stored = embedding_repository
            .stored_embedding(journal_entry_id, TEST_EMBEDDING_MODEL)
            .await
            .unwrap()
            .unwrap();

        assert!(first_created);
        assert!(!second_created);
        assert_eq!(count, 1);
        assert_eq!(stored.embedding, embedding(1.0));
    }

    #[tokio::test]
    async fn supports_multiple_embedding_models_for_same_entry() {
        let (journal_repository, embedding_repository) = setup().await;
        let journal_entry_id = store_entry(&journal_repository, "100", "first", at(10, 0)).await;

        embedding_repository
            .store_embedding(
                journal_entry_id,
                TEST_EMBEDDING_MODEL,
                TEST_EMBEDDING_DIMENSIONS,
                &embedding(1.0),
            )
            .await
            .unwrap();
        embedding_repository
            .store_embedding(
                journal_entry_id,
                "test-model-v2",
                TEST_EMBEDDING_DIMENSIONS,
                &embedding(4.0),
            )
            .await
            .unwrap();

        assert!(
            embedding_repository
                .has_embedding(journal_entry_id, TEST_EMBEDDING_MODEL)
                .await
                .unwrap()
        );
        assert!(
            embedding_repository
                .has_embedding(journal_entry_id, "test-model-v2")
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn finds_missing_and_failed_entries_oldest_first_with_limit() {
        let (journal_repository, embedding_repository) = setup().await;
        let second = store_entry(&journal_repository, "2", "second", at(11, 0)).await;
        let first = store_entry(&journal_repository, "1", "first", at(10, 0)).await;
        let third = store_entry(&journal_repository, "3", "third", at(12, 0)).await;

        // second has a completed embedding — should not appear
        embedding_repository
            .store_embedding(
                second,
                TEST_EMBEDDING_MODEL,
                TEST_EMBEDDING_DIMENSIONS,
                &embedding(1.0),
            )
            .await
            .unwrap();
        // third has a failed embedding — should appear
        embedding_repository
            .record_embedding_failure(third, TEST_EMBEDDING_MODEL, "provider error")
            .await
            .unwrap();

        let candidates = embedding_repository
            .find_entries_missing_or_failed_embedding(TEST_EMBEDDING_MODEL, 10)
            .await
            .unwrap();

        assert_eq!(
            candidates,
            vec![
                JournalEntryEmbeddingCandidate {
                    journal_entry_id: first,
                    raw_text: "first".to_string(),
                },
                JournalEntryEmbeddingCandidate {
                    journal_entry_id: third,
                    raw_text: "third".to_string(),
                },
            ]
        );
    }

    #[tokio::test]
    async fn find_missing_or_failed_respects_limit() {
        let (journal_repository, embedding_repository) = setup().await;
        let first = store_entry(&journal_repository, "1", "first", at(10, 0)).await;
        store_entry(&journal_repository, "2", "second", at(11, 0)).await;
        store_entry(&journal_repository, "3", "third", at(12, 0)).await;

        let candidates = embedding_repository
            .find_entries_missing_or_failed_embedding(TEST_EMBEDDING_MODEL, 2)
            .await
            .unwrap();

        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].journal_entry_id, first);
    }

    #[tokio::test]
    async fn records_embedding_failure_inserts_failed_row() {
        let (journal_repository, embedding_repository) = setup().await;
        let entry_id = store_entry(&journal_repository, "1", "first", at(10, 0)).await;

        embedding_repository
            .record_embedding_failure(entry_id, TEST_EMBEDDING_MODEL, "provider down")
            .await
            .unwrap();

        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM journal_entry_embedding_metadata WHERE journal_entry_id = ? AND status = 'failed'",
        )
        .bind(entry_id)
        .fetch_one(&embedding_repository.pool)
        .await
        .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn record_embedding_failure_does_not_overwrite_completed_embedding() {
        let (journal_repository, embedding_repository) = setup().await;
        let entry_id = store_entry(&journal_repository, "1", "first", at(10, 0)).await;

        embedding_repository
            .store_embedding(
                entry_id,
                TEST_EMBEDDING_MODEL,
                TEST_EMBEDDING_DIMENSIONS,
                &embedding(1.0),
            )
            .await
            .unwrap();
        embedding_repository
            .record_embedding_failure(entry_id, TEST_EMBEDDING_MODEL, "should be ignored")
            .await
            .unwrap();

        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM journal_entry_embedding_metadata WHERE journal_entry_id = ? AND status = 'completed'",
        )
        .bind(entry_id)
        .fetch_one(&embedding_repository.pool)
        .await
        .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn delete_failed_embedding_removes_failed_row() {
        let (journal_repository, embedding_repository) = setup().await;
        let entry_id = store_entry(&journal_repository, "1", "first", at(10, 0)).await;

        embedding_repository
            .record_embedding_failure(entry_id, TEST_EMBEDDING_MODEL, "error")
            .await
            .unwrap();

        let deleted = embedding_repository
            .delete_failed_embedding(entry_id, TEST_EMBEDDING_MODEL)
            .await
            .unwrap();

        assert!(deleted);
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM journal_entry_embedding_metadata")
                .fetch_one(&embedding_repository.pool)
                .await
                .unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn delete_failed_embedding_returns_false_when_no_failed_row_exists() {
        let (journal_repository, embedding_repository) = setup().await;
        let entry_id = store_entry(&journal_repository, "1", "first", at(10, 0)).await;

        let deleted = embedding_repository
            .delete_failed_embedding(entry_id, TEST_EMBEDDING_MODEL)
            .await
            .unwrap();

        assert!(!deleted);
    }

    #[tokio::test]
    async fn delete_failed_embedding_does_not_remove_completed_embedding() {
        let (journal_repository, embedding_repository) = setup().await;
        let entry_id = store_entry(&journal_repository, "1", "first", at(10, 0)).await;

        embedding_repository
            .store_embedding(
                entry_id,
                TEST_EMBEDDING_MODEL,
                TEST_EMBEDDING_DIMENSIONS,
                &embedding(1.0),
            )
            .await
            .unwrap();
        let deleted = embedding_repository
            .delete_failed_embedding(entry_id, TEST_EMBEDDING_MODEL)
            .await
            .unwrap();

        assert!(!deleted);
        assert!(
            embedding_repository
                .has_embedding(entry_id, TEST_EMBEDDING_MODEL)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn detects_entries_missing_or_failed_for_new_embedding_model() {
        let (journal_repository, embedding_repository) = setup().await;
        let journal_entry_id = store_entry(&journal_repository, "100", "first", at(10, 0)).await;

        embedding_repository
            .store_embedding(
                journal_entry_id,
                TEST_EMBEDDING_MODEL,
                TEST_EMBEDDING_DIMENSIONS,
                &embedding(1.0),
            )
            .await
            .unwrap();

        assert_eq!(
            embedding_repository
                .count_entries_missing_or_failed_embedding(TEST_EMBEDDING_MODEL)
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            embedding_repository
                .count_entries_missing_or_failed_embedding("test-model-v2")
                .await
                .unwrap(),
            1
        );
    }

    #[tokio::test]
    async fn counts_failed_entries_as_missing() {
        let (journal_repository, embedding_repository) = setup().await;
        let entry_id = store_entry(&journal_repository, "1", "first", at(10, 0)).await;

        embedding_repository
            .record_embedding_failure(entry_id, TEST_EMBEDDING_MODEL, "error")
            .await
            .unwrap();

        assert_eq!(
            embedding_repository
                .count_entries_missing_or_failed_embedding(TEST_EMBEDDING_MODEL)
                .await
                .unwrap(),
            1
        );
    }

    #[tokio::test]
    async fn search_returns_results_ordered_by_cosine_distance() {
        let (journal_repository, embedding_repository) = setup().await;
        let first = store_entry(&journal_repository, "1", "first", at(10, 0)).await;
        let second = store_entry(&journal_repository, "2", "second", at(11, 0)).await;
        let third = store_entry(&journal_repository, "3", "third", at(12, 0)).await;

        // Give each entry a unique direction so cosine distances are meaningfully distinct.
        // query points along dim 1, so second (also dim 1) is the closest match.
        embedding_repository
            .store_embedding(
                first,
                TEST_EMBEDDING_MODEL,
                TEST_EMBEDDING_DIMENSIONS,
                &directional_embedding(0, 1.0),
            )
            .await
            .unwrap();
        embedding_repository
            .store_embedding(
                second,
                TEST_EMBEDDING_MODEL,
                TEST_EMBEDDING_DIMENSIONS,
                &directional_embedding(1, 1.0),
            )
            .await
            .unwrap();
        embedding_repository
            .store_embedding(
                third,
                TEST_EMBEDDING_MODEL,
                TEST_EMBEDDING_DIMENSIONS,
                &directional_embedding(2, 1.0),
            )
            .await
            .unwrap();

        let query = directional_embedding(1, 1.0);
        let results = embedding_repository
            .search(&query, TEST_EMBEDDING_MODEL, 3)
            .await
            .unwrap();

        assert_eq!(results.len(), 3);
        assert_eq!(results[0].journal_entry_id, second);
        assert!(results[0].distance <= results[1].distance);
        assert!(results[1].distance <= results[2].distance);
    }

    #[tokio::test]
    async fn search_respects_limit() {
        let (journal_repository, embedding_repository) = setup().await;
        let first = store_entry(&journal_repository, "1", "first", at(10, 0)).await;
        let second = store_entry(&journal_repository, "2", "second", at(11, 0)).await;
        let third = store_entry(&journal_repository, "3", "third", at(12, 0)).await;

        for (id, dim) in [(first, 0), (second, 1), (third, 2)] {
            embedding_repository
                .store_embedding(
                    id,
                    TEST_EMBEDDING_MODEL,
                    TEST_EMBEDDING_DIMENSIONS,
                    &directional_embedding(dim, 1.0),
                )
                .await
                .unwrap();
        }

        let results = embedding_repository
            .search(&directional_embedding(0, 1.0), TEST_EMBEDDING_MODEL, 2)
            .await
            .unwrap();

        assert_eq!(results.len(), 2);
    }

    #[tokio::test]
    async fn search_filters_by_embedding_model() {
        let (journal_repository, embedding_repository) = setup().await;
        let entry = store_entry(&journal_repository, "1", "first", at(10, 0)).await;

        embedding_repository
            .store_embedding(entry, "model-a", TEST_EMBEDDING_DIMENSIONS, &embedding(1.0))
            .await
            .unwrap();

        let results = embedding_repository
            .search(&embedding(1.0), "model-b", 10)
            .await
            .unwrap();

        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn search_returns_empty_when_no_embeddings_exist() {
        let (_, embedding_repository) = setup().await;

        let results = embedding_repository
            .search(&embedding(1.0), TEST_EMBEDDING_MODEL, 10)
            .await
            .unwrap();

        assert!(results.is_empty());
    }
}
