use chrono::NaiveDate;

use crate::journal::embedding::{EmbeddingSchema, SqliteVectorIndex};

/// Schema of the daily-review embeddings.
pub struct DailyReviewEmbeddings;

impl EmbeddingSchema for DailyReviewEmbeddings {
    type Id = i64;
    /// `review_date` is a date-only TEXT column, so bounds bind as strings.
    type DateBound = String;

    const METADATA_TABLE: &'static str = "daily_review_embedding_metadata";
    const VEC_TABLE: &'static str = "daily_review_embedding_vec";
    const OWNER_ID_COLUMN: &'static str = "daily_review_id";
    const OWNER_TABLE: &'static str = "daily_reviews";
    const TEXT_COLUMN: &'static str = "review_text";
    const CANDIDATE_PREDICATE: &'static str = "daily_reviews.review_text IS NOT NULL";
    const DATE_COLUMN: &'static str = "review_date";

    fn date_bound(date: NaiveDate) -> String {
        date.to_string()
    }
}

pub type SqliteDailyReviewEmbeddingRepository = SqliteVectorIndex<DailyReviewEmbeddings>;

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;

    use super::*;
    use crate::journal::embedding::Embedding;
    use crate::journal::{
        embedding::SUPPORTED_EMBEDDING_DIMENSIONS, review::repository::DailyReviewRepository,
    };

    async fn setup() -> (DailyReviewRepository, SqliteDailyReviewEmbeddingRepository) {
        let pool = crate::database::test_pool().await;

        (
            DailyReviewRepository::new(pool.clone()),
            SqliteDailyReviewEmbeddingRepository::new(pool),
        )
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

    #[tokio::test]
    async fn stores_and_finds_daily_review_embeddings() {
        let (review_repo, embedding_repo) = setup().await;
        let date = NaiveDate::from_ymd_opt(2026, 4, 28).unwrap();
        let review = review_repo
            .upsert_completed(date, "review text", "model", "v1")
            .await
            .unwrap();

        let created = embedding_repo
            .store_embedding(
                &review.id,
                TEST_EMBEDDING_MODEL,
                TEST_EMBEDDING_DIMENSIONS,
                &embedding(1.0),
            )
            .await
            .unwrap();

        assert!(created);

        let candidates = embedding_repo
            .find_entries_missing_or_failed_embedding(TEST_EMBEDDING_MODEL, 10)
            .await
            .unwrap();

        assert_eq!(candidates.len(), 0);
    }

    #[tokio::test]
    async fn finds_missing_review_embeddings() {
        let (review_repo, embedding_repo) = setup().await;
        let date = NaiveDate::from_ymd_opt(2026, 4, 28).unwrap();
        let review = review_repo
            .upsert_completed(date, "review text", "model", "v1")
            .await
            .unwrap();

        let candidates = embedding_repo
            .find_entries_missing_or_failed_embedding(TEST_EMBEDDING_MODEL, 10)
            .await
            .unwrap();

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].id, review.id);
        assert_eq!(candidates[0].raw_text, "review text");
    }

    #[tokio::test]
    async fn records_embedding_failure_inserts_failed_row() {
        let (review_repo, embedding_repo) = setup().await;
        let date = NaiveDate::from_ymd_opt(2026, 4, 28).unwrap();
        let review = review_repo
            .upsert_completed(date, "review text", "model", "v1")
            .await
            .unwrap();

        embedding_repo
            .record_embedding_failure(&review.id, TEST_EMBEDDING_MODEL, "provider error")
            .await
            .unwrap();

        let candidates = embedding_repo
            .find_entries_missing_or_failed_embedding(TEST_EMBEDDING_MODEL, 10)
            .await
            .unwrap();

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].id, review.id);
    }

    #[tokio::test]
    async fn delete_failed_embedding_removes_failed_row() {
        let (review_repo, embedding_repo) = setup().await;
        let date = NaiveDate::from_ymd_opt(2026, 4, 28).unwrap();
        let review = review_repo
            .upsert_completed(date, "review text", "model", "v1")
            .await
            .unwrap();

        embedding_repo
            .record_embedding_failure(&review.id, TEST_EMBEDDING_MODEL, "provider error")
            .await
            .unwrap();

        let deleted = embedding_repo
            .delete_failed_embedding(&review.id, TEST_EMBEDDING_MODEL)
            .await
            .unwrap();

        assert!(deleted);

        let candidates = embedding_repo
            .find_entries_missing_or_failed_embedding(TEST_EMBEDDING_MODEL, 10)
            .await
            .unwrap();

        assert_eq!(candidates.len(), 1);
    }

    #[tokio::test]
    async fn search_returns_results_ordered_by_cosine_distance() {
        let (review_repo, embedding_repo) = setup().await;
        let date1 = NaiveDate::from_ymd_opt(2026, 4, 28).unwrap();
        let date2 = NaiveDate::from_ymd_opt(2026, 4, 29).unwrap();
        let date3 = NaiveDate::from_ymd_opt(2026, 4, 30).unwrap();

        let review1 = review_repo
            .upsert_completed(date1, "review 1", "model", "v1")
            .await
            .unwrap();
        let review2 = review_repo
            .upsert_completed(date2, "review 2", "model", "v1")
            .await
            .unwrap();
        let review3 = review_repo
            .upsert_completed(date3, "review 3", "model", "v1")
            .await
            .unwrap();

        // Directional embeddings: 1 is closest to query 1, then 2, then 3 is furthest.
        embedding_repo
            .store_embedding(
                &review1.id,
                TEST_EMBEDDING_MODEL,
                TEST_EMBEDDING_DIMENSIONS,
                &directional_embedding(1),
            )
            .await
            .unwrap();
        embedding_repo
            .store_embedding(
                &review2.id,
                TEST_EMBEDDING_MODEL,
                TEST_EMBEDDING_DIMENSIONS,
                &directional_embedding(2),
            )
            .await
            .unwrap();
        embedding_repo
            .store_embedding(
                &review3.id,
                TEST_EMBEDDING_MODEL,
                TEST_EMBEDDING_DIMENSIONS,
                &directional_embedding(3),
            )
            .await
            .unwrap();

        let query = directional_embedding(1);
        let results = embedding_repo
            .search(&query, TEST_EMBEDDING_MODEL, None, None, 10)
            .await
            .unwrap();

        assert_eq!(results.len(), 3);
        assert_eq!(results[0].id, review1.id);
        assert_eq!(results[1].id, review2.id);
        assert_eq!(results[2].id, review3.id);
    }

    fn directional_embedding(nonzero_dim: usize) -> Embedding {
        let mut values = vec![0.0f32; TEST_EMBEDDING_DIMENSIONS];
        values[nonzero_dim] = 1.0;
        Embedding::new(values, TEST_EMBEDDING_DIMENSIONS).unwrap()
    }
}
