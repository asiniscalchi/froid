use std::time::Duration;

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

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM journal_entry_embedding_metadata")
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
    let config = EmbeddingWorkerConfig::from_values(Some("true".to_string()), None, None).unwrap();

    assert!(config.enabled);
}

#[test]
fn worker_config_remains_disabled_when_set_to_false() {
    let config = EmbeddingWorkerConfig::from_values(Some("false".to_string()), None, None).unwrap();

    assert!(!config.enabled);
}

#[test]
fn worker_config_accepts_custom_batch_size_and_interval() {
    let config =
        EmbeddingWorkerConfig::from_values(None, Some("50".to_string()), Some("60".to_string()))
            .unwrap();

    assert_eq!(config.batch_size, 50);
    assert_eq!(config.interval, Duration::from_secs(60));
}

#[test]
fn worker_config_rejects_zero_batch_size() {
    let error = EmbeddingWorkerConfig::from_values(None, Some("0".to_string()), None).unwrap_err();

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
    let error = EmbeddingWorkerConfig::from_values(None, None, Some("0".to_string())).unwrap_err();

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
