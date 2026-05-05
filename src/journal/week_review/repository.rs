use std::{error::Error, fmt};

use chrono::{DateTime, NaiveDate, Utc};
use sqlx::{Row, SqlitePool, sqlite::SqliteRow};

use crate::messages::SINGLE_USER_ID;

use super::{WeeklyReview, WeeklyReviewStatus};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WeeklyReviewRepositoryError {
    Storage(String),
    InvalidWeekStartDate(String),
    InvalidStatus(String),
}

impl fmt::Display for WeeklyReviewRepositoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Storage(message) => write!(f, "{message}"),
            Self::InvalidWeekStartDate(value) => {
                write!(
                    f,
                    "invalid weekly review week start stored in database: {value}"
                )
            }
            Self::InvalidStatus(value) => {
                write!(
                    f,
                    "invalid weekly review status stored in database: {value}"
                )
            }
        }
    }
}

impl Error for WeeklyReviewRepositoryError {}

impl From<sqlx::Error> for WeeklyReviewRepositoryError {
    fn from(error: sqlx::Error) -> Self {
        Self::Storage(error.to_string())
    }
}

#[derive(Debug, Clone)]
pub struct WeeklyReviewRepository {
    pool: SqlitePool,
}

impl WeeklyReviewRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn find_by_user_and_week(
        &self,
        _user_id: &str,
        week_start_date: NaiveDate,
    ) -> Result<Option<WeeklyReview>, WeeklyReviewRepositoryError> {
        let row = sqlx::query(
            r#"
            SELECT id, week_start_date, review_text, model, prompt_version, status,
                   error_message, delivered_at, delivery_error, inputs_snapshot,
                   created_at, updated_at
            FROM weekly_reviews
            WHERE week_start_date = ?
            "#,
        )
        .bind(week_start_date.to_string())
        .fetch_optional(&self.pool)
        .await?;

        row.map(row_to_weekly_review).transpose()
    }

    pub async fn fetch_completed_in_range(
        &self,
        _user_id: &str,
        start_date: NaiveDate,
        end_date_exclusive: NaiveDate,
    ) -> Result<Vec<WeeklyReview>, WeeklyReviewRepositoryError> {
        let rows = sqlx::query(
            r#"
            SELECT id, week_start_date, review_text, model, prompt_version, status,
                   error_message, delivered_at, delivery_error, inputs_snapshot,
                   created_at, updated_at
            FROM weekly_reviews
            WHERE week_start_date >= ?
              AND week_start_date < ?
              AND status = 'completed'
              AND review_text IS NOT NULL
              AND TRIM(review_text) != ''
            ORDER BY week_start_date ASC
            "#,
        )
        .bind(start_date.to_string())
        .bind(end_date_exclusive.to_string())
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(row_to_weekly_review).collect()
    }

    pub async fn upsert_completed(
        &self,
        user_id: &str,
        week_start_date: NaiveDate,
        review_text: &str,
        model: &str,
        prompt_version: &str,
        inputs_snapshot: &str,
    ) -> Result<WeeklyReview, WeeklyReviewRepositoryError> {
        sqlx::query(
            r#"
            INSERT INTO weekly_reviews
                (week_start_date, review_text, model, prompt_version, status, error_message, inputs_snapshot)
            VALUES (?, ?, ?, ?, 'completed', NULL, ?)
            ON CONFLICT(week_start_date) DO UPDATE SET
                review_text = excluded.review_text,
                model = excluded.model,
                prompt_version = excluded.prompt_version,
                status = 'completed',
                error_message = NULL,
                delivery_error = NULL,
                inputs_snapshot = excluded.inputs_snapshot,
                updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
            "#,
        )
        .bind(week_start_date.to_string())
        .bind(review_text)
        .bind(model)
        .bind(prompt_version)
        .bind(inputs_snapshot)
        .execute(&self.pool)
        .await?;

        self.find_by_user_and_week(user_id, week_start_date)
            .await?
            .ok_or_else(|| {
                WeeklyReviewRepositoryError::Storage("weekly review was not stored".into())
            })
    }

    pub async fn upsert_failed(
        &self,
        user_id: &str,
        week_start_date: NaiveDate,
        model: &str,
        prompt_version: &str,
        error_message: &str,
    ) -> Result<WeeklyReview, WeeklyReviewRepositoryError> {
        sqlx::query(
            r#"
            INSERT INTO weekly_reviews
                (week_start_date, review_text, model, prompt_version, status, error_message)
            VALUES (?, NULL, ?, ?, 'failed', ?)
            ON CONFLICT(week_start_date) DO UPDATE SET
                review_text = NULL,
                model = excluded.model,
                prompt_version = excluded.prompt_version,
                status = 'failed',
                error_message = excluded.error_message,
                delivery_error = NULL,
                updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
            WHERE weekly_reviews.status = 'failed'
            "#,
        )
        .bind(week_start_date.to_string())
        .bind(model)
        .bind(prompt_version)
        .bind(error_message)
        .execute(&self.pool)
        .await?;

        self.find_by_user_and_week(user_id, week_start_date)
            .await?
            .ok_or_else(|| {
                WeeklyReviewRepositoryError::Storage("weekly review was not stored".into())
            })
    }

    pub async fn mark_delivered(
        &self,
        _user_id: &str,
        week_start_date: NaiveDate,
    ) -> Result<(), WeeklyReviewRepositoryError> {
        sqlx::query(
            r#"
            UPDATE weekly_reviews
            SET delivered_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                delivery_error = NULL,
                updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
            WHERE week_start_date = ?
              AND status = 'completed'
            "#,
        )
        .bind(week_start_date.to_string())
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn mark_delivery_failed(
        &self,
        _user_id: &str,
        week_start_date: NaiveDate,
        error_message: &str,
    ) -> Result<(), WeeklyReviewRepositoryError> {
        sqlx::query(
            r#"
            UPDATE weekly_reviews
            SET delivery_error = ?,
                updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
            WHERE week_start_date = ?
            "#,
        )
        .bind(error_message)
        .bind(week_start_date.to_string())
        .execute(&self.pool)
        .await?;

        Ok(())
    }
}

