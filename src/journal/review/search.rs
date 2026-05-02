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

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;
    use sqlx::SqlitePool;

    use super::*;
    use crate::{
        database,
        journal::{
            embedding::{
                Embedder, EmbedderError, Embedding, EmbeddingCandidate, SUPPORTED_EMBEDDING_DIMENSIONS,
            },
            review::repository::DailyReviewRepository,
        },
    };

    async fn setup() -> (DailyReviewRepository, FakeIndex) {
        database::register_sqlite_vec_extension();
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();

        (
            DailyReviewRepository::new(pool),
            FakeIndex { results: vec![] },
        )
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

    #[derive(Clone)]
    struct FakeIndex {
        results: Vec<EmbeddingSearchResult<i64>>,
    }

    #[async_trait]
    impl EmbeddingIndex<i64> for FakeIndex {
        async fn store_embedding(
            &self,
            _id: i64,
            _embedding_model: &str,
            _embedding_dim: usize,
            _embedding: &Embedding,
        ) -> Result<bool, EmbeddingRepositoryError> {
            unreachable!()
        }

        async fn record_embedding_failure(
            &self,
            _id: i64,
            _embedding_model: &str,
            _error_message: &str,
        ) -> Result<(), EmbeddingRepositoryError> {
            unreachable!()
        }

        async fn delete_failed_embedding(
            &self,
            _id: i64,
            _embedding_model: &str,
        ) -> Result<bool, EmbeddingRepositoryError> {
            unreachable!()
        }

        async fn find_entries_missing_or_failed_embedding(
            &self,
            _embedding_model: &str,
            _limit: u32,
        ) -> Result<Vec<EmbeddingCandidate<i64>>, EmbeddingRepositoryError> {
            unreachable!()
        }

        async fn count_entries_missing_or_failed_embedding(
            &self,
            _embedding_model: &str,
        ) -> Result<u32, EmbeddingRepositoryError> {
            unreachable!()
        }

        async fn search_for_user(
            &self,
            _user_id: &str,
            _embedding: &Embedding,
            _embedding_model: &str,
            _limit: usize,
        ) -> Result<Vec<EmbeddingSearchResult<i64>>, EmbeddingRepositoryError> {
            Ok(self.results.clone())
        }
    }

    #[tokio::test]
    async fn search_returns_mapped_review_results() {
        let (repo, mut index) = setup().await;
        let date = NaiveDate::from_ymd_opt(2026, 4, 28).unwrap();
        let review = repo
            .upsert_completed("user-1", date, "review text", "model", "v1")
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

        let results = service.search("user-1", "query").await.unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].review.review_text, Some("review text".to_string()));
        assert_eq!(results[0].distance, 0.1);
    }

    #[tokio::test]
    async fn search_filters_out_reviews_for_other_users() {
        let (repo, mut index) = setup().await;
        let date = NaiveDate::from_ymd_opt(2026, 4, 28).unwrap();
        let review = repo
            .upsert_completed("user-2", date, "other user review", "model", "v1")
            .await
            .unwrap();

        // Index returns the review, but repository.fetch_by_ids will scope by user-1
        index.results = vec![EmbeddingSearchResult {
            id: review.id,
            distance: 0.1,
        }];

        let service = SemanticDailyReviewSearchService::new(
            index,
            FakeEmbedder::succeeds(TEST_MODEL, 0),
            repo,
        );

        let results = service.search("user-1", "query").await.unwrap();

        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn search_returns_empty_when_index_is_empty() {
        let (repo, index) = setup().await;
        let service = SemanticDailyReviewSearchService::new(
            index,
            FakeEmbedder::succeeds(TEST_MODEL, 0),
            repo,
        );

        let results = service.search("user-1", "query").await.unwrap();

        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn search_returns_error_when_embedder_fails() {
        let (repo, index) = setup().await;
        let service = SemanticDailyReviewSearchService::new(
            index,
            FakeEmbedder::fails(TEST_MODEL),
            repo,
        );

        let err = service.search("user-1", "query").await.unwrap_err();

        assert!(matches!(err, DailyReviewSearchError::Embedder(_)));
    }
}
