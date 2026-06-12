use std::collections::HashMap;

use async_trait::async_trait;
use thiserror::Error;

use crate::journal::{
    embedding::{
        Embedder, EmbedderError, EmbeddingIndex, EmbeddingRepositoryError, EmbeddingSearchResult,
    },
    review::{DailyReview, DailyReviewSearchResult, repository::DailyReviewRepository},
};

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DailyReviewSearchError {
    #[error("failed to embed search query: {0}")]
    Embedder(EmbedderError),
    #[error("vector search failed: {0}")]
    Index(EmbeddingRepositoryError),
    #[error("failed to load daily reviews: {0}")]
    Repository(String),
}

#[async_trait]
pub trait DailyReviewSearchService: Send + Sync {
    async fn search(
        &self,
        query: &str,
    ) -> Result<Vec<DailyReviewSearchResult>, DailyReviewSearchError>;
}

#[derive(Clone)]
pub struct SemanticDailyReviewSearchService<I, E> {
    index: I,
    embedder: E,
    repository: DailyReviewRepository,
}

impl<I, E> SemanticDailyReviewSearchService<I, E>
where
    I: EmbeddingIndex<i64>,
    E: Embedder,
{
    pub fn new(index: I, embedder: E, repository: DailyReviewRepository) -> Self {
        Self {
            index,
            embedder,
            repository,
        }
    }
}

#[async_trait]
impl<I, E> DailyReviewSearchService for SemanticDailyReviewSearchService<I, E>
where
    I: EmbeddingIndex<i64> + Send + Sync,
    E: Embedder + Send + Sync,
{
    async fn search(
        &self,
        query: &str,
    ) -> Result<Vec<DailyReviewSearchResult>, DailyReviewSearchError> {
        let embedding = self
            .embedder
            .embed(query)
            .await
            .map_err(DailyReviewSearchError::Embedder)?;

        let model = self.embedder.model();

        let index_results: Vec<EmbeddingSearchResult<i64>> = self
            .index
            .search(&embedding, model, None, None, 5)
            .await
            .map_err(DailyReviewSearchError::Index)?;

        if index_results.is_empty() {
            return Ok(vec![]);
        }

        let ids: Vec<i64> = index_results.iter().map(|r| r.id).collect();

        let loaded = self
            .repository
            .fetch_by_ids(&ids)
            .await
            .map_err(|e| DailyReviewSearchError::Repository(e.to_string()))?;

        let review_map: HashMap<i64, DailyReview> = loaded.into_iter().collect();

        let results = index_results
            .into_iter()
            .filter_map(|r| {
                review_map.get(&r.id).map(|review| DailyReviewSearchResult {
                    review: review.clone(),
                    distance: r.distance,
                })
            })
            .collect();

        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;

    use super::*;
    use crate::journal::{
        embedding::test_support::{FakeEmbedder, PresetIndex},
        review::repository::DailyReviewRepository,
    };

    async fn setup() -> (DailyReviewRepository, PresetIndex<i64>) {
        let pool = crate::database::test_pool().await;

        (
            DailyReviewRepository::new(pool),
            PresetIndex { results: vec![] },
        )
    }

    const TEST_MODEL: &str = "test-model";

    #[tokio::test]
    async fn search_returns_mapped_review_results() {
        let (repo, mut index) = setup().await;
        let date = NaiveDate::from_ymd_opt(2026, 4, 28).unwrap();
        let review = repo
            .upsert_completed(date, "review text", "model", "v1")
            .await
            .unwrap();

        index.results = vec![EmbeddingSearchResult {
            id: review.id,
            distance: 0.1,
        }];

        let service = SemanticDailyReviewSearchService::new(
            index,
            FakeEmbedder::succeeds(TEST_MODEL, 0),
            repo,
        );

        let results = service.search("query").await.unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0].review.review_text,
            Some("review text".to_string())
        );
        assert_eq!(results[0].distance, 0.1);
    }

    #[tokio::test]
    async fn search_returns_reviews_from_single_user_journal() {
        let (repo, mut index) = setup().await;
        let date = NaiveDate::from_ymd_opt(2026, 4, 28).unwrap();
        let review = repo
            .upsert_completed(date, "other user review", "model", "v1")
            .await
            .unwrap();

        // Index returns the review and the repository uses the single local journal.
        index.results = vec![EmbeddingSearchResult {
            id: review.id,
            distance: 0.1,
        }];

        let service = SemanticDailyReviewSearchService::new(
            index,
            FakeEmbedder::succeeds(TEST_MODEL, 0),
            repo,
        );

        let results = service.search("query").await.unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0].review.review_text,
            Some("other user review".to_string())
        );
    }

    #[tokio::test]
    async fn search_returns_empty_when_index_is_empty() {
        let (repo, index) = setup().await;
        let service = SemanticDailyReviewSearchService::new(
            index,
            FakeEmbedder::succeeds(TEST_MODEL, 0),
            repo,
        );

        let results = service.search("query").await.unwrap();

        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn search_returns_error_when_embedder_fails() {
        let (repo, index) = setup().await;
        let service =
            SemanticDailyReviewSearchService::new(index, FakeEmbedder::fails(TEST_MODEL), repo);

        let err = service.search("query").await.unwrap_err();

        assert!(matches!(err, DailyReviewSearchError::Embedder(_)));
    }
}
