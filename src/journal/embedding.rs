#![allow(dead_code)]

use std::{env, error::Error, fmt, mem::size_of_val, time::Duration};

use async_trait::async_trait;
use rig::{
    client::EmbeddingsClient,
    embeddings::EmbeddingModel,
    providers::openai::{self, Client as OpenAiClient},
};
use sqlx::{Row, SqlitePool};
use tracing::warn;

pub const DEFAULT_EMBEDDING_MODEL: &str = "text-embedding-3-small";
pub const SUPPORTED_EMBEDDING_DIMENSIONS: usize = 1536;

#[derive(Debug, Clone, PartialEq)]
pub struct Embedding(Vec<f32>);

impl Embedding {
    pub fn new(values: Vec<f32>, expected_dimensions: usize) -> Result<Self, EmbedderError> {
        if values.len() != expected_dimensions {
            return Err(EmbedderError::InvalidDimension {
                expected: expected_dimensions,
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
pub enum EmbedderError {
    InvalidDimension { expected: usize, actual: usize },
    Provider(String),
}

impl fmt::Display for EmbedderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDimension { expected, actual } => {
                write!(
                    f,
                    "embedding dimension mismatch: expected {expected}, got {actual}"
                )
            }
            Self::Provider(message) => write!(f, "embedding provider failed: {message}"),
        }
    }
}

impl Error for EmbedderError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingConfig {
    pub model: String,
    pub dimensions: usize,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            model: DEFAULT_EMBEDDING_MODEL.to_string(),
            dimensions: SUPPORTED_EMBEDDING_DIMENSIONS,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmbeddingConfigError {
    InvalidDimensions(String),
    UnsupportedDimensions { configured: usize, supported: usize },
}

impl fmt::Display for EmbeddingConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDimensions(value) => {
                write!(
                    f,
                    "FROID_EMBEDDING_DIMENSIONS must be a positive integer, got {value:?}"
                )
            }
            Self::UnsupportedDimensions {
                configured,
                supported,
            } => write!(
                f,
                "FROID_EMBEDDING_DIMENSIONS={configured} is not supported; this build supports only {supported}"
            ),
        }
    }
}

impl Error for EmbeddingConfigError {}

impl EmbeddingConfig {
    pub fn from_env() -> Result<Self, EmbeddingConfigError> {
        Self::from_values(
            env::var("FROID_EMBEDDING_MODEL").ok(),
            env::var("FROID_EMBEDDING_DIMENSIONS").ok(),
        )
    }

    fn from_values(
        model: Option<String>,
        dimensions: Option<String>,
    ) -> Result<Self, EmbeddingConfigError> {
        let model = model
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_EMBEDDING_MODEL.to_string());
        let dimensions = match dimensions {
            Some(value) if !value.trim().is_empty() => value
                .parse::<usize>()
                .map_err(|_| EmbeddingConfigError::InvalidDimensions(value))?,
            _ => SUPPORTED_EMBEDDING_DIMENSIONS,
        };

        if dimensions != SUPPORTED_EMBEDDING_DIMENSIONS {
            return Err(EmbeddingConfigError::UnsupportedDimensions {
                configured: dimensions,
                supported: SUPPORTED_EMBEDDING_DIMENSIONS,
            });
        }

        Ok(Self { model, dimensions })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RigOpenAiEmbedderError {
    MissingOpenAiApiKey,
    Client(String),
}

impl fmt::Display for RigOpenAiEmbedderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingOpenAiApiKey => write!(f, "OPENAI_API_KEY is required"),
            Self::Client(message) => write!(f, "failed to construct OpenAI embedder: {message}"),
        }
    }
}

impl Error for RigOpenAiEmbedderError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderError {
    Request(String),
}

impl fmt::Display for ProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Request(message) => write!(f, "{message}"),
        }
    }
}

impl Error for ProviderError {}

#[async_trait]
pub(crate) trait EmbeddingProvider: Send + Sync {
    async fn embed(&self, model: &str, text: &str) -> Result<Vec<f32>, ProviderError>;
}

#[derive(Clone)]
pub(crate) struct RigOpenAiProvider {
    embedding_model: openai::EmbeddingModel,
}

impl RigOpenAiProvider {
    fn new(config: &EmbeddingConfig, api_key: &str) -> Result<Self, RigOpenAiEmbedderError> {
        let client = OpenAiClient::new(api_key)
            .map_err(|error| RigOpenAiEmbedderError::Client(error.to_string()))?;
        let embedding_model = client.embedding_model_with_ndims(&config.model, config.dimensions);

        Ok(Self { embedding_model })
    }
}

