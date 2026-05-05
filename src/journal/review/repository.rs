use std::{error::Error, fmt};

use chrono::{DateTime, NaiveDate, Utc};
use sqlx::{Row, SqlitePool, sqlite::SqliteRow};

use crate::messages::SINGLE_USER_ID;

use super::{DailyReview, DailyReviewStatus, SignalGenerationStatus};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DailyReviewRepositoryError {
    Storage(String),
    InvalidReviewDate(String),
    InvalidStatus(String),
    InvalidSignalStatus(String),
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
            Self::InvalidSignalStatus(value) => {
                write!(
                    f,
                    "invalid signal generation status stored in database: {value}"
                )
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
        _user_id: &str,
        review_date: NaiveDate,
    ) -> Result<Option<DailyReview>, DailyReviewRepositoryError> {
        let row = sqlx::query(
            r#"
            SELECT id, review_date, review_text, model, prompt_version, status,
                   error_message, delivered_at, delivery_error,
                   signals_status, signals_error, signals_model, signals_prompt_version, signals_updated_at,
                   created_at, updated_at
            FROM daily_reviews
            WHERE review_date = ?
            "#,
        )
        .bind(review_date.to_string())
        .fetch_optional(&self.pool)
        .await?;

        row.map(row_to_daily_review).transpose()
    }

    pub async fn find_by_id(
        &self,
        id: i64,
    ) -> Result<Option<DailyReview>, DailyReviewRepositoryError> {
        let row = sqlx::query(
            r#"
            SELECT id, review_date, review_text, model, prompt_version, status,
                   error_message, delivered_at, delivery_error,
                   signals_status, signals_error, signals_model, signals_prompt_version, signals_updated_at,
                   created_at, updated_at
            FROM daily_reviews
            WHERE id = ?
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        row.map(row_to_daily_review).transpose()
    }

    pub async fn fetch_by_ids(
        &self,
        _user_id: &str,
        ids: &[i64],
    ) -> Result<Vec<(i64, DailyReview)>, DailyReviewRepositoryError> {
        if ids.is_empty() {
            return Ok(vec![]);
        }

        let query = format!(
            r#"
            SELECT id, review_date, review_text, model, prompt_version, status,
                   error_message, delivered_at, delivery_error,
                   signals_status, signals_error, signals_model, signals_prompt_version, signals_updated_at,
                   created_at, updated_at
            FROM daily_reviews
            WHERE id IN ({})
            "#,
            vec!["?"; ids.len()].join(", ")
        );

        let mut q = sqlx::query(&query);
        for id in ids {
            q = q.bind(id);
        }

        let rows = q.fetch_all(&self.pool).await?;

        let mut results = Vec::new();
        for row in rows {
            let review = row_to_daily_review(row)?;
            results.push((review.id, review));
        }

        Ok(results)
    }

    pub async fn fetch_completed_in_range(
        &self,
        _user_id: &str,
        start_date: NaiveDate,
        end_date_exclusive: NaiveDate,
    ) -> Result<Vec<DailyReview>, DailyReviewRepositoryError> {
        let rows = sqlx::query(
            r#"
            SELECT id, review_date, review_text, model, prompt_version, status,
                   error_message, delivered_at, delivery_error,
                   signals_status, signals_error, signals_model, signals_prompt_version, signals_updated_at,
                   created_at, updated_at
            FROM daily_reviews
            WHERE review_date >= ?
              AND review_date < ?
              AND status = 'completed'
              AND review_text IS NOT NULL
              AND TRIM(review_text) != ''
            ORDER BY review_date ASC
            "#,
        )
        .bind(start_date.to_string())
        .bind(end_date_exclusive.to_string())
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(row_to_daily_review).collect()
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
                (review_date, review_text, model, prompt_version, status, error_message)
            VALUES (?, ?, ?, ?, 'completed', NULL)
            ON CONFLICT(review_date) DO UPDATE SET
                review_text = excluded.review_text,
                model = excluded.model,
                prompt_version = excluded.prompt_version,
                status = 'completed',
                delivery_error = NULL,
                error_message = NULL,
                signals_status = NULL,
                signals_error = NULL,
                signals_model = NULL,
                signals_prompt_version = NULL,
                signals_updated_at = NULL,
                updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
            "#,
        )
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
                (review_date, review_text, model, prompt_version, status, error_message)
            VALUES (?, NULL, ?, ?, 'failed', ?)
            ON CONFLICT(review_date) DO UPDATE SET
                review_text = NULL,
                model = excluded.model,
                prompt_version = excluded.prompt_version,
                status = 'failed',
                error_message = excluded.error_message,
                delivery_error = NULL,
                signals_status = NULL,
                signals_error = NULL,
                signals_model = NULL,
                signals_prompt_version = NULL,
                signals_updated_at = NULL,
                updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
            WHERE daily_reviews.status = 'failed'
            "#,
        )
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

    pub async fn mark_delivered(
        &self,
        _user_id: &str,
        review_date: NaiveDate,
    ) -> Result<(), DailyReviewRepositoryError> {
        sqlx::query(
            r#"
            UPDATE daily_reviews
            SET delivered_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                delivery_error = NULL,
                updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
            WHERE review_date = ?
              AND status = 'completed'
            "#,
        )
        .bind(review_date.to_string())
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn mark_delivery_failed(
        &self,
        _user_id: &str,
        review_date: NaiveDate,
        error_message: &str,
    ) -> Result<(), DailyReviewRepositoryError> {
        sqlx::query(
            r#"
            UPDATE daily_reviews
            SET delivery_error = ?,
                updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
            WHERE review_date = ?
            "#,
        )
        .bind(error_message)
        .bind(review_date.to_string())
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn mark_signals_pending(
        &self,
        daily_review_id: i64,
        model: &str,
        prompt_version: &str,
    ) -> Result<(), DailyReviewRepositoryError> {
        sqlx::query(
            r#"
            UPDATE daily_reviews
            SET signals_status = 'pending',
                signals_model = ?,
                signals_prompt_version = ?,
                signals_error = NULL,
                signals_updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
            WHERE id = ?
            "#,
        )
        .bind(model)
        .bind(prompt_version)
        .bind(daily_review_id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn mark_signals_completed(
        &self,
        daily_review_id: i64,
    ) -> Result<(), DailyReviewRepositoryError> {
        sqlx::query(
            r#"
            UPDATE daily_reviews
            SET signals_status = 'completed',
                signals_error = NULL,
                signals_updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
            WHERE id = ?
            "#,
        )
        .bind(daily_review_id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn mark_signals_failed(
        &self,
        daily_review_id: i64,
        error_message: &str,
    ) -> Result<(), DailyReviewRepositoryError> {
        sqlx::query(
            r#"
            UPDATE daily_reviews
            SET signals_status = 'failed',
                signals_error = ?,
                signals_updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
            WHERE id = ?
            "#,
        )
        .bind(error_message)
        .bind(daily_review_id)
        .execute(&self.pool)
        .await?;

        Ok(())
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

    let signals_status = row
        .get::<Option<String>, _>("signals_status")
        .map(|s| match s.as_str() {
            "pending" => Ok(SignalGenerationStatus::Pending),
            "completed" => Ok(SignalGenerationStatus::Completed),
            "failed" => Ok(SignalGenerationStatus::Failed),
            other => Err(DailyReviewRepositoryError::InvalidSignalStatus(
                other.to_string(),
            )),
        })
        .transpose()?;

    Ok(DailyReview {
        id: row.get("id"),
        user_id: SINGLE_USER_ID.to_string(),
        review_date,
        review_text: row.get("review_text"),
        model: row.get("model"),
        prompt_version: row.get("prompt_version"),
        status,
        error_message: row.get("error_message"),
        delivered_at: row.get("delivered_at"),
        delivery_error: row.get("delivery_error"),
        signals_status,
        signals_error: row.get("signals_error"),
        signals_model: row.get("signals_model"),
        signals_prompt_version: row.get("signals_prompt_version"),
        signals_updated_at: row.get("signals_updated_at"),
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

        assert_eq!(review.user_id, SINGLE_USER_ID);
        assert_eq!(review.review_date, date());
        assert_eq!(review.review_text, Some("review text".to_string()));
        assert_eq!(review.model, "test-model");
        assert_eq!(review.prompt_version, "v1");
        assert_eq!(review.status, DailyReviewStatus::Completed);
        assert_eq!(review.error_message, None);
        assert_eq!(review.delivered_at, None);
        assert_eq!(review.delivery_error, None);
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
        assert_eq!(review.delivered_at, None);
        assert_eq!(review.delivery_error, None);
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
    async fn completed_reviews_are_unique_by_date_in_single_user_journal() {
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

    #[tokio::test]
    async fn mark_delivered_records_delivery_time_and_clears_delivery_error() {
        let repo = setup().await;
        repo.upsert_completed("user-1", date(), "review text", "test-model", "v1")
            .await
            .unwrap();
        repo.mark_delivery_failed("user-1", date(), "telegram failed")
            .await
            .unwrap();

        repo.mark_delivered("user-1", date()).await.unwrap();

        let review = repo
            .find_by_user_and_date("user-1", date())
            .await
            .unwrap()
            .unwrap();
        assert!(review.delivered_at.is_some());
        assert_eq!(review.delivery_error, None);
    }

    #[tokio::test]
    async fn mark_delivery_failed_records_latest_delivery_error() {
        let repo = setup().await;
        repo.upsert_completed("user-1", date(), "review text", "test-model", "v1")
            .await
            .unwrap();

        repo.mark_delivery_failed("user-1", date(), "first error")
            .await
            .unwrap();
        repo.mark_delivery_failed("user-1", date(), "second error")
            .await
            .unwrap();

        let review = repo
            .find_by_user_and_date("user-1", date())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(review.delivered_at, None);
        assert_eq!(review.delivery_error, Some("second error".to_string()));
    }

    #[tokio::test]
    async fn fetch_completed_in_range_returns_only_completed_rows_in_range_ordered_ascending() {
        let repo = setup().await;
        let monday = NaiveDate::from_ymd_opt(2026, 4, 27).unwrap();
        let tuesday = NaiveDate::from_ymd_opt(2026, 4, 28).unwrap();
        let next_monday = NaiveDate::from_ymd_opt(2026, 5, 4).unwrap();
        let prev_sunday = NaiveDate::from_ymd_opt(2026, 4, 26).unwrap();

        repo.upsert_completed("user-1", monday, "monday", "m", "v1")
            .await
            .unwrap();
        repo.upsert_completed("user-1", tuesday, "tuesday", "m", "v1")
            .await
            .unwrap();
        repo.upsert_completed("user-1", prev_sunday, "previous week", "m", "v1")
            .await
            .unwrap();
        repo.upsert_completed("user-1", next_monday, "next week", "m", "v1")
            .await
            .unwrap();
        repo.upsert_failed(
            "user-1",
            NaiveDate::from_ymd_opt(2026, 4, 29).unwrap(),
            "m",
            "v1",
            "boom",
        )
        .await
        .unwrap();

        let rows = repo
            .fetch_completed_in_range(
                "user-1",
                monday,
                NaiveDate::from_ymd_opt(2026, 5, 4).unwrap(),
            )
            .await
            .unwrap();

        let dates: Vec<_> = rows.iter().map(|r| r.review_date).collect();
        assert_eq!(dates, vec![monday, tuesday]);
    }

    #[tokio::test]
    async fn fetch_completed_in_range_excludes_blank_review_text() {
        let repo = setup().await;
        let date_a = NaiveDate::from_ymd_opt(2026, 4, 27).unwrap();
        let date_b = NaiveDate::from_ymd_opt(2026, 4, 28).unwrap();
        repo.upsert_completed("user-1", date_a, "real", "m", "v1")
            .await
            .unwrap();
        repo.upsert_completed("user-1", date_b, "   ", "m", "v1")
            .await
            .unwrap();

        let rows = repo
            .fetch_completed_in_range(
                "user-1",
                date_a,
                NaiveDate::from_ymd_opt(2026, 5, 4).unwrap(),
            )
            .await
            .unwrap();

        let dates: Vec<_> = rows.iter().map(|r| r.review_date).collect();
        assert_eq!(dates, vec![date_a]);
    }

    #[tokio::test]
    async fn fetch_completed_in_range_ignores_caller_user_id() {
        let repo = setup().await;
        let target = NaiveDate::from_ymd_opt(2026, 4, 27).unwrap();
        repo.upsert_completed("user-1", target, "user one", "m", "v1")
            .await
            .unwrap();
        repo.upsert_completed("user-2", target, "user two", "m", "v1")
            .await
            .unwrap();

        let rows = repo
            .fetch_completed_in_range(
                "user-1",
                target,
                NaiveDate::from_ymd_opt(2026, 5, 4).unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].user_id, SINGLE_USER_ID);
        assert_eq!(rows[0].review_text, Some("user two".to_string()));
    }
}
