use std::{error::Error, fmt};

use chrono::{DateTime, NaiveDate, Utc};
use sqlx::{Row, SqlitePool, sqlite::SqliteRow};

use crate::journal::extraction::{BehaviorValence, NeedStatus};

use super::types::{
    DailyReviewSignal, DailyReviewSignalCandidate, DailyReviewSignalJob, SignalJobStatus,
    SignalType,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DailyReviewSignalRepositoryError {
    Storage(String),
    InvalidSignalType(String),
    InvalidSignalJobStatus(String),
    InvalidReviewDate(String),
}

impl fmt::Display for DailyReviewSignalRepositoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Storage(message) => write!(f, "{message}"),
            Self::InvalidSignalType(value) => {
                write!(f, "invalid signal_type stored in database: {value}")
            }
            Self::InvalidSignalJobStatus(value) => {
                write!(f, "invalid signal job status stored in database: {value}")
            }
            Self::InvalidReviewDate(value) => {
                write!(f, "invalid review_date stored in database: {value}")
            }
        }
    }
}

impl Error for DailyReviewSignalRepositoryError {}

impl From<sqlx::Error> for DailyReviewSignalRepositoryError {
    fn from(error: sqlx::Error) -> Self {
        Self::Storage(error.to_string())
    }
}

#[derive(Debug, Clone)]
pub struct DailyReviewSignalRepository {
    pool: SqlitePool,
}

impl DailyReviewSignalRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Atomically replaces all signals for the given daily review with the provided candidates.
    /// Deletes existing signals first, then inserts new ones in the same transaction.
    pub async fn replace_in_transaction(
        &self,
        daily_review_id: i64,
        user_id: &str,
        review_date: NaiveDate,
        candidates: &[DailyReviewSignalCandidate],
        model: &str,
        prompt_version: &str,
    ) -> Result<Vec<DailyReviewSignal>, DailyReviewSignalRepositoryError> {
        let mut tx = self.pool.begin().await?;

        sqlx::query("DELETE FROM daily_review_signals WHERE daily_review_id = ?")
            .bind(daily_review_id)
            .execute(&mut *tx)
            .await?;

        for candidate in candidates {
            sqlx::query(
                r#"
                INSERT INTO daily_review_signals
                    (daily_review_id, user_id, review_date, signal_type, label, status, valence,
                     strength, confidence, evidence, model, prompt_version)
                VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                "#,
            )
            .bind(daily_review_id)
            .bind(user_id)
            .bind(review_date.to_string())
            .bind(candidate.signal_type.as_str())
            .bind(&candidate.label)
            .bind(candidate.status.as_ref().map(need_status_to_str))
            .bind(candidate.valence.as_ref().map(behavior_valence_to_str))
            .bind(candidate.strength)
            .bind(candidate.confidence)
            .bind(&candidate.evidence)
            .bind(model)
            .bind(prompt_version)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;

        self.find_by_daily_review_id(daily_review_id).await
    }

    pub async fn find_by_daily_review_id(
        &self,
        daily_review_id: i64,
    ) -> Result<Vec<DailyReviewSignal>, DailyReviewSignalRepositoryError> {
        let rows = sqlx::query(
            r#"
            SELECT id, daily_review_id, user_id, review_date, signal_type, label, status,
                   valence, strength, confidence, evidence, model, prompt_version,
                   created_at, updated_at
            FROM daily_review_signals
            WHERE daily_review_id = ?
            ORDER BY id ASC
            "#,
        )
        .bind(daily_review_id)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(row_to_signal).collect()
    }

    pub async fn find_by_user_and_date(
        &self,
        user_id: &str,
        review_date: NaiveDate,
    ) -> Result<Vec<DailyReviewSignal>, DailyReviewSignalRepositoryError> {
        let rows = sqlx::query(
            r#"
            SELECT id, daily_review_id, user_id, review_date, signal_type, label, status,
                   valence, strength, confidence, evidence, model, prompt_version,
                   created_at, updated_at
            FROM daily_review_signals
            WHERE user_id = ? AND review_date = ?
            ORDER BY id ASC
            "#,
        )
        .bind(user_id)
        .bind(review_date.to_string())
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(row_to_signal).collect()
    }
}

