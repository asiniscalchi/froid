use std::{collections::HashMap, error::Error, fmt};

use async_trait::async_trait;

use super::{
    embedding::{Embedder, EmbedderError, EmbeddingIndex, EmbeddingRepositoryError},
    entry::JournalEntry,
    repository::JournalRepository,
};

pub const DEFAULT_SEARCH_LIMIT: usize = 5;
pub const MAX_SEARCH_LIMIT: usize = 20;

#[derive(Debug, Clone, PartialEq)]
pub struct SemanticSearchResult {
    pub journal_entry: JournalEntry,
    pub score: f32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SemanticSearchError {
    Embedder(EmbedderError),
    Index(EmbeddingRepositoryError),
    Repository(String),
}

impl fmt::Display for SemanticSearchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Embedder(e) => write!(f, "failed to embed search query: {e}"),
            Self::Index(e) => write!(f, "vector search failed: {e}"),
            Self::Repository(e) => write!(f, "failed to load journal entries: {e}"),
        }
    }
}

impl Error for SemanticSearchError {}

#[async_trait]
pub(crate) trait SearchService: Send + Sync {
    async fn search(
        &self,
        user_id: &str,
        query: &str,
    ) -> Result<Vec<SemanticSearchResult>, SemanticSearchError>;
}

#[derive(Clone)]
pub struct SemanticSearchService<I, E> {
    index: I,
    embedder: E,
    repository: JournalRepository,
}

impl<I, E> SemanticSearchService<I, E>
where
    I: EmbeddingIndex,
    E: Embedder,
{
    pub fn new(index: I, embedder: E, repository: JournalRepository) -> Self {
        Self {
            index,
            embedder,
            repository,
        }
    }
}

#[async_trait]
impl<I, E> SearchService for SemanticSearchService<I, E>
where
    I: EmbeddingIndex + Send + Sync,
    E: Embedder + Send + Sync,
{
    async fn search(
        &self,
        user_id: &str,
        query: &str,
    ) -> Result<Vec<SemanticSearchResult>, SemanticSearchError> {
        let embedding = self
            .embedder
            .embed(query)
            .await
            .map_err(SemanticSearchError::Embedder)?;

        let model = self.embedder.model();

        let index_results = self
            .index
            .search(&embedding, model, MAX_SEARCH_LIMIT)
            .await
            .map_err(SemanticSearchError::Index)?;

        if index_results.is_empty() {
            return Ok(vec![]);
        }

        let ids: Vec<i64> = index_results.iter().map(|r| r.journal_entry_id).collect();

        let loaded = self
            .repository
            .fetch_by_ids(user_id, &ids)
            .await
            .map_err(|e| SemanticSearchError::Repository(e.to_string()))?;

        let entry_map: HashMap<i64, JournalEntry> = loaded.into_iter().collect();

        let results = index_results
            .into_iter()
            .filter_map(|r| {
                entry_map
                    .get(&r.journal_entry_id)
                    .map(|entry| SemanticSearchResult {
                        journal_entry: entry.clone(),
                        score: r.score,
                    })
            })
            .take(DEFAULT_SEARCH_LIMIT)
            .collect();

        Ok(results)
    }
}

