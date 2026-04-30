use std::{error::Error, fmt};

use chrono::{DateTime, NaiveDate, Utc};
use sqlx::{Row, SqlitePool, sqlite::SqliteRow};

use super::{DailyReview, DailyReviewStatus};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DailyReviewRepositoryError {
    Storage(String),
    InvalidReviewDate(String),
    InvalidStatus(String),
}

impl fmt::Display for DailyReviewRepositoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Storage(message) => write!(f, "{message}"),
            Self::InvalidReviewDate(value) => {
                write!(f, "invalid daily review date stored in database: {value}")
            }
            Self::InvalidStatus(value) => {
                write!(f, "invalid daily review status stored in database: {value}")
            }
        }
    }
}

impl Error for DailyReviewRepositoryError {}

impl From<sqlx::Error> for DailyReviewRepositoryError {
    fn from(error: sqlx::Error) -> Self {
        Self::Storage(error.to_string())
    }
}

#[derive(Debug, Clone)]
pub struct DailyReviewRepository {
    pool: SqlitePool,
}

impl DailyReviewRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn find_by_user_and_date(
        &self,
        user_id: &str,
        review_date: NaiveDate,
    ) -> Result<Option<DailyReview>, DailyReviewRepositoryError> {
        let row = sqlx::query(
            r#"
            SELECT id, user_id, review_date, review_text, model, prompt_version, status,
                   error_message, created_at, updated_at
            FROM daily_reviews
            WHERE user_id = ? AND review_date = ?
            "#,
        )
        .bind(user_id)
        .bind(review_date.to_string())
        .fetch_optional(&self.pool)
        .await?;

        row.map(row_to_daily_review).transpose()
    }

    pub async fn upsert_completed(
        &self,
        user_id: &str,
        review_date: NaiveDate,
        review_text: &str,
        model: &str,
        prompt_version: &str,
    ) -> Result<DailyReview, DailyReviewRepositoryError> {
        sqlx::query(
            r#"
            INSERT INTO daily_reviews
                (user_id, review_date, review_text, model, prompt_version, status, error_message)
            VALUES (?, ?, ?, ?, ?, 'completed', NULL)
            ON CONFLICT(user_id, review_date) DO UPDATE SET
                review_text = excluded.review_text,
                model = excluded.model,
                prompt_version = excluded.prompt_version,
                status = 'completed',
                error_message = NULL,
                updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
            "#,
        )
        .bind(user_id)
        .bind(review_date.to_string())
        .bind(review_text)
        .bind(model)
        .bind(prompt_version)
        .execute(&self.pool)
        .await?;

        self.find_by_user_and_date(user_id, review_date)
            .await?
            .ok_or_else(|| {
                DailyReviewRepositoryError::Storage("daily review was not stored".into())
            })
    }

    pub async fn upsert_failed(
        &self,
        user_id: &str,
        review_date: NaiveDate,
        model: &str,
        prompt_version: &str,
        error_message: &str,
    ) -> Result<DailyReview, DailyReviewRepositoryError> {
        sqlx::query(
            r#"
            INSERT INTO daily_reviews
                (user_id, review_date, review_text, model, prompt_version, status, error_message)
            VALUES (?, ?, NULL, ?, ?, 'failed', ?)
            ON CONFLICT(user_id, review_date) DO UPDATE SET
                review_text = NULL,
                model = excluded.model,
                prompt_version = excluded.prompt_version,
                status = 'failed',
                error_message = excluded.error_message,
                updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
            WHERE daily_reviews.status = 'failed'
            "#,
        )
        .bind(user_id)
        .bind(review_date.to_string())
        .bind(model)
        .bind(prompt_version)
        .bind(error_message)
        .execute(&self.pool)
        .await?;

        self.find_by_user_and_date(user_id, review_date)
            .await?
            .ok_or_else(|| {
                DailyReviewRepositoryError::Storage("daily review was not stored".into())
            })
    }
}