fn row_to_signal(row: SqliteRow) -> Result<DailyReviewSignal, DailyReviewSignalRepositoryError> {
    let signal_type_str = row.get::<String, _>("signal_type");
    let signal_type = SignalType::from_str(&signal_type_str)
        .ok_or(DailyReviewSignalRepositoryError::InvalidSignalType(signal_type_str))?;

    let review_date_str = row.get::<String, _>("review_date");
    let review_date = NaiveDate::parse_from_str(&review_date_str, "%Y-%m-%d")
        .map_err(|_| DailyReviewSignalRepositoryError::InvalidReviewDate(review_date_str))?;

    let status = row
        .get::<Option<String>, _>("status")
        .map(|s| need_status_from_str(&s))
        .transpose()?;

    let valence = row
        .get::<Option<String>, _>("valence")
        .map(|s| behavior_valence_from_str(&s))
        .transpose()?;

    Ok(DailyReviewSignal {
        id: row.get("id"),
        daily_review_id: row.get("daily_review_id"),
        user_id: row.get("user_id"),
        review_date,
        signal_type,
        label: row.get("label"),
        status,
        valence,
        strength: row.get::<f64, _>("strength") as f32,
        confidence: row.get::<f64, _>("confidence") as f32,
        evidence: row.get("evidence"),
        model: row.get("model"),
        prompt_version: row.get("prompt_version"),
        created_at: row.get::<DateTime<Utc>, _>("created_at"),
        updated_at: row.get::<DateTime<Utc>, _>("updated_at"),
    })
}

fn need_status_to_str(status: &NeedStatus) -> &'static str {
    match status {
        NeedStatus::Activated => "activated",
        NeedStatus::Unmet => "unmet",
        NeedStatus::Fulfilled => "fulfilled",
        NeedStatus::Unclear => "unclear",
    }
}

fn need_status_from_str(s: &str) -> Result<NeedStatus, DailyReviewSignalRepositoryError> {
    match s {
        "activated" => Ok(NeedStatus::Activated),
        "unmet" => Ok(NeedStatus::Unmet),
        "fulfilled" => Ok(NeedStatus::Fulfilled),
        "unclear" => Ok(NeedStatus::Unclear),
        other => Err(DailyReviewSignalRepositoryError::InvalidSignalType(
            format!("invalid need status: {other}"),
        )),
    }
}

fn behavior_valence_to_str(valence: &BehaviorValence) -> &'static str {
    match valence {
        BehaviorValence::Positive => "positive",
        BehaviorValence::Negative => "negative",
        BehaviorValence::Ambiguous => "ambiguous",
        BehaviorValence::Neutral => "neutral",
        BehaviorValence::Unclear => "unclear",
    }
}

fn behavior_valence_from_str(s: &str) -> Result<BehaviorValence, DailyReviewSignalRepositoryError> {
    match s {
        "positive" => Ok(BehaviorValence::Positive),
        "negative" => Ok(BehaviorValence::Negative),
        "ambiguous" => Ok(BehaviorValence::Ambiguous),
        "neutral" => Ok(BehaviorValence::Neutral),
        "unclear" => Ok(BehaviorValence::Unclear),
        other => Err(DailyReviewSignalRepositoryError::InvalidSignalType(
            format!("invalid behavior valence: {other}"),
        )),
    }
}

// ── Job repository ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct DailyReviewSignalJobRepository {
    pool: SqlitePool,
}

