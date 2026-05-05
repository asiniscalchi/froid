use async_trait::async_trait;
use sqlx::{Row, SqlitePool};

use crate::journal::embedding::{
    Embedding, EmbeddingCandidate, EmbeddingIndex, EmbeddingRepositoryError, EmbeddingSearchResult,
};

#[derive(Debug, Clone)]
pub struct SqliteDailyReviewEmbeddingRepository {
    pool: SqlitePool,
}

impl SqliteDailyReviewEmbeddingRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    async fn record_embedding_failure(
        &self,
        daily_review_id: i64,
        embedding_model: &str,
        error_message: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"
            INSERT OR IGNORE INTO daily_review_embedding_metadata
                (daily_review_id, embedding_model, embedding_dim, status, error_message)
            VALUES (?, ?, 0, 'failed', ?)
            "#,
        )
        .bind(daily_review_id)
        .bind(embedding_model)
        .bind(error_message)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn delete_failed_embedding(
        &self,
        daily_review_id: i64,
        embedding_model: &str,
    ) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            r#"
            DELETE FROM daily_review_embedding_metadata
            WHERE daily_review_id = ? AND embedding_model = ? AND status = 'failed'
            "#,
        )
        .bind(daily_review_id)
        .bind(embedding_model)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected() > 0)
    }

    async fn store_embedding(
        &self,
        daily_review_id: i64,
        embedding_model: &str,
        embedding_dim: usize,
        embedding: &Embedding,
    ) -> Result<bool, sqlx::Error> {
        let mut tx = self.pool.begin().await?;

        let result = sqlx::query(
            r#"
            INSERT OR IGNORE INTO daily_review_embedding_metadata
                (daily_review_id, embedding_model, embedding_dim)
            VALUES (?, ?, ?)
            "#,
        )
        .bind(daily_review_id)
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
            INSERT INTO daily_review_embedding_vec(rowid, embedding)
            VALUES (?, ?)
            "#,
        )
        .bind(result.last_insert_rowid())
        .bind(embedding.to_blob())
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;

        Ok(true)
    }

    async fn find_entries_missing_or_failed_embedding(
        &self,
        embedding_model: &str,
        limit: u32,
    ) -> Result<Vec<EmbeddingCandidate<i64>>, sqlx::Error> {
        let rows = sqlx::query(
            r#"
            SELECT daily_reviews.id, daily_reviews.review_text
            FROM daily_reviews
            LEFT JOIN daily_review_embedding_metadata
              ON daily_review_embedding_metadata.daily_review_id = daily_reviews.id
             AND daily_review_embedding_metadata.embedding_model = ?
            WHERE daily_reviews.review_text IS NOT NULL
              AND (daily_review_embedding_metadata.id IS NULL
                   OR daily_review_embedding_metadata.status = 'failed')
            ORDER BY daily_reviews.review_date ASC, daily_reviews.id ASC
            LIMIT ?
            "#,
        )
        .bind(embedding_model)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| EmbeddingCandidate {
                id: row.get("id"),
                raw_text: row.get("review_text"),
            })
            .collect())
    }

    async fn count_entries_missing_or_failed_embedding(
        &self,
        embedding_model: &str,
    ) -> Result<u32, sqlx::Error> {
        let count: i32 = sqlx::query_scalar(
            r#"
            SELECT COUNT(*)
            FROM daily_reviews
            LEFT JOIN daily_review_embedding_metadata
              ON daily_review_embedding_metadata.daily_review_id = daily_reviews.id
             AND daily_review_embedding_metadata.embedding_model = ?
            WHERE daily_reviews.review_text IS NOT NULL
              AND (daily_review_embedding_metadata.id IS NULL
                   OR daily_review_embedding_metadata.status = 'failed')
            "#,
        )
        .bind(embedding_model)
        .fetch_one(&self.pool)
        .await?;

        Ok(count as u32)
    }

    async fn search_for_user(
        &self,
        _user_id: &str,
        embedding: &Embedding,
        embedding_model: &str,
        limit: usize,
    ) -> Result<Vec<EmbeddingSearchResult<i64>>, sqlx::Error> {
        let rows = sqlx::query(
            r#"
            SELECT
                m.daily_review_id,
                vec_distance_cosine(v.embedding, ?) AS distance
            FROM daily_review_embedding_metadata m
            JOIN daily_review_embedding_vec v ON v.rowid = m.id
            JOIN daily_reviews r ON r.id = m.daily_review_id
            WHERE m.embedding_model = ?
            ORDER BY distance ASC
            LIMIT ?
            "#,
        )
        .bind(embedding.to_blob())
        .bind(embedding_model)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| EmbeddingSearchResult {
                id: row.get("daily_review_id"),
                distance: row.get("distance"),
            })
            .collect())
    }
}