pub fn format_search_results(query: &str, results: &[SemanticSearchResult]) -> String {
    let header = format!("Search results for: {query}");
    let entries = results
        .iter()
        .enumerate()
        .map(|(i, r)| {
            format!(
                "{}. {}\n{}",
                i + 1,
                r.journal_entry.received_at.format("%Y-%m-%d %H:%M"),
                r.journal_entry.text
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    format!("{header}\n\n{entries}")
}

pub fn search_usage_response() -> String {
    "Usage: /search <query>\n\nExample:\n/search anxiety before meetings".to_string()
}

pub fn search_empty_response() -> String {
    "No results found.".to_string()
}

pub fn search_error_response(error: &SemanticSearchError) -> String {
    format!("Search failed: {error}")
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, TimeZone, Utc};
    use sqlx::SqlitePool;

    use super::*;
    use crate::{
        database,
        journal::{
            embedding::{
                EmbedderError, Embedding, EmbeddingIndex, SUPPORTED_EMBEDDING_DIMENSIONS,
                SqliteEmbeddingRepository,
            },
            repository::JournalRepository,
        },
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

    fn at(h: u32, m: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 4, 28, h, m, 0).unwrap()
    }

    fn incoming(msg_id: &str, text: &str, received_at: DateTime<Utc>) -> IncomingMessage {
        IncomingMessage {
            source: MessageSource::Telegram,
            source_conversation_id: "42".to_string(),
            source_message_id: msg_id.to_string(),
            user_id: "7".to_string(),
            text: text.to_string(),
            received_at,
        }
    }

    async fn store_and_embed(
        repo: &JournalRepository,
        index: &SqliteEmbeddingRepository,
        msg_id: &str,
        text: &str,
        received_at: DateTime<Utc>,
        dim: usize,
    ) -> i64 {
        repo.store(&incoming(msg_id, text, received_at))
            .await
            .unwrap();

        let entry_id: i64 = sqlx::query_scalar(
            "SELECT id FROM journal_entries WHERE source = 'telegram' AND source_message_id = ?",
        )
        .bind(msg_id)
        .fetch_one(repo.pool())
        .await
        .unwrap();

        let embedding = directional_embedding(dim);
        index
            .store_embedding(
                entry_id,
                TEST_MODEL,
                SUPPORTED_EMBEDDING_DIMENSIONS,
                &embedding,
            )
            .await
            .unwrap();

        entry_id
    }

    const TEST_MODEL: &str = "test-model";

    fn directional_embedding(nonzero_dim: usize) -> Embedding {
        let mut values = vec![0.0f32; SUPPORTED_EMBEDDING_DIMENSIONS];
        values[nonzero_dim] = 1.0;
        Embedding::new(values, SUPPORTED_EMBEDDING_DIMENSIONS).unwrap()
    }

    #[derive(Clone)]
    struct FakeEmbedder {
        model: String,
        result: Result<Embedding, EmbedderError>,
    }

    impl FakeEmbedder {
        fn succeeds(model: &str, dim: usize) -> Self {
            Self {
                model: model.to_string(),
                result: Ok(directional_embedding(dim)),
            }
        }

        fn fails(model: &str) -> Self {
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

    fn make_service(
        index: SqliteEmbeddingRepository,
        embedder: FakeEmbedder,
        repo: JournalRepository,
    ) -> SemanticSearchService<SqliteEmbeddingRepository, FakeEmbedder> {
        SemanticSearchService::new(index, embedder, repo)
    }

    #[tokio::test]
    async fn search_returns_results_ordered_by_similarity() {
        let (repo, index) = setup().await;

        // Entry at dim 1 is closest to query at dim 1.
        let _ = store_and_embed(&repo, &index, "1", "irrelevant entry", at(10, 0), 0).await;
        let closest =
            store_and_embed(&repo, &index, "2", "most relevant entry", at(11, 0), 1).await;
        let _ = store_and_embed(&repo, &index, "3", "another irrelevant", at(12, 0), 2).await;

        let service = make_service(index, FakeEmbedder::succeeds(TEST_MODEL, 1), repo);

        let results = service.search("7", "query").await.unwrap();

        assert!(!results.is_empty());
        assert_eq!(results[0].journal_entry.text, "most relevant entry");
        let _ = closest;
    }

    #[tokio::test]
    async fn search_returns_empty_when_no_embeddings_exist() {
        let (repo, index) = setup().await;
        repo.store(&incoming("1", "some text", at(10, 0)))
            .await
            .unwrap();

        let service = make_service(index, FakeEmbedder::succeeds(TEST_MODEL, 0), repo);

        let results = service.search("7", "query").await.unwrap();

        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn search_returns_error_when_embedder_fails() {
        let (repo, index) = setup().await;

        let service = make_service(index, FakeEmbedder::fails(TEST_MODEL), repo);

        let error = service.search("7", "query").await.unwrap_err();

        assert!(matches!(error, SemanticSearchError::Embedder(_)));
    }

    #[tokio::test]
    async fn search_filters_out_embeddings_for_other_models() {
        let (repo, index) = setup().await;

        // Store an embedding under a different model.
        store_and_embed(&repo, &index, "1", "some text", at(10, 0), 0).await;

        // Query with a model that has no stored embeddings.
        let service = make_service(index, FakeEmbedder::succeeds("other-model", 0), repo);

        let results = service.search("7", "query").await.unwrap();

        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn search_preserves_similarity_ordering_from_index() {
        let (repo, index) = setup().await;

        // Entries at dims 0, 1, 2. Query at dim 1 makes entry 1 (dim 1) closest.
        store_and_embed(&repo, &index, "1", "entry A", at(10, 0), 2).await;
        store_and_embed(&repo, &index, "2", "entry B", at(11, 0), 1).await;
        store_and_embed(&repo, &index, "3", "entry C", at(12, 0), 0).await;

        let service = make_service(index, FakeEmbedder::succeeds(TEST_MODEL, 1), repo);

        let results = service.search("7", "query").await.unwrap();

        assert_eq!(results[0].journal_entry.text, "entry B");
        for window in results.windows(2) {
            assert!(window[0].score <= window[1].score);
        }
    }

    #[tokio::test]
    async fn search_skips_entries_not_belonging_to_the_searching_user() {
        let (repo, index) = setup().await;

        // Entry for user "7" at dim 0.
        store_and_embed(&repo, &index, "1", "real entry", at(10, 0), 0).await;

        // Entry for a different user at dim 1 — closer to the query vector at dim 1.
        let other_msg = IncomingMessage {
            source: MessageSource::Telegram,
            source_conversation_id: "99".to_string(),
            source_message_id: "2".to_string(),
            user_id: "other_user".to_string(),
            text: "other user entry".to_string(),
            received_at: at(11, 0),
        };
        repo.store(&other_msg).await.unwrap();
        let other_id: i64 = sqlx::query_scalar(
            "SELECT id FROM journal_entries WHERE source = 'telegram' AND source_message_id = '2'",
        )
        .fetch_one(repo.pool())
        .await
        .unwrap();
        index
            .store_embedding(
                other_id,
                TEST_MODEL,
                SUPPORTED_EMBEDDING_DIMENSIONS,
                &directional_embedding(1),
            )
            .await
            .unwrap();

        // Query at dim 1 — other user's entry is closest, but user "7" must not see it.
        let service = make_service(index, FakeEmbedder::succeeds(TEST_MODEL, 1), repo);

        let results = service.search("7", "query").await.unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].journal_entry.text, "real entry");
    }

    #[tokio::test]
    async fn search_applies_default_limit() {
        let (repo, index) = setup().await;

        for i in 0..(DEFAULT_SEARCH_LIMIT + 2) {
            store_and_embed(
                &repo,
                &index,
                &i.to_string(),
                &format!("entry {i}"),
                at(10, i as u32),
                i,
            )
            .await;
        }

        let service = make_service(index, FakeEmbedder::succeeds(TEST_MODEL, 0), repo);

        let results = service.search("7", "query").await.unwrap();

        assert_eq!(results.len(), DEFAULT_SEARCH_LIMIT);
    }

    #[tokio::test]
    async fn format_search_results_formats_numbered_entries() {
        let results = vec![
            SemanticSearchResult {
                journal_entry: JournalEntry {
                    text: "felt nervous".to_string(),
                    received_at: Utc.with_ymd_and_hms(2026, 4, 29, 9, 12, 0).unwrap(),
                },
                score: 0.1,
            },
            SemanticSearchResult {
                journal_entry: JournalEntry {
                    text: "avoid calls".to_string(),
                    received_at: Utc.with_ymd_and_hms(2026, 4, 20, 18, 44, 0).unwrap(),
                },
                score: 0.2,
            },
        ];

        let text = format_search_results("anxiety", &results);

        assert!(text.starts_with("Search results for: anxiety"));
        assert!(text.contains("1. 2026-04-29 09:12"));
        assert!(text.contains("felt nervous"));
        assert!(text.contains("2. 2026-04-20 18:44"));
        assert!(text.contains("avoid calls"));
    }
}