impl DailyReviewSignalJobRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn insert_pending(
        &self,
        daily_review_id: i64,
    ) -> Result<DailyReviewSignalJob, DailyReviewSignalRepositoryError> {
        let id: i64 = sqlx::query_scalar(
            r#"
            INSERT INTO daily_review_signal_jobs (daily_review_id, status)
            VALUES (?, 'pending')
            RETURNING id
            "#,
        )
        .bind(daily_review_id)
        .fetch_one(&self.pool)
        .await?;

        self.find_by_id(id).await?.ok_or_else(|| {
            DailyReviewSignalRepositoryError::Storage("job not found after insert".into())
        })
    }

    pub async fn mark_started(
        &self,
        job_id: i64,
        model: &str,
        prompt_version: &str,
    ) -> Result<(), DailyReviewSignalRepositoryError> {
        sqlx::query(
            r#"
            UPDATE daily_review_signal_jobs
            SET status = 'pending',
                model = ?,
                prompt_version = ?,
                started_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
            WHERE id = ?
            "#,
        )
        .bind(model)
        .bind(prompt_version)
        .bind(job_id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn mark_completed(
        &self,
        job_id: i64,
    ) -> Result<(), DailyReviewSignalRepositoryError> {
        sqlx::query(
            r#"
            UPDATE daily_review_signal_jobs
            SET status = 'completed',
                error_message = NULL,
                updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
            WHERE id = ?
            "#,
        )
        .bind(job_id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn mark_failed(
        &self,
        job_id: i64,
        error_message: &str,
    ) -> Result<(), DailyReviewSignalRepositoryError> {
        sqlx::query(
            r#"
            UPDATE daily_review_signal_jobs
            SET status = 'failed',
                error_message = ?,
                updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
            WHERE id = ?
            "#,
        )
        .bind(error_message)
        .bind(job_id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn find_by_id(
        &self,
        id: i64,
    ) -> Result<Option<DailyReviewSignalJob>, DailyReviewSignalRepositoryError> {
        let row = sqlx::query(
            r#"
            SELECT id, daily_review_id, status, error_message, model, prompt_version,
                   started_at, created_at, updated_at
            FROM daily_review_signal_jobs
            WHERE id = ?
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        row.map(row_to_job).transpose()
    }
}

fn row_to_job(row: SqliteRow) -> Result<DailyReviewSignalJob, DailyReviewSignalRepositoryError> {
    let status_str = row.get::<String, _>("status");
    let status = match status_str.as_str() {
        "pending" => SignalJobStatus::Pending,
        "completed" => SignalJobStatus::Completed,
        "failed" => SignalJobStatus::Failed,
        _ => {
            return Err(DailyReviewSignalRepositoryError::InvalidSignalJobStatus(
                status_str,
            ));
        }
    };

    Ok(DailyReviewSignalJob {
        id: row.get("id"),
        daily_review_id: row.get("daily_review_id"),
        status,
        error_message: row.get("error_message"),
        model: row.get("model"),
        prompt_version: row.get("prompt_version"),
        started_at: row.get("started_at"),
        created_at: row.get::<DateTime<Utc>, _>("created_at"),
        updated_at: row.get::<DateTime<Utc>, _>("updated_at"),
    })
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;
    use sqlx::SqlitePool;

    use super::*;
    use crate::{
        database,
        journal::{
            extraction::{BehaviorValence, NeedStatus},
            review::signals::types::{DailyReviewSignalCandidate, SignalType},
        },
        messages::{IncomingMessage, MessageSource},
    };

    async fn setup() -> (
        DailyReviewSignalRepository,
        DailyReviewSignalJobRepository,
        SqlitePool,
    ) {
        database::register_sqlite_vec_extension();
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        (
            DailyReviewSignalRepository::new(pool.clone()),
            DailyReviewSignalJobRepository::new(pool.clone()),
            pool,
        )
    }

    async fn insert_daily_review(pool: &SqlitePool) -> i64 {
        let user_id = "user-1";
        let msg = IncomingMessage {
            source: MessageSource::Telegram,
            source_conversation_id: "42".to_string(),
            source_message_id: "1".to_string(),
            user_id: user_id.to_string(),
            text: "entry text".to_string(),
            received_at: chrono::Utc::now(),
        };
        crate::journal::repository::JournalRepository::new(pool.clone())
            .store(&msg)
            .await
            .unwrap();

        let review_repo =
            crate::journal::review::repository::DailyReviewRepository::new(pool.clone());
        let review = review_repo
            .upsert_completed(
                user_id,
                NaiveDate::from_ymd_opt(2026, 4, 28).unwrap(),
                "review text",
                "model",
                "v1",
            )
            .await
            .unwrap();

        review.id
    }

    fn date() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 4, 28).unwrap()
    }

    fn theme_candidate() -> DailyReviewSignalCandidate {
        DailyReviewSignalCandidate {
            signal_type: SignalType::Theme,
            label: "physical appearance".to_string(),
            status: None,
            valence: None,
            strength: 0.8,
            confidence: 0.9,
            evidence: "Review mentions training and diet.".to_string(),
        }
    }

    fn need_candidate() -> DailyReviewSignalCandidate {
        DailyReviewSignalCandidate {
            signal_type: SignalType::Need,
            label: "control".to_string(),
            status: Some(NeedStatus::Unmet),
            valence: None,
            strength: 0.7,
            confidence: 0.85,
            evidence: "Review notes repeated attempts to regain control.".to_string(),
        }
    }

    fn behavior_candidate() -> DailyReviewSignalCandidate {
        DailyReviewSignalCandidate {
            signal_type: SignalType::Behavior,
            label: "plan switching".to_string(),
            status: None,
            valence: Some(BehaviorValence::Negative),
            strength: 0.75,
            confidence: 0.8,
            evidence: "Plan changed multiple times.".to_string(),
        }
    }

    #[tokio::test]
    async fn replace_inserts_signals_and_returns_them() {
        let (repo, _jobs, pool) = setup().await;
        let review_id = insert_daily_review(&pool).await;

        let stored = repo
            .replace_in_transaction(
                review_id,
                "user-1",
                date(),
                &[theme_candidate(), need_candidate()],
                "model",
                "v1",
            )
            .await
            .unwrap();

        assert_eq!(stored.len(), 2);
        assert_eq!(stored[0].signal_type, SignalType::Theme);
        assert_eq!(stored[0].label, "physical appearance");
        assert_eq!(stored[0].status, None);
        assert_eq!(stored[0].valence, None);
        assert_eq!(stored[1].signal_type, SignalType::Need);
        assert_eq!(stored[1].status, Some(NeedStatus::Unmet));
    }

    #[tokio::test]
    async fn replace_stores_behavior_valence() {
        let (repo, _jobs, pool) = setup().await;
        let review_id = insert_daily_review(&pool).await;

        let stored = repo
            .replace_in_transaction(
                review_id,
                "user-1",
                date(),
                &[behavior_candidate()],
                "model",
                "v1",
            )
            .await
            .unwrap();

        assert_eq!(stored[0].signal_type, SignalType::Behavior);
        assert_eq!(stored[0].valence, Some(BehaviorValence::Negative));
        assert_eq!(stored[0].status, None);
    }

    #[tokio::test]
    async fn replace_deletes_existing_signals_before_inserting() {
        let (repo, _jobs, pool) = setup().await;
        let review_id = insert_daily_review(&pool).await;

        repo.replace_in_transaction(
            review_id,
            "user-1",
            date(),
            &[theme_candidate()],
            "model",
            "v1",
        )
        .await
        .unwrap();

        let second = repo
            .replace_in_transaction(
                review_id,
                "user-1",
                date(),
                &[need_candidate()],
                "model",
                "v1",
            )
            .await
            .unwrap();

        assert_eq!(second.len(), 1);
        assert_eq!(second[0].signal_type, SignalType::Need);
    }

    #[tokio::test]
    async fn replace_with_empty_candidates_clears_all_signals() {
        let (repo, _jobs, pool) = setup().await;
        let review_id = insert_daily_review(&pool).await;

        repo.replace_in_transaction(
            review_id,
            "user-1",
            date(),
            &[theme_candidate()],
            "model",
            "v1",
        )
        .await
        .unwrap();

        let stored = repo
            .replace_in_transaction(review_id, "user-1", date(), &[], "model", "v1")
            .await
            .unwrap();

        assert!(stored.is_empty());
    }

    #[tokio::test]
    async fn find_by_user_and_date_returns_only_matching_signals() {
        let (repo, _jobs, pool) = setup().await;
        let review_id = insert_daily_review(&pool).await;

        repo.replace_in_transaction(
            review_id,
            "user-1",
            date(),
            &[theme_candidate()],
            "model",
            "v1",
        )
        .await
        .unwrap();

        let found = repo.find_by_user_and_date("user-1", date()).await.unwrap();
        let not_found = repo
            .find_by_user_and_date("user-1", NaiveDate::from_ymd_opt(2026, 4, 29).unwrap())
            .await
            .unwrap();
        let other_user = repo.find_by_user_and_date("user-2", date()).await.unwrap();

        assert_eq!(found.len(), 1);
        assert!(not_found.is_empty());
        assert!(other_user.is_empty());
    }

    #[tokio::test]
    async fn find_by_daily_review_id_returns_correct_signals() {
        let (repo, _jobs, pool) = setup().await;
        let review_id = insert_daily_review(&pool).await;

        repo.replace_in_transaction(
            review_id,
            "user-1",
            date(),
            &[theme_candidate(), behavior_candidate()],
            "model",
            "v1",
        )
        .await
        .unwrap();

        let found = repo.find_by_daily_review_id(review_id).await.unwrap();
        assert_eq!(found.len(), 2);
    }

    // ── Job repository tests ─────────────────────────────────────────────

    #[tokio::test]
    async fn insert_pending_creates_job_with_pending_status() {
        let (_signals, jobs, pool) = setup().await;
        let review_id = insert_daily_review(&pool).await;

        let job = jobs.insert_pending(review_id).await.unwrap();

        assert_eq!(job.daily_review_id, review_id);
        assert_eq!(job.status, SignalJobStatus::Pending);
        assert_eq!(job.error_message, None);
        assert_eq!(job.model, None);
        assert_eq!(job.prompt_version, None);
        assert_eq!(job.started_at, None);
    }

    #[tokio::test]
    async fn mark_started_records_model_and_prompt_version() {
        let (_signals, jobs, pool) = setup().await;
        let review_id = insert_daily_review(&pool).await;
        let job = jobs.insert_pending(review_id).await.unwrap();

        jobs.mark_started(job.id, "test-model", "v1").await.unwrap();

        let updated = jobs.find_by_id(job.id).await.unwrap().unwrap();
        assert_eq!(updated.status, SignalJobStatus::Pending);
        assert_eq!(updated.model, Some("test-model".to_string()));
        assert_eq!(updated.prompt_version, Some("v1".to_string()));
        assert!(updated.started_at.is_some());
    }

    #[tokio::test]
    async fn mark_completed_sets_status_and_clears_error() {
        let (_signals, jobs, pool) = setup().await;
        let review_id = insert_daily_review(&pool).await;
        let job = jobs.insert_pending(review_id).await.unwrap();

        jobs.mark_completed(job.id).await.unwrap();

        let updated = jobs.find_by_id(job.id).await.unwrap().unwrap();
        assert_eq!(updated.status, SignalJobStatus::Completed);
        assert_eq!(updated.error_message, None);
    }

    #[tokio::test]
    async fn mark_failed_sets_status_and_records_error() {
        let (_signals, jobs, pool) = setup().await;
        let review_id = insert_daily_review(&pool).await;
        let job = jobs.insert_pending(review_id).await.unwrap();

        jobs.mark_failed(job.id, "provider down").await.unwrap();

        let updated = jobs.find_by_id(job.id).await.unwrap().unwrap();
        assert_eq!(updated.status, SignalJobStatus::Failed);
        assert_eq!(updated.error_message, Some("provider down".to_string()));
    }

    #[tokio::test]
    async fn signal_repository_stores_float_strength_and_confidence() {
        let (repo, _jobs, pool) = setup().await;
        let review_id = insert_daily_review(&pool).await;
        let candidate = DailyReviewSignalCandidate {
            signal_type: SignalType::Tension,
            label: "discipline vs optimization".to_string(),
            status: None,
            valence: None,
            strength: 0.85,
            confidence: 0.88,
            evidence: "Review identifies tension between discipline and plan changes.".to_string(),
        };

        let stored = repo
            .replace_in_transaction(review_id, "user-1", date(), &[candidate], "model", "v1")
            .await
            .unwrap();

        // Allow tiny floating-point tolerance from SQLite REAL
        assert!((stored[0].strength - 0.85).abs() < 1e-5);
        assert!((stored[0].confidence - 0.88).abs() < 1e-5);
    }
}