#[async_trait]
impl EmbeddingIndex<i64> for SqliteDailyReviewEmbeddingRepository {
    async fn store_embedding(
        &self,
        id: i64,
        embedding_model: &str,
        embedding_dim: usize,
        embedding: &Embedding,
    ) -> Result<bool, EmbeddingRepositoryError> {
        self.store_embedding(id, embedding_model, embedding_dim, embedding)
            .await
            .map_err(Into::into)
    }

    async fn record_embedding_failure(
        &self,
        id: i64,
        embedding_model: &str,
        error_message: &str,
    ) -> Result<(), EmbeddingRepositoryError> {
        self.record_embedding_failure(id, embedding_model, error_message)
            .await
            .map_err(Into::into)
    }

    async fn delete_failed_embedding(
        &self,
        id: i64,
        embedding_model: &str,
    ) -> Result<bool, EmbeddingRepositoryError> {
        self.delete_failed_embedding(id, embedding_model)
            .await
            .map_err(Into::into)
    }

    async fn find_entries_missing_or_failed_embedding(
        &self,
        embedding_model: &str,
        limit: u32,
    ) -> Result<Vec<EmbeddingCandidate<i64>>, EmbeddingRepositoryError> {
        self.find_entries_missing_or_failed_embedding(embedding_model, limit)
            .await
            .map_err(Into::into)
    }

    async fn count_entries_missing_or_failed_embedding(
        &self,
        embedding_model: &str,
    ) -> Result<u32, EmbeddingRepositoryError> {
        self.count_entries_missing_or_failed_embedding(embedding_model)
            .await
            .map_err(Into::into)
    }

    async fn search_for_user(
        &self,
        user_id: &str,
        embedding: &Embedding,
        embedding_model: &str,
        limit: usize,
    ) -> Result<Vec<EmbeddingSearchResult<i64>>, EmbeddingRepositoryError> {
        self.search_for_user(user_id, embedding, embedding_model, limit)
            .await
            .map_err(Into::into)
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
            embedding::SUPPORTED_EMBEDDING_DIMENSIONS, review::repository::DailyReviewRepository,
        },
    };

    async fn setup() -> (DailyReviewRepository, SqliteDailyReviewEmbeddingRepository) {
        database::register_sqlite_vec_extension();
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();

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
            .upsert_completed("user-1", date, "review text", "model", "v1")
            .await
            .unwrap();

        let created = embedding_repo
            .store_embedding(
                review.id,
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
            .upsert_completed("user-1", date, "review text", "model", "v1")
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
            .upsert_completed("user-1", date, "review text", "model", "v1")
            .await
            .unwrap();

        embedding_repo
            .record_embedding_failure(review.id, TEST_EMBEDDING_MODEL, "provider error")
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
            .upsert_completed("user-1", date, "review text", "model", "v1")
            .await
            .unwrap();

        embedding_repo
            .record_embedding_failure(review.id, TEST_EMBEDDING_MODEL, "provider error")
            .await
            .unwrap();

        let deleted = embedding_repo
            .delete_failed_embedding(review.id, TEST_EMBEDDING_MODEL)
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
            .upsert_completed("user-1", date1, "review 1", "model", "v1")
            .await
            .unwrap();
        let review2 = review_repo
            .upsert_completed("user-1", date2, "review 2", "model", "v1")
            .await
            .unwrap();
        let review3 = review_repo
            .upsert_completed("user-1", date3, "review 3", "model", "v1")
            .await
            .unwrap();

        // Directional embeddings: 1 is closest to query 1, then 2, then 3 is furthest.
        embedding_repo
            .store_embedding(
                review1.id,
                TEST_EMBEDDING_MODEL,
                TEST_EMBEDDING_DIMENSIONS,
                &directional_embedding(1),
            )
            .await
            .unwrap();
        embedding_repo
            .store_embedding(
                review2.id,
                TEST_EMBEDDING_MODEL,
                TEST_EMBEDDING_DIMENSIONS,
                &directional_embedding(2),
            )
            .await
            .unwrap();
        embedding_repo
            .store_embedding(
                review3.id,
                TEST_EMBEDDING_MODEL,
                TEST_EMBEDDING_DIMENSIONS,
                &directional_embedding(3),
            )
            .await
            .unwrap();

        let query = directional_embedding(1);
        let results = embedding_repo
            .search_for_user("user-1", &query, TEST_EMBEDDING_MODEL, 10)
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