fn row_to_weekly_review(row: SqliteRow) -> Result<WeeklyReview, WeeklyReviewRepositoryError> {
    let week_start_date = row.get::<String, _>("week_start_date");
    let week_start_date = NaiveDate::parse_from_str(&week_start_date, "%Y-%m-%d")
        .map_err(|_| WeeklyReviewRepositoryError::InvalidWeekStartDate(week_start_date))?;

    let status = row.get::<String, _>("status");
    let status = match status.as_str() {
        "completed" => WeeklyReviewStatus::Completed,
        "failed" => WeeklyReviewStatus::Failed,
        _ => return Err(WeeklyReviewRepositoryError::InvalidStatus(status)),
    };

    Ok(WeeklyReview {
        id: row.get("id"),
        user_id: SINGLE_USER_ID.to_string(),
        week_start_date,
        review_text: row.get("review_text"),
        model: row.get("model"),
        prompt_version: row.get("prompt_version"),
        status,
        error_message: row.get("error_message"),
        delivered_at: row.get("delivered_at"),
        delivery_error: row.get("delivery_error"),
        inputs_snapshot: row.get("inputs_snapshot"),
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

    async fn setup() -> WeeklyReviewRepository {
        database::register_sqlite_vec_extension();
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        WeeklyReviewRepository::new(pool)
    }

    fn week_start() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 4, 27).unwrap()
    }

    #[tokio::test]
    async fn stores_completed_review() {
        let repo = setup().await;

        let review = repo
            .upsert_completed(
                "user-1",
                week_start(),
                "review text",
                "test-model",
                "v1",
                "{}",
            )
            .await
            .unwrap();

        assert_eq!(review.user_id, SINGLE_USER_ID);
        assert_eq!(review.week_start_date, week_start());
        assert_eq!(review.review_text, Some("review text".to_string()));
        assert_eq!(review.model, "test-model");
        assert_eq!(review.prompt_version, "v1");
        assert_eq!(review.status, WeeklyReviewStatus::Completed);
        assert_eq!(review.error_message, None);
        assert_eq!(review.delivered_at, None);
        assert_eq!(review.delivery_error, None);
        assert!(review.created_at <= review.updated_at);
    }

    #[tokio::test]
    async fn stores_failed_review() {
        let repo = setup().await;

        let review = repo
            .upsert_failed("user-1", week_start(), "test-model", "v1", "provider down")
            .await
            .unwrap();

        assert_eq!(review.review_text, None);
        assert_eq!(review.status, WeeklyReviewStatus::Failed);
        assert_eq!(review.error_message, Some("provider down".to_string()));
        assert_eq!(review.delivered_at, None);
        assert_eq!(review.delivery_error, None);
        assert_eq!(review.model, "test-model");
        assert_eq!(review.prompt_version, "v1");
    }

    #[tokio::test]
    async fn finds_review_by_user_and_week() {
        let repo = setup().await;
        repo.upsert_completed(
            "user-1",
            week_start(),
            "review text",
            "test-model",
            "v1",
            "{}",
        )
        .await
        .unwrap();

        let found = repo
            .find_by_user_and_week("user-1", week_start())
            .await
            .unwrap()
            .unwrap();

        assert_eq!(found.review_text, Some("review text".to_string()));
    }

    #[tokio::test]
    async fn completed_reviews_are_unique_by_week_in_single_user_journal() {
        let repo = setup().await;
        let other_week = NaiveDate::from_ymd_opt(2026, 5, 4).unwrap();

        repo.upsert_completed("user-1", week_start(), "user one", "test-model", "v1", "{}")
            .await
            .unwrap();
        repo.upsert_completed("user-2", week_start(), "user two", "test-model", "v1", "{}")
            .await
            .unwrap();
        repo.upsert_completed("user-1", other_week, "other week", "test-model", "v1", "{}")
            .await
            .unwrap();

        assert_eq!(
            repo.find_by_user_and_week("user-2", week_start())
                .await
                .unwrap()
                .unwrap()
                .review_text,
            Some("user two".to_string())
        );
        assert_eq!(
            repo.find_by_user_and_week("user-1", other_week)
                .await
                .unwrap()
                .unwrap()
                .review_text,
            Some("other week".to_string())
        );
    }

    #[tokio::test]
    async fn upsert_completed_overwrites_existing_completed_row() {
        let repo = setup().await;

        let original = repo
            .upsert_completed("user-1", week_start(), "original", "test-model", "v1", "{}")
            .await
            .unwrap();
        let updated = repo
            .upsert_completed(
                "user-1",
                week_start(),
                "new review",
                "new-model",
                "v2",
                "{}",
            )
            .await
            .unwrap();

        assert_eq!(updated.id, original.id);
        assert_eq!(updated.created_at, original.created_at);
        assert_eq!(updated.review_text, Some("new review".to_string()));
        assert_eq!(updated.model, "new-model");
        assert_eq!(updated.prompt_version, "v2");
        assert_eq!(updated.status, WeeklyReviewStatus::Completed);
    }

    #[tokio::test]
    async fn upsert_failed_does_not_overwrite_completed_review() {
        let repo = setup().await;

        let original = repo
            .upsert_completed("user-1", week_start(), "original", "test-model", "v1", "{}")
            .await
            .unwrap();
        let after_failed = repo
            .upsert_failed("user-1", week_start(), "new-model", "v2", "new error")
            .await
            .unwrap();

        assert_eq!(after_failed, original);
    }

    #[tokio::test]
    async fn failed_review_can_be_updated_to_completed_without_replacing_row() {
        let repo = setup().await;

        let failed = repo
            .upsert_failed("user-1", week_start(), "test-model", "v1", "provider down")
            .await
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));

        let completed = repo
            .upsert_completed(
                "user-1",
                week_start(),
                "review text",
                "test-model",
                "v1",
                "{}",
            )
            .await
            .unwrap();

        assert_eq!(completed.id, failed.id);
        assert_eq!(completed.created_at, failed.created_at);
        assert!(completed.updated_at > failed.updated_at);
        assert_eq!(completed.status, WeeklyReviewStatus::Completed);
        assert_eq!(completed.review_text, Some("review text".to_string()));
        assert_eq!(completed.error_message, None);
    }

    #[tokio::test]
    async fn failed_review_can_be_updated_with_latest_failure() {
        let repo = setup().await;

        let first = repo
            .upsert_failed("user-1", week_start(), "test-model", "v1", "first error")
            .await
            .unwrap();
        let second = repo
            .upsert_failed("user-1", week_start(), "test-model-2", "v2", "second error")
            .await
            .unwrap();

        assert_eq!(second.id, first.id);
        assert_eq!(second.status, WeeklyReviewStatus::Failed);
        assert_eq!(second.model, "test-model-2");
        assert_eq!(second.prompt_version, "v2");
        assert_eq!(second.error_message, Some("second error".to_string()));
    }

    #[tokio::test]
    async fn mark_delivered_records_delivery_time_and_clears_delivery_error() {
        let repo = setup().await;
        repo.upsert_completed(
            "user-1",
            week_start(),
            "review text",
            "test-model",
            "v1",
            "{}",
        )
        .await
        .unwrap();
        repo.mark_delivery_failed("user-1", week_start(), "telegram failed")
            .await
            .unwrap();

        repo.mark_delivered("user-1", week_start()).await.unwrap();

        let review = repo
            .find_by_user_and_week("user-1", week_start())
            .await
            .unwrap()
            .unwrap();
        assert!(review.delivered_at.is_some());
        assert_eq!(review.delivery_error, None);
    }

    #[tokio::test]
    async fn mark_delivery_failed_records_latest_delivery_error() {
        let repo = setup().await;
        repo.upsert_completed(
            "user-1",
            week_start(),
            "review text",
            "test-model",
            "v1",
            "{}",
        )
        .await
        .unwrap();

        repo.mark_delivery_failed("user-1", week_start(), "first error")
            .await
            .unwrap();
        repo.mark_delivery_failed("user-1", week_start(), "second error")
            .await
            .unwrap();

        let review = repo
            .find_by_user_and_week("user-1", week_start())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(review.delivered_at, None);
        assert_eq!(review.delivery_error, Some("second error".to_string()));
    }

    #[tokio::test]
    async fn check_constraint_rejects_completed_with_null_review_text() {
        let repo = setup().await;
        let result = sqlx::query(
            r#"
            INSERT INTO weekly_reviews
                (user_id, week_start_date, review_text, model, prompt_version, status, error_message)
            VALUES ('user-1', '2026-04-27', NULL, 'm', 'v1', 'completed', NULL)
            "#,
        )
        .execute(&repo.pool)
        .await;

        assert!(result.is_err(), "expected CHECK constraint to reject row");
    }

    #[tokio::test]
    async fn check_constraint_rejects_failed_with_null_error_message() {
        let repo = setup().await;
        let result = sqlx::query(
            r#"
            INSERT INTO weekly_reviews
                (user_id, week_start_date, review_text, model, prompt_version, status, error_message)
            VALUES ('user-1', '2026-04-27', NULL, 'm', 'v1', 'failed', NULL)
            "#,
        )
        .execute(&repo.pool)
        .await;

        assert!(result.is_err(), "expected CHECK constraint to reject row");
    }

    #[tokio::test]
    async fn unique_constraint_enforced_on_user_and_week() {
        let repo = setup().await;
        repo.upsert_completed("user-1", week_start(), "first", "m", "v1", "{}")
            .await
            .unwrap();

        let result = sqlx::query(
            r#"
            INSERT INTO weekly_reviews
                (user_id, week_start_date, review_text, model, prompt_version, status, error_message)
            VALUES ('user-1', '2026-04-27', 'second', 'm', 'v1', 'completed', NULL)
            "#,
        )
        .execute(&repo.pool)
        .await;

        assert!(result.is_err(), "expected UNIQUE constraint to reject row");
    }

    #[tokio::test]
    async fn upsert_completed_persists_inputs_snapshot() {
        let repo = setup().await;
        let snapshot = r#"{"week_start":"2026-04-27","days":[]}"#;

        let stored = repo
            .upsert_completed("user-1", week_start(), "review", "m", "v1", snapshot)
            .await
            .unwrap();

        assert_eq!(stored.inputs_snapshot, Some(snapshot.to_string()));
    }

    #[tokio::test]
    async fn upsert_completed_overwrites_inputs_snapshot_on_regeneration() {
        let repo = setup().await;
        repo.upsert_completed("user-1", week_start(), "first", "m", "v1", r#"{"v":1}"#)
            .await
            .unwrap();

        let updated = repo
            .upsert_completed("user-1", week_start(), "second", "m", "v1", r#"{"v":2}"#)
            .await
            .unwrap();

        assert_eq!(updated.inputs_snapshot, Some(r#"{"v":2}"#.to_string()));
    }

    #[tokio::test]
    async fn upsert_failed_leaves_existing_inputs_snapshot_untouched() {
        let repo = setup().await;
        repo.upsert_completed("user-1", week_start(), "first", "m", "v1", r#"{"v":1}"#)
            .await
            .unwrap();
        repo.upsert_failed("user-1", week_start(), "m", "v1", "ignored")
            .await
            .unwrap();

        let stored = repo
            .find_by_user_and_week("user-1", week_start())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.inputs_snapshot, Some(r#"{"v":1}"#.to_string()));
    }

    #[tokio::test]
    async fn fetch_completed_in_range_returns_completed_reviews_in_ascending_order() {
        let repo = setup().await;
        let w1 = NaiveDate::from_ymd_opt(2026, 4, 13).unwrap();
        let w2 = NaiveDate::from_ymd_opt(2026, 4, 20).unwrap();
        let w3 = NaiveDate::from_ymd_opt(2026, 4, 27).unwrap();

        repo.upsert_completed("user-1", w2, "second", "m", "v1", "{}")
            .await
            .unwrap();
        repo.upsert_completed("user-1", w1, "first", "m", "v1", "{}")
            .await
            .unwrap();
        repo.upsert_completed("user-1", w3, "third", "m", "v1", "{}")
            .await
            .unwrap();

        let reviews = repo
            .fetch_completed_in_range("user-1", w1, NaiveDate::from_ymd_opt(2026, 5, 4).unwrap())
            .await
            .unwrap();

        assert_eq!(reviews.len(), 3);
        assert_eq!(reviews[0].week_start_date, w1);
        assert_eq!(reviews[1].week_start_date, w2);
        assert_eq!(reviews[2].week_start_date, w3);
    }

    #[tokio::test]
    async fn fetch_completed_in_range_treats_end_date_as_exclusive() {
        let repo = setup().await;
        let w1 = NaiveDate::from_ymd_opt(2026, 4, 20).unwrap();
        let w2 = NaiveDate::from_ymd_opt(2026, 4, 27).unwrap();

        repo.upsert_completed("user-1", w1, "in", "m", "v1", "{}")
            .await
            .unwrap();
        repo.upsert_completed("user-1", w2, "out", "m", "v1", "{}")
            .await
            .unwrap();

        let reviews = repo
            .fetch_completed_in_range("user-1", w1, w2)
            .await
            .unwrap();

        assert_eq!(reviews.len(), 1);
        assert_eq!(reviews[0].review_text, Some("in".to_string()));
    }

    #[tokio::test]
    async fn fetch_completed_in_range_ignores_caller_user_id() {
        let repo = setup().await;
        let w = week_start();

        repo.upsert_completed("user-1", w, "mine", "m", "v1", "{}")
            .await
            .unwrap();
        repo.upsert_completed("user-2", w, "theirs", "m", "v1", "{}")
            .await
            .unwrap();

        let reviews = repo
            .fetch_completed_in_range("user-1", w, NaiveDate::from_ymd_opt(2026, 5, 4).unwrap())
            .await
            .unwrap();

        assert_eq!(reviews.len(), 1);
        assert_eq!(reviews[0].review_text, Some("theirs".to_string()));
    }

    #[tokio::test]
    async fn fetch_completed_in_range_excludes_failed_reviews() {
        let repo = setup().await;
        let w1 = NaiveDate::from_ymd_opt(2026, 4, 20).unwrap();
        let w2 = NaiveDate::from_ymd_opt(2026, 4, 27).unwrap();

        repo.upsert_completed("user-1", w1, "ok", "m", "v1", "{}")
            .await
            .unwrap();
        repo.upsert_failed("user-1", w2, "m", "v1", "boom")
            .await
            .unwrap();

        let reviews = repo
            .fetch_completed_in_range("user-1", w1, NaiveDate::from_ymd_opt(2026, 5, 4).unwrap())
            .await
            .unwrap();

        assert_eq!(reviews.len(), 1);
        assert_eq!(reviews[0].week_start_date, w1);
    }

    #[tokio::test]
    async fn fetch_completed_in_range_returns_empty_when_no_reviews_match() {
        let repo = setup().await;

        let reviews = repo
            .fetch_completed_in_range(
                "user-1",
                NaiveDate::from_ymd_opt(2026, 4, 20).unwrap(),
                NaiveDate::from_ymd_opt(2026, 4, 27).unwrap(),
            )
            .await
            .unwrap();

        assert!(reviews.is_empty());
    }

    #[tokio::test]
    async fn upsert_failed_starts_with_null_inputs_snapshot() {
        let repo = setup().await;
        let stored = repo
            .upsert_failed("user-1", week_start(), "m", "v1", "boom")
            .await
            .unwrap();
        assert_eq!(stored.inputs_snapshot, None);
    }
}
