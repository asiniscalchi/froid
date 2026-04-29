use std::{error::Error, fmt};

use sqlx::{Row, SqlitePool};

pub const EMBEDDING_DIMENSIONS: usize = 4;

#[derive(Debug, Clone, PartialEq)]
pub struct Embedding(Vec<f32>);

impl Embedding {
    pub fn new(values: Vec<f32>) -> Result<Self, EmbeddingError> {
        if values.len() != EMBEDDING_DIMENSIONS {
            return Err(EmbeddingError::InvalidDimension {
                expected: EMBEDDING_DIMENSIONS,
                actual: values.len(),
            });
        }

        Ok(Self(values))
    }

    pub fn values(&self) -> &[f32] {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmbeddingError {
    InvalidDimension { expected: usize, actual: usize },
    GenerationFailed(String),
}

impl fmt::Display for EmbeddingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDimension { expected, actual } => {
                write!(
                    f,
                    "embedding dimension mismatch: expected {expected}, got {actual}"
                )
            }
            Self::GenerationFailed(message) => write!(f, "embedding generation failed: {message}"),
        }
    }
}

impl Error for EmbeddingError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalEntryEmbeddingCandidate {
    pub journal_entry_id: i64,
    pub raw_text: String,
}

#[derive(Debug, Clone)]
pub struct SqliteEmbeddingRepository {
    pool: SqlitePool,
}

impl SqliteEmbeddingRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn store_embedding(
        &self,
        journal_entry_id: i64,
        embedding_model: &str,
        embedding: &Embedding,
    ) -> Result<bool, sqlx::Error> {
        let mut tx = self.pool.begin().await?;

        let result = sqlx::query(
            r#"
            INSERT OR IGNORE INTO journal_entry_embedding_metadata
                (journal_entry_id, embedding_model)
            VALUES (?, ?)
            "#,
        )
        .bind(journal_entry_id)
        .bind(embedding_model)
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

    pub async fn has_embedding(
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

    pub async fn find_entries_missing_embedding(
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

    pub async fn count_entries_missing_embedding(
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
            "#,
        )
        .bind(embedding_model)
        .fetch_one(&self.pool)
        .await
    }

    #[cfg(test)]
    async fn stored_embedding(
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
                embedding: Embedding::new(blob_to_embedding_values(
                    &row.get::<Vec<u8>, _>("embedding"),
                ))
                .map_err(|error| sqlx::Error::Decode(Box::new(error)))?,
            })
        })
        .transpose()
    }
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq)]
struct StoredEmbedding {
    metadata_id: i64,
    journal_entry_id: i64,
    embedding_model: String,
    embedding: Embedding,
}

fn embedding_to_blob(embedding: &Embedding) -> Vec<u8> {
    let mut blob = Vec::with_capacity(embedding.values().len() * size_of::<f32>());

    for value in embedding.values() {
        blob.extend_from_slice(&value.to_le_bytes());
    }

    blob
}

#[cfg(test)]
fn blob_to_embedding_values(blob: &[u8]) -> Vec<f32> {
    blob.chunks_exact(size_of::<f32>())
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

    fn embedding(values: [f32; EMBEDDING_DIMENSIONS]) -> Embedding {
        Embedding::new(values.to_vec()).unwrap()
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
                "test-model-v1",
                &embedding([1.0, 2.0, 3.0, 4.0]),
            )
            .await
            .unwrap();

        assert!(created);

        let stored = embedding_repository
            .stored_embedding(journal_entry_id, "test-model-v1")
            .await
            .unwrap()
            .unwrap();

        assert_eq!(stored.journal_entry_id, journal_entry_id);
        assert_eq!(stored.embedding_model, "test-model-v1");
        assert_eq!(stored.embedding, embedding([1.0, 2.0, 3.0, 4.0]));
    }

    #[tokio::test]
    async fn duplicate_embedding_storage_is_noop() {
        let (journal_repository, embedding_repository) = setup().await;
        let journal_entry_id = store_entry(&journal_repository, "100", "first", at(10, 0)).await;

        let first_created = embedding_repository
            .store_embedding(
                journal_entry_id,
                "test-model-v1",
                &embedding([1.0, 2.0, 3.0, 4.0]),
            )
            .await
            .unwrap();
        let second_created = embedding_repository
            .store_embedding(
                journal_entry_id,
                "test-model-v1",
                &embedding([4.0, 3.0, 2.0, 1.0]),
            )
            .await
            .unwrap();

        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM journal_entry_embedding_metadata WHERE journal_entry_id = ? AND embedding_model = ?",
        )
        .bind(journal_entry_id)
        .bind("test-model-v1")
        .fetch_one(&embedding_repository.pool)
        .await
        .unwrap();

        let stored = embedding_repository
            .stored_embedding(journal_entry_id, "test-model-v1")
            .await
            .unwrap()
            .unwrap();

        assert!(first_created);
        assert!(!second_created);
        assert_eq!(count, 1);
        assert_eq!(stored.embedding, embedding([1.0, 2.0, 3.0, 4.0]));
    }

    #[tokio::test]
    async fn supports_multiple_embedding_models_for_same_entry() {
        let (journal_repository, embedding_repository) = setup().await;
        let journal_entry_id = store_entry(&journal_repository, "100", "first", at(10, 0)).await;

        embedding_repository
            .store_embedding(
                journal_entry_id,
                "test-model-v1",
                &embedding([1.0, 2.0, 3.0, 4.0]),
            )
            .await
            .unwrap();
        embedding_repository
            .store_embedding(
                journal_entry_id,
                "test-model-v2",
                &embedding([4.0, 3.0, 2.0, 1.0]),
            )
            .await
            .unwrap();

        assert!(
            embedding_repository
                .has_embedding(journal_entry_id, "test-model-v1")
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
    async fn finds_missing_entries_oldest_first_with_limit() {
        let (journal_repository, embedding_repository) = setup().await;
        let second = store_entry(&journal_repository, "2", "second", at(11, 0)).await;
        let first = store_entry(&journal_repository, "1", "first", at(10, 0)).await;
        let third = store_entry(&journal_repository, "3", "third", at(12, 0)).await;

        embedding_repository
            .store_embedding(second, "test-model-v1", &embedding([1.0, 2.0, 3.0, 4.0]))
            .await
            .unwrap();

        let missing = embedding_repository
            .find_entries_missing_embedding("test-model-v1", 2)
            .await
            .unwrap();

        assert_eq!(
            missing,
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
    async fn detects_entries_missing_new_embedding_model() {
        let (journal_repository, embedding_repository) = setup().await;
        let journal_entry_id = store_entry(&journal_repository, "100", "first", at(10, 0)).await;

        embedding_repository
            .store_embedding(
                journal_entry_id,
                "test-model-v1",
                &embedding([1.0, 2.0, 3.0, 4.0]),
            )
            .await
            .unwrap();

        assert_eq!(
            embedding_repository
                .count_entries_missing_embedding("test-model-v1")
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            embedding_repository
                .count_entries_missing_embedding("test-model-v2")
                .await
                .unwrap(),
            1
        );
    }
}
