use std::{error::Error, fmt, mem::size_of_val};

use async_trait::async_trait;
use sqlx::{Row, SqlitePool};

use super::Embedding;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalEntryEmbeddingCandidate {
    pub journal_entry_id: i64,
    pub raw_text: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EmbeddingSearchResult {
    pub journal_entry_id: i64,
    pub score: f32,
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
#[allow(dead_code)]
pub trait EmbeddingIndex: Send + Sync {
    async fn store_embedding(
        &self,
        journal_entry_id: i64,
        embedding_model: &str,
        embedding_dim: usize,
        embedding: &Embedding,
    ) -> Result<bool, EmbeddingRepositoryError>;

    async fn has_embedding(
        &self,
        journal_entry_id: i64,
        embedding_model: &str,
    ) -> Result<bool, EmbeddingRepositoryError>;

    async fn find_entries_missing_embedding(
        &self,
        embedding_model: &str,
        limit: u32,
    ) -> Result<Vec<JournalEntryEmbeddingCandidate>, EmbeddingRepositoryError>;

    async fn count_entries_missing_embedding(
        &self,
        embedding_model: &str,
    ) -> Result<i64, EmbeddingRepositoryError>;

    async fn search(
        &self,
        embedding: &Embedding,
        embedding_model: &str,
        limit: usize,
    ) -> Result<Vec<EmbeddingSearchResult>, EmbeddingRepositoryError>;

    async fn search_for_user(
        &self,
        user_id: &str,
        embedding: &Embedding,
        embedding_model: &str,
        limit: usize,
    ) -> Result<Vec<EmbeddingSearchResult>, EmbeddingRepositoryError>;
}

#[derive(Debug, Clone)]
pub struct SqliteEmbeddingRepository {
    pub(crate) pool: SqlitePool,
}

impl SqliteEmbeddingRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
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

    pub async fn search(
        &self,
        embedding: &Embedding,
        embedding_model: &str,
        limit: usize,
    ) -> Result<Vec<EmbeddingSearchResult>, sqlx::Error> {
        let rows = sqlx::query(
            r#"
            SELECT
                m.journal_entry_id,
                vec_distance_cosine(v.embedding, ?) AS score
            FROM journal_entry_embedding_metadata m
            JOIN journal_entry_embedding_vec v ON v.rowid = m.id
            WHERE m.embedding_model = ?
            ORDER BY score ASC
            LIMIT ?
            "#,
        )
        .bind(embedding_to_blob(embedding))
        .bind(embedding_model)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| EmbeddingSearchResult {
                journal_entry_id: row.get("journal_entry_id"),
                score: row.get("score"),
            })
            .collect())
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
                vec_distance_cosine(v.embedding, ?) AS score
            FROM journal_entry_embedding_metadata m
            JOIN journal_entry_embedding_vec v ON v.rowid = m.id
            JOIN journal_entries j ON j.id = m.journal_entry_id
            WHERE m.embedding_model = ?
              AND j.user_id = ?
            ORDER BY score ASC
            LIMIT ?
            "#,
        )
        .bind(embedding_to_blob(embedding))
        .bind(embedding_model)
        .bind(user_id)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| EmbeddingSearchResult {
                journal_entry_id: row.get("journal_entry_id"),
                score: row.get("score"),
            })
            .collect())
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

    async fn has_embedding(
        &self,
        journal_entry_id: i64,
        embedding_model: &str,
    ) -> Result<bool, EmbeddingRepositoryError> {
        SqliteEmbeddingRepository::has_embedding(self, journal_entry_id, embedding_model)
            .await
            .map_err(Into::into)
    }

    async fn find_entries_missing_embedding(
        &self,
        embedding_model: &str,
        limit: u32,
    ) -> Result<Vec<JournalEntryEmbeddingCandidate>, EmbeddingRepositoryError> {
        SqliteEmbeddingRepository::find_entries_missing_embedding(self, embedding_model, limit)
            .await
            .map_err(Into::into)
    }

    async fn count_entries_missing_embedding(
        &self,
        embedding_model: &str,
    ) -> Result<i64, EmbeddingRepositoryError> {
        SqliteEmbeddingRepository::count_entries_missing_embedding(self, embedding_model)
            .await
            .map_err(Into::into)
    }

    async fn search(
        &self,
        embedding: &Embedding,
        embedding_model: &str,
        limit: usize,
    ) -> Result<Vec<EmbeddingSearchResult>, EmbeddingRepositoryError> {
        SqliteEmbeddingRepository::search(self, embedding, embedding_model, limit)
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