#[async_trait]
impl EmbeddingProvider for RigOpenAiProvider {
    async fn embed(&self, _model: &str, text: &str) -> Result<Vec<f32>, ProviderError> {
        let embedding = self
            .embedding_model
            .embed_text(text)
            .await
            .map_err(|error| ProviderError::Request(error.to_string()))?;

        Ok(embedding
            .vec
            .into_iter()
            .map(|value| value as f32)
            .collect())
    }
}

#[derive(Clone)]
pub struct RigOpenAiEmbedder<P = RigOpenAiProvider> {
    config: EmbeddingConfig,
    provider: P,
}

impl RigOpenAiEmbedder<RigOpenAiProvider> {
    pub fn from_env(config: EmbeddingConfig) -> Result<Self, RigOpenAiEmbedderError> {
        Self::from_optional_api_key(config, env::var("OPENAI_API_KEY").ok())
    }

    fn from_optional_api_key(
        config: EmbeddingConfig,
        api_key: Option<String>,
    ) -> Result<Self, RigOpenAiEmbedderError> {
        let api_key = api_key
            .filter(|value| !value.trim().is_empty())
            .ok_or(RigOpenAiEmbedderError::MissingOpenAiApiKey)?;
        let provider = RigOpenAiProvider::new(&config, &api_key)?;

        Ok(Self { config, provider })
    }
}

impl<P> RigOpenAiEmbedder<P>
where
    P: EmbeddingProvider,
{
    fn new(config: EmbeddingConfig, provider: P) -> Self {
        Self { config, provider }
    }
}

#[async_trait]
impl<P> Embedder for RigOpenAiEmbedder<P>
where
    P: EmbeddingProvider,
{
    fn model(&self) -> &str {
        &self.config.model
    }

    fn dimensions(&self) -> usize {
        self.config.dimensions
    }

    async fn embed(&self, text: &str) -> Result<Embedding, EmbedderError> {
        let values = self
            .provider
            .embed(&self.config.model, text)
            .await
            .map_err(|error| EmbedderError::Provider(error.to_string()))?;

        Embedding::new(values, self.config.dimensions)
    }
}

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

#[async_trait]
pub trait Embedder: Send + Sync {
    fn model(&self) -> &str;

    fn dimensions(&self) -> usize;

    async fn embed(&self, text: &str) -> Result<Embedding, EmbedderError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackfillResult {
    pub attempted: u32,
    pub created: u32,
    pub failed: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmbeddingBackfillError {
    Repository(EmbeddingRepositoryError),
}

impl fmt::Display for EmbeddingBackfillError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Repository(error) => write!(f, "{error}"),
        }
    }
}

impl Error for EmbeddingBackfillError {}

#[derive(Debug, Clone)]
pub struct EmbeddingBackfillService<I, E> {
    index: I,
    embedder: E,
}

impl<I, E> EmbeddingBackfillService<I, E>
where
    I: EmbeddingIndex,
    E: Embedder,
{
    pub fn new(index: I, embedder: E) -> Self {
        Self { index, embedder }
    }

    pub fn model(&self) -> &str {
        self.embedder.model()
    }

    pub fn dimensions(&self) -> usize {
        self.embedder.dimensions()
    }

    pub async fn backfill_missing_embeddings(
        &self,
        limit: u32,
    ) -> Result<BackfillResult, EmbeddingBackfillError> {
        let embedding_model = self.embedder.model();
        let embedding_dim = self.embedder.dimensions();

        let candidates = self
            .index
            .find_entries_missing_embedding(embedding_model, limit)
            .await
            .map_err(EmbeddingBackfillError::Repository)?;

        let mut result = BackfillResult {
            attempted: candidates.len() as u32,
            created: 0,
            failed: 0,
        };

        for candidate in candidates {
            let embedding = match self.embedder.embed(&candidate.raw_text).await {
                Ok(embedding) => embedding,
                Err(error) => {
                    result.failed += 1;
                    warn!(
                        journal_entry_id = candidate.journal_entry_id,
                        embedding_model,
                        embedding_dim,
                        error = %error,
                        "failed to generate journal entry embedding"
                    );
                    continue;
                }
            };

            match self
                .index
                .store_embedding(
                    candidate.journal_entry_id,
                    embedding_model,
                    embedding_dim,
                    &embedding,
                )
                .await
            {
                Ok(true) => result.created += 1,
                Ok(false) => {}
                Err(error) => {
                    result.failed += 1;
                    warn!(
                        journal_entry_id = candidate.journal_entry_id,
                        embedding_model,
                        embedding_dim,
                        error = %error,
                        "failed to store journal entry embedding"
                    );
                }
            }
        }

        Ok(result)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmbeddingWorkerConfigError {
    InvalidBatchSize(String),
    InvalidInterval(String),
}

impl fmt::Display for EmbeddingWorkerConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidBatchSize(value) => write!(
                f,
                "FROID_EMBEDDING_WORKER_BATCH_SIZE must be a positive integer, got {value:?}"
            ),
            Self::InvalidInterval(value) => write!(
                f,
                "FROID_EMBEDDING_WORKER_INTERVAL_SECONDS must be a positive integer, got {value:?}"
            ),
        }
    }
}

