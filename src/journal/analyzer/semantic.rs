use std::collections::HashMap;

use async_trait::async_trait;
use chrono::NaiveDate;

use crate::journal::embedding::{Embedder, EmbeddingIndex};
use crate::journal::entry::JournalEntry;
use crate::journal::repository::JournalRepository;

use super::types::{AnalyzerError, SemanticHit};

#[async_trait]
pub trait SemanticJournalSearcher: Send + Sync {
    /// Returns up to `limit` journal entries semantically similar to `query`,
    /// scoped to the optional date bounds.
    async fn search(
        &self,
        query: &str,
        from_date: Option<NaiveDate>,
        to_date_exclusive: Option<NaiveDate>,
        limit: usize,
    ) -> Result<Vec<SemanticHit>, AnalyzerError>;
}

#[derive(Debug, Clone)]
pub struct DefaultSemanticJournalSearcher<I, E> {
    index: I,
    embedder: E,
    repository: JournalRepository,
}

impl<I, E> DefaultSemanticJournalSearcher<I, E>
where
    I: EmbeddingIndex<String>,
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
impl<I, E> SemanticJournalSearcher for DefaultSemanticJournalSearcher<I, E>
where
    I: EmbeddingIndex<String> + Send + Sync,
    E: Embedder + Send + Sync,
{
    async fn search(
        &self,
        query: &str,
        from_date: Option<NaiveDate>,
        to_date_exclusive: Option<NaiveDate>,
        limit: usize,
    ) -> Result<Vec<SemanticHit>, AnalyzerError> {
        let embedding = self
            .embedder
            .embed(query)
            .await
            .map_err(|e| AnalyzerError::Internal(Box::new(e)))?;

        let model = self.embedder.model();

        let index_results = self
            .index
            .search(&embedding, model, from_date, to_date_exclusive, limit)
            .await
            .map_err(|e| AnalyzerError::Internal(Box::new(e)))?;

        if index_results.is_empty() {
            return Ok(Vec::new());
        }

        let ids: Vec<String> = index_results.iter().map(|r| r.id.clone()).collect();
        let loaded = self
            .repository
            .fetch_by_ids(&ids)
            .await
            .map_err(|e| AnalyzerError::Internal(Box::new(e)))?;

        let entry_map: HashMap<String, JournalEntry> = loaded.into_iter().collect();

        Ok(index_results
            .into_iter()
            .filter_map(|r| {
                entry_map.get(&r.id).map(|entry| SemanticHit {
                    id: r.id.clone(),
                    received_at: entry.received_at,
                    text: entry.text.clone(),
                    distance: r.distance,
                })
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {

    use chrono::{DateTime, TimeZone, Utc};

    use super::*;

    use crate::journal::embedding::{
        EmbeddingSearchResult, SUPPORTED_EMBEDDING_DIMENSIONS, SqliteEmbeddingRepository,
        test_support::{FakeEmbedder, PresetIndex, directional_embedding},
    };
    use crate::messages::{IncomingMessage, MessageSource};

    const TEST_MODEL: &str = "test-model";

    async fn setup() -> (JournalRepository, SqliteEmbeddingRepository) {
        let pool = crate::database::test_pool().await;
        (
            JournalRepository::new(pool.clone()),
            SqliteEmbeddingRepository::new(pool),
        )
    }

    fn at(h: u32, m: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 4, 28, h, m, 0).unwrap()
    }

    async fn store_entry(
        repo: &JournalRepository,
        index: &SqliteEmbeddingRepository,
        msg_id: &str,
        text: &str,
        received_at: DateTime<Utc>,
        dim: usize,
    ) -> String {
        let msg = IncomingMessage {
            source: MessageSource::Telegram,
            source_conversation_id: "42".to_string(),
            source_message_id: msg_id.to_string(),
            text: text.to_string(),
            received_at,
        };
        repo.store(&msg).await.unwrap();
        let id: String = sqlx::query_scalar(
            "SELECT id FROM journal_entries WHERE source = 'telegram' AND source_message_id = ?",
        )
        .bind(msg_id)
        .fetch_one(repo.pool())
        .await
        .unwrap();
        index
            .store_embedding(
                &id,
                TEST_MODEL,
                SUPPORTED_EMBEDDING_DIMENSIONS,
                &directional_embedding(dim),
            )
            .await
            .unwrap();
        id
    }

    #[tokio::test]
    async fn search_returns_hits_ordered_by_distance() {
        let (repo, index) = setup().await;
        store_entry(&repo, &index, "1", "irrelevant", at(10, 0), 0).await;
        store_entry(&repo, &index, "2", "closest", at(11, 0), 1).await;
        store_entry(&repo, &index, "3", "also irrelevant", at(12, 0), 2).await;

        let searcher =
            DefaultSemanticJournalSearcher::new(index, FakeEmbedder::succeeds(TEST_MODEL, 1), repo);

        let hits = searcher.search("query", None, None, 10).await.unwrap();

        assert!(!hits.is_empty());
        assert_eq!(hits[0].text, "closest");
        for window in hits.windows(2) {
            assert!(window[0].distance <= window[1].distance);
        }
    }

    #[tokio::test]
    async fn search_respects_caller_supplied_limit() {
        let (repo, index) = setup().await;
        for i in 0..6usize {
            store_entry(
                &repo,
                &index,
                &i.to_string(),
                &format!("entry {i}"),
                at(10, i as u32),
                i,
            )
            .await;
        }
        let searcher =
            DefaultSemanticJournalSearcher::new(index, FakeEmbedder::succeeds(TEST_MODEL, 0), repo);

        let hits = searcher.search("query", None, None, 3).await.unwrap();

        assert_eq!(hits.len(), 3);
    }

    #[tokio::test]
    async fn search_uses_single_user_scope() {
        let (repo, index) = setup().await;
        store_entry(&repo, &index, "1", "mine", at(10, 0), 0).await;

        let other = IncomingMessage {
            source: MessageSource::Telegram,
            source_conversation_id: "99".to_string(),
            source_message_id: "2".to_string(),
            text: "theirs".to_string(),
            received_at: at(11, 0),
        };
        repo.store(&other).await.unwrap();
        let other_id: String = sqlx::query_scalar(
            "SELECT id FROM journal_entries WHERE source = 'telegram' AND source_message_id = '2'",
        )
        .fetch_one(repo.pool())
        .await
        .unwrap();
        index
            .store_embedding(
                &other_id,
                TEST_MODEL,
                SUPPORTED_EMBEDDING_DIMENSIONS,
                &directional_embedding(0),
            )
            .await
            .unwrap();

        let searcher =
            DefaultSemanticJournalSearcher::new(index, FakeEmbedder::succeeds(TEST_MODEL, 0), repo);

        let hits = searcher.search("query", None, None, 10).await.unwrap();

        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].text, "mine");
        assert_eq!(hits[1].text, "theirs");
    }

    #[tokio::test]
    async fn search_maps_embedder_failure_to_internal_error() {
        let (repo, index) = setup().await;
        let searcher =
            DefaultSemanticJournalSearcher::new(index, FakeEmbedder::fails(TEST_MODEL), repo);

        let err = searcher.search("query", None, None, 5).await.unwrap_err();

        assert!(matches!(err, AnalyzerError::Internal(_)));
    }

    #[tokio::test]
    async fn search_skips_index_results_that_do_not_belong_to_user() {
        let (repo, _) = setup().await;
        let kept = {
            let msg = IncomingMessage {
                source: MessageSource::Telegram,
                source_conversation_id: "42".to_string(),
                source_message_id: "1".to_string(),
                text: "kept".to_string(),
                received_at: at(10, 0),
            };
            repo.store(&msg).await.unwrap();
            sqlx::query_scalar::<_, String>(
                "SELECT id FROM journal_entries WHERE source_message_id = '1'",
            )
            .fetch_one(repo.pool())
            .await
            .unwrap()
        };

        let stub_index = PresetIndex {
            results: vec![
                EmbeddingSearchResult {
                    id: "missing".to_string(),
                    distance: 0.1,
                },
                EmbeddingSearchResult {
                    id: kept,
                    distance: 0.2,
                },
            ],
        };
        let searcher = DefaultSemanticJournalSearcher::new(
            stub_index,
            FakeEmbedder::succeeds(TEST_MODEL, 0),
            repo,
        );

        let hits = searcher.search("query", None, None, 5).await.unwrap();

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].text, "kept");
        assert_eq!(hits[0].distance, 0.2);
    }
}