fn row_to_daily_review(row: SqliteRow) -> Result<DailyReview, DailyReviewRepositoryError> {
    let review_date = row.get::<String, _>("review_date");
    let review_date = NaiveDate::parse_from_str(&review_date, "%Y-%m-%d")
        .map_err(|_| DailyReviewRepositoryError::InvalidReviewDate(review_date))?;

    let status = row.get::<String, _>("status");
    let status = match status.as_str() {
        "completed" => DailyReviewStatus::Completed,
        "failed" => DailyReviewStatus::Failed,
        _ => return Err(DailyReviewRepositoryError::InvalidStatus(status)),
    };

    Ok(DailyReview {
        id: row.get("id"),
        user_id: row.get("user_id"),
        review_date,
        review_text: row.get("review_text"),
        model: row.get("model"),
        prompt_version: row.get("prompt_version"),
        status,
        error_message: row.get("error_message"),
        created_at: row.get::<DateTime<Utc>, _>("created_at"),
        updated_at: row.get::<DateTime<Utc>, _>("updated_at"),
    })
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;
    use sqlx::SqlitePool;

    use super::*;
    use crate::database;

    async fn setup() -> DailyReviewRepository {
        database::register_sqlite_vec_extension();
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        DailyReviewRepository::new(pool)
    }

    fn date() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 4, 28).unwrap()
    }

    #[tokio::test]
    async fn stores_completed_review() {
        let repo = setup().await;

        let review = repo
            .upsert_completed("user-1", date(), "review text", "test-model", "v1")
            .await
            .unwrap();

        assert_eq!(review.user_id, "user-1");
        assert_eq!(review.review_date, date());
        assert_eq!(review.review_text, Some("review text".to_string()));
        assert_eq!(review.model, "test-model");
        assert_eq!(review.prompt_version, "v1");
        assert_eq!(review.status, DailyReviewStatus::Completed);
        assert_eq!(review.error_message, None);
        assert!(review.created_at <= review.updated_at);
    }

    #[tokio::test]
    async fn stores_failed_review() {
        let repo = setup().await;

        let review = repo
            .upsert_failed("user-1", date(), "test-model", "v1", "provider down")
            .await
            .unwrap();

        assert_eq!(review.review_text, None);
        assert_eq!(review.status, DailyReviewStatus::Failed);
        assert_eq!(review.error_message, Some("provider down".to_string()));
        assert_eq!(review.model, "test-model");
        assert_eq!(review.prompt_version, "v1");
    }

    #[tokio::test]
    async fn finds_review_by_user_and_date() {
        let repo = setup().await;
        repo.upsert_completed("user-1", date(), "review text", "test-model", "v1")
            .await
            .unwrap();

        let found = repo
            .find_by_user_and_date("user-1", date())
            .await
            .unwrap()
            .unwrap();

        assert_eq!(found.review_text, Some("review text".to_string()));
    }

    #[tokio::test]
    async fn completed_reviews_are_separate_by_user_and_date() {
        let repo = setup().await;
        let other_date = NaiveDate::from_ymd_opt(2026, 4, 29).unwrap();

        repo.upsert_completed("user-1", date(), "user one", "test-model", "v1")
            .await
            .unwrap();
        repo.upsert_completed("user-2", date(), "user two", "test-model", "v1")
            .await
            .unwrap();
        repo.upsert_completed("user-1", other_date, "other date", "test-model", "v1")
            .await
            .unwrap();

        assert_eq!(
            repo.find_by_user_and_date("user-1", date())
                .await
                .unwrap()
                .unwrap()
                .review_text,
            Some("user one".to_string())
        );
        assert_eq!(
            repo.find_by_user_and_date("user-2", date())
                .await
                .unwrap()
                .unwrap()
                .review_text,
            Some("user two".to_string())
        );
        assert_eq!(
            repo.find_by_user_and_date("user-1", other_date)
                .await
                .unwrap()
                .unwrap()
                .review_text,
            Some("other date".to_string())
        );
    }

    #[tokio::test]
    async fn upsert_completed_overwrites_existing_completed_row() {
        let repo = setup().await;

        let original = repo
            .upsert_completed("user-1", date(), "original", "test-model", "v1")
            .await
            .unwrap();
        let updated = repo
            .upsert_completed("user-1", date(), "new review", "new-model", "v2")
            .await
            .unwrap();

        assert_eq!(updated.id, original.id);
        assert_eq!(updated.created_at, original.created_at);
        assert_eq!(updated.review_text, Some("new review".to_string()));
        assert_eq!(updated.model, "new-model");
        assert_eq!(updated.prompt_version, "v2");
        assert_eq!(updated.status, DailyReviewStatus::Completed);
    }

    #[tokio::test]
    async fn upsert_failed_does_not_overwrite_completed_review() {
        let repo = setup().await;

        let original = repo
            .upsert_completed("user-1", date(), "original", "test-model", "v1")
            .await
            .unwrap();
        let after_failed = repo
            .upsert_failed("user-1", date(), "new-model", "v2", "new error")
            .await
            .unwrap();

        assert_eq!(after_failed, original);
    }

    #[tokio::test]
    async fn failed_review_can_be_updated_to_completed_without_replacing_row() {
        let repo = setup().await;

        let failed = repo
            .upsert_failed("user-1", date(), "test-model", "v1", "provider down")
            .await
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));

        let completed = repo
            .upsert_completed("user-1", date(), "review text", "test-model", "v1")
            .await
            .unwrap();

        assert_eq!(completed.id, failed.id);
        assert_eq!(completed.created_at, failed.created_at);
        assert!(completed.updated_at > failed.updated_at);
        assert_eq!(completed.status, DailyReviewStatus::Completed);
        assert_eq!(completed.review_text, Some("review text".to_string()));
        assert_eq!(completed.error_message, None);
    }

    #[tokio::test]
    async fn failed_review_can_be_updated_with_latest_failure() {
        let repo = setup().await;

        let first = repo
            .upsert_failed("user-1", date(), "test-model", "v1", "first error")
            .await
            .unwrap();
        let second = repo
            .upsert_failed("user-1", date(), "test-model-2", "v2", "second error")
            .await
            .unwrap();

        assert_eq!(second.id, first.id);
        assert_eq!(second.status, DailyReviewStatus::Failed);
        assert_eq!(second.model, "test-model-2");
        assert_eq!(second.prompt_version, "v2");
        assert_eq!(second.error_message, Some("second error".to_string()));
    }
}