impl Error for EmbeddingWorkerConfigError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingWorkerConfig {
    pub enabled: bool,
    pub batch_size: u32,
    pub interval: Duration,
}

impl Default for EmbeddingWorkerConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            batch_size: 20,
            interval: Duration::from_secs(300),
        }
    }
}

impl EmbeddingWorkerConfig {
    pub fn from_env() -> Result<Self, EmbeddingWorkerConfigError> {
        Self::from_values(
            env::var("FROID_EMBEDDING_WORKER_ENABLED").ok(),
            env::var("FROID_EMBEDDING_WORKER_BATCH_SIZE").ok(),
            env::var("FROID_EMBEDDING_WORKER_INTERVAL_SECONDS").ok(),
        )
    }

    pub fn from_values(
        enabled: Option<String>,
        batch_size: Option<String>,
        interval_seconds: Option<String>,
    ) -> Result<Self, EmbeddingWorkerConfigError> {
        let enabled = enabled
            .filter(|v| !v.trim().is_empty())
            .map(|v| v.trim() == "true")
            .unwrap_or(false);

        let batch_size = match batch_size {
            Some(ref value) if !value.trim().is_empty() => {
                let parsed = value
                    .trim()
                    .parse::<u32>()
                    .map_err(|_| EmbeddingWorkerConfigError::InvalidBatchSize(value.clone()))?;
                if parsed == 0 {
                    return Err(EmbeddingWorkerConfigError::InvalidBatchSize(value.clone()));
                }
                parsed
            }
            _ => 20,
        };

        let interval_secs = match interval_seconds {
            Some(ref value) if !value.trim().is_empty() => {
                let parsed = value
                    .trim()
                    .parse::<u64>()
                    .map_err(|_| EmbeddingWorkerConfigError::InvalidInterval(value.clone()))?;
                if parsed == 0 {
                    return Err(EmbeddingWorkerConfigError::InvalidInterval(value.clone()));
                }
                parsed
            }
            _ => 300,
        };

        Ok(Self {
            enabled,
            batch_size,
            interval: Duration::from_secs(interval_secs),
        })
    }
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
struct StoredEmbedding {
    metadata_id: i64,
    journal_entry_id: i64,
    embedding_model: String,
    embedding_dim: i64,
    embedding: Embedding,
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
    struct FakeProvider {
        result: Result<Vec<f32>, ProviderError>,
    }

    #[async_trait]
    impl EmbeddingProvider for FakeProvider {
        async fn embed(&self, _model: &str, _text: &str) -> Result<Vec<f32>, ProviderError> {
            self.result.clone()
        }
    }

    #[derive(Debug, Clone)]
    struct StorageFailingIndex {
        inner: SqliteEmbeddingRepository,
        failing_journal_entry_id: i64,
    }

    #[async_trait]
    impl EmbeddingIndex for StorageFailingIndex {
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

        async fn has_embedding(
            &self,
            journal_entry_id: i64,
            embedding_model: &str,
        ) -> Result<bool, EmbeddingRepositoryError> {
            self.inner
                .has_embedding(journal_entry_id, embedding_model)
                .await
                .map_err(Into::into)
        }

        async fn find_entries_missing_embedding(
            &self,
            embedding_model: &str,
            limit: u32,
        ) -> Result<Vec<JournalEntryEmbeddingCandidate>, EmbeddingRepositoryError> {
            self.inner
                .find_entries_missing_embedding(embedding_model, limit)
                .await
                .map_err(Into::into)
        }

        async fn count_entries_missing_embedding(
            &self,
            embedding_model: &str,
        ) -> Result<i64, EmbeddingRepositoryError> {
            self.inner
                .count_entries_missing_embedding(embedding_model)
                .await
                .map_err(Into::into)
        }

