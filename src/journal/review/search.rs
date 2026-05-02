use std::{collections::HashMap, error::Error, fmt};

use async_trait::async_trait;

use crate::journal::{
    embedding::{Embedder, EmbedderError, EmbeddingIndex, EmbeddingRepositoryError, EmbeddingSearchResult},
    review::{DailyReview, DailyReviewSearchResult, repository::DailyReviewRepository},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DailyReviewSearchError {
    Embedder(EmbedderError),
    Index(EmbeddingRepositoryError),
    Repository(String),
}

impl fmt::Display for DailyReviewSearchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Embedder(e) => write!(f, "failed to embed search query: {e}"),
            Self::Index(e) => write!(f, "vector search failed: {e}"),
            Self::Repository(e) => write!(f, "failed to load daily reviews: {e}"),
        }
    }
}

impl Error for DailyReviewSearchError {}

#[async_trait]
pub trait DailyReviewSearchService: Send + Sync {
    async fn search(
        &self,
        user_id: &str,
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
        user_id: &str,
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
            .search_for_user(user_id, &embedding, model, 5)
            .await
            .map_err(DailyReviewSearchError::Index)?;

        if index_results.is_empty() {
            return Ok(vec![]);
        }

        let ids: Vec<i64> = index_results.iter().map(|r| r.id).collect();

        let loaded = self
            .repository
            .fetch_by_ids(user_id, &ids)
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