        async fn search(
            &self,
            embedding: &Embedding,
            embedding_model: &str,
            limit: usize,
        ) -> Result<Vec<EmbeddingSearchResult>, EmbeddingRepositoryError> {
            self.inner
                .search(embedding, embedding_model, limit)
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
            self.inner
                .search_for_user(user_id, embedding, embedding_model, limit)
                .await
                .map_err(Into::into)
        }
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

    #[test]
    fn embedding_config_uses_default_model_and_dimensions() {
        let config = EmbeddingConfig::from_values(None, None).unwrap();

        assert_eq!(config.model, DEFAULT_EMBEDDING_MODEL);
        assert_eq!(config.dimensions, SUPPORTED_EMBEDDING_DIMENSIONS);
    }

    #[test]
    fn embedding_config_rejects_non_1536_dimensions() {
        let error = EmbeddingConfig::from_values(None, Some("4".to_string())).unwrap_err();

        assert_eq!(
            error,
            EmbeddingConfigError::UnsupportedDimensions {
                configured: 4,
                supported: SUPPORTED_EMBEDDING_DIMENSIONS,
            }
        );
    }

    #[test]
    fn real_openai_embedder_requires_api_key() {
        let result = RigOpenAiEmbedder::from_optional_api_key(EmbeddingConfig::default(), None);

        assert!(matches!(
            result,
            Err(RigOpenAiEmbedderError::MissingOpenAiApiKey)
        ));
    }

    #[tokio::test]
    async fn rig_openai_embedder_accepts_provider_vector_with_configured_dimensions() {
        let embedder = RigOpenAiEmbedder::new(
            EmbeddingConfig::default(),
            FakeProvider {
                result: Ok(vec![1.0; SUPPORTED_EMBEDDING_DIMENSIONS]),
            },
        );

        let embedding = embedder.embed("hello").await.unwrap();

        assert_eq!(embedder.model(), DEFAULT_EMBEDDING_MODEL);
        assert_eq!(embedder.dimensions(), SUPPORTED_EMBEDDING_DIMENSIONS);
        assert_eq!(embedding.values().len(), SUPPORTED_EMBEDDING_DIMENSIONS);
    }

    #[tokio::test]
    async fn rig_openai_embedder_rejects_provider_vector_with_wrong_dimensions() {
        let embedder = RigOpenAiEmbedder::new(
            EmbeddingConfig::default(),
            FakeProvider {
                result: Ok(vec![1.0; SUPPORTED_EMBEDDING_DIMENSIONS - 1]),
            },
        );

        let error = embedder.embed("hello").await.unwrap_err();

        assert_eq!(
            error,
            EmbedderError::InvalidDimension {
                expected: SUPPORTED_EMBEDDING_DIMENSIONS,
                actual: SUPPORTED_EMBEDDING_DIMENSIONS - 1,
            }
        );
    }

    #[tokio::test]
    async fn rig_openai_embedder_maps_provider_errors() {
        let embedder = RigOpenAiEmbedder::new(
            EmbeddingConfig::default(),
            FakeProvider {
                result: Err(ProviderError::Request("provider down".to_string())),
            },
        );

        let error = embedder.embed("hello").await.unwrap_err();

        assert_eq!(error, EmbedderError::Provider("provider down".to_string()));
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
    async fn finds_missing_entries_oldest_first_with_limit() {
        let (journal_repository, embedding_repository) = setup().await;
        let second = store_entry(&journal_repository, "2", "second", at(11, 0)).await;
        let first = store_entry(&journal_repository, "1", "first", at(10, 0)).await;
        let third = store_entry(&journal_repository, "3", "third", at(12, 0)).await;

        embedding_repository
            .store_embedding(
                second,
                TEST_EMBEDDING_MODEL,
                TEST_EMBEDDING_DIMENSIONS,
                &embedding(1.0),
            )
            .await
            .unwrap();

        let missing = embedding_repository
            .find_entries_missing_embedding(TEST_EMBEDDING_MODEL, 2)
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
                TEST_EMBEDDING_MODEL,
                TEST_EMBEDDING_DIMENSIONS,
                &embedding(1.0),
            )
            .await
            .unwrap();

        assert_eq!(
            embedding_repository
                .count_entries_missing_embedding(TEST_EMBEDDING_MODEL)
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

    #[tokio::test]
    async fn backfill_generates_missing_embeddings_with_limit_oldest_first() {
        let (journal_repository, embedding_repository) = setup().await;
        let first = store_entry(&journal_repository, "1", "first", at(10, 0)).await;
        let second = store_entry(&journal_repository, "2", "second", at(11, 0)).await;
        let third = store_entry(&journal_repository, "3", "third", at(12, 0)).await;

        let service = EmbeddingBackfillService::new(embedding_repository.clone(), FakeEmbedder);

        let result = service.backfill_missing_embeddings(2).await.unwrap();

        assert_eq!(
            result,
            BackfillResult {
                attempted: 2,
                created: 2,
                failed: 0,
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

        let first_result = service.backfill_missing_embeddings(50).await.unwrap();
        let second_result = service.backfill_missing_embeddings(50).await.unwrap();

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
            }
        );
        assert_eq!(
            second_result,
            BackfillResult {
                attempted: 0,
                created: 0,
                failed: 0,
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

        let result = service.backfill_missing_embeddings(50).await.unwrap();

        assert_eq!(
            result,
            BackfillResult {
                attempted: 2,
                created: 1,
                failed: 1,
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
                .count_entries_missing_embedding(TEST_EMBEDDING_MODEL)
                .await
                .unwrap(),
            1
        );
    }

    #[test]
    fn worker_config_uses_default_values_when_not_configured() {
        let config = EmbeddingWorkerConfig::from_values(None, None, None).unwrap();

        assert!(!config.enabled);
        assert_eq!(config.batch_size, 20);
        assert_eq!(config.interval, Duration::from_secs(300));
    }

    #[test]
    fn worker_config_enables_when_set_to_true() {
        let config =
            EmbeddingWorkerConfig::from_values(Some("true".to_string()), None, None).unwrap();

        assert!(config.enabled);
    }

    #[test]
    fn worker_config_remains_disabled_when_set_to_false() {
        let config =
            EmbeddingWorkerConfig::from_values(Some("false".to_string()), None, None).unwrap();

        assert!(!config.enabled);
    }

    #[test]
    fn worker_config_accepts_custom_batch_size_and_interval() {
        let config = EmbeddingWorkerConfig::from_values(
            None,
            Some("50".to_string()),
            Some("60".to_string()),
        )
        .unwrap();

        assert_eq!(config.batch_size, 50);
        assert_eq!(config.interval, Duration::from_secs(60));
    }

    #[test]
    fn worker_config_rejects_zero_batch_size() {
        let error =
            EmbeddingWorkerConfig::from_values(None, Some("0".to_string()), None).unwrap_err();

        assert_eq!(
            error,
            EmbeddingWorkerConfigError::InvalidBatchSize("0".to_string())
        );
        assert!(
            error
                .to_string()
                .contains("FROID_EMBEDDING_WORKER_BATCH_SIZE")
        );
    }

    #[test]
    fn worker_config_rejects_invalid_batch_size() {
        let error =
            EmbeddingWorkerConfig::from_values(None, Some("abc".to_string()), None).unwrap_err();

        assert_eq!(
            error,
            EmbeddingWorkerConfigError::InvalidBatchSize("abc".to_string())
        );
    }

    #[test]
    fn worker_config_rejects_zero_interval() {
        let error =
            EmbeddingWorkerConfig::from_values(None, None, Some("0".to_string())).unwrap_err();

        assert_eq!(
            error,
            EmbeddingWorkerConfigError::InvalidInterval("0".to_string())
        );
        assert!(
            error
                .to_string()
                .contains("FROID_EMBEDDING_WORKER_INTERVAL_SECONDS")
        );
    }

    #[test]
    fn worker_config_rejects_invalid_interval() {
        let error =
            EmbeddingWorkerConfig::from_values(None, None, Some("abc".to_string())).unwrap_err();

        assert_eq!(
            error,
            EmbeddingWorkerConfigError::InvalidInterval("abc".to_string())
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
        assert!(results[0].score <= results[1].score);
        assert!(results[1].score <= results[2].score);
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

        let result = service.backfill_missing_embeddings(50).await.unwrap();

        assert_eq!(
            result,
            BackfillResult {
                attempted: 2,
                created: 1,
                failed: 1,
            }
        );
        assert!(
            !embedding_repository
                .has_embedding(first, TEST_EMBEDDING_MODEL)
                .await
                .unwrap()
        );
        assert!(
            embedding_repository
                .has_embedding(second, TEST_EMBEDDING_MODEL)
                .await
                .unwrap()
        );
    }
}
