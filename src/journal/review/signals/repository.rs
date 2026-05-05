use std::{error::Error, fmt};

use chrono::{DateTime, NaiveDate, Utc};
use sqlx::{Row, SqlitePool, sqlite::SqliteRow};

use crate::{
    journal::extraction::{BehaviorValence, NeedStatus},
    messages::SINGLE_USER_ID,
};

use super::types::{DailyReviewSignal, DailyReviewSignalCandidate, SignalType};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DailyReviewSignalRepositoryError {
    Storage(String),
    InvalidSignalType(String),
    InvalidReviewDate(String),
}

impl fmt::Display for DailyReviewSignalRepositoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Storage(message) => write!(f, "{message}"),
            Self::InvalidSignalType(value) => {
                write!(f, "invalid signal_type stored in database: {value}")
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

#[derive(Debug, Clone, Default)]
pub struct SignalSearchFilters {
    pub signal_type: Option<SignalType>,
    pub label_contains: Option<String>,
    pub status: Option<NeedStatus>,
    pub valence: Option<BehaviorValence>,
    pub from_date: Option<NaiveDate>,
    pub to_date_exclusive: Option<NaiveDate>,
    pub min_strength: Option<f32>,
    pub limit: u32,
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
        _user_id: &str,
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
                    (daily_review_id, review_date, signal_type, label, status, valence,
                     strength, confidence, evidence, model, prompt_version)
                VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                "#,
            )
            .bind(daily_review_id)
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
            SELECT id, daily_review_id, review_date, signal_type, label, status,
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

    pub async fn find_completed_reviews_missing_signals(
        &self,
        limit: u32,
    ) -> Result<Vec<(i64, String, NaiveDate)>, DailyReviewSignalRepositoryError> {
        let rows = sqlx::query(
            r#"
            SELECT dr.id, dr.review_date
            FROM daily_reviews dr
            WHERE dr.status = 'completed'
              AND dr.review_text IS NOT NULL
              AND dr.review_text != ''
              AND (dr.signals_status IS NULL OR dr.signals_status = 'failed')
            ORDER BY dr.review_date ASC
            LIMIT ?
            "#,
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|row| {
                let review_date_str = row.get::<String, _>("review_date");
                let review_date =
                    NaiveDate::parse_from_str(&review_date_str, "%Y-%m-%d").map_err(|_| {
                        DailyReviewSignalRepositoryError::InvalidReviewDate(review_date_str)
                    })?;
                Ok((
                    row.get::<i64, _>("id"),
                    SINGLE_USER_ID.to_string(),
                    review_date,
                ))
            })
            .collect()
    }

    pub async fn count_completed_reviews_missing_signals(
        &self,
    ) -> Result<u32, DailyReviewSignalRepositoryError> {
        let count: i64 = sqlx::query_scalar(
            r#"
            SELECT COUNT(*)
            FROM daily_reviews dr
            WHERE dr.status = 'completed'
              AND dr.review_text IS NOT NULL
              AND dr.review_text != ''
              AND (dr.signals_status IS NULL OR dr.signals_status = 'failed')
            "#,
        )
        .fetch_one(&self.pool)
        .await?;

        Ok(count as u32)
    }

    pub async fn find_by_user_and_date(
        &self,
        _user_id: &str,
        review_date: NaiveDate,
    ) -> Result<Vec<DailyReviewSignal>, DailyReviewSignalRepositoryError> {
        let rows = sqlx::query(
            r#"
            SELECT id, daily_review_id, review_date, signal_type, label, status,
                   valence, strength, confidence, evidence, model, prompt_version,
                   created_at, updated_at
            FROM daily_review_signals
            WHERE review_date = ?
            ORDER BY id ASC
            "#,
        )
        .bind(review_date.to_string())
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(row_to_signal).collect()
    }

    pub async fn search(
        &self,
        _user_id: &str,
        filters: &SignalSearchFilters,
    ) -> Result<Vec<DailyReviewSignal>, DailyReviewSignalRepositoryError> {
        let mut sql = String::from(
            r#"SELECT id, daily_review_id, review_date, signal_type, label, status,
                      valence, strength, confidence, evidence, model, prompt_version,
                      created_at, updated_at
               FROM daily_review_signals
               WHERE 1 = 1"#,
        );
        if filters.signal_type.is_some() {
            sql.push_str(" AND signal_type = ?");
        }
        if filters.label_contains.is_some() {
            sql.push_str(" AND LOWER(label) LIKE LOWER(?)");
        }
        if filters.status.is_some() {
            sql.push_str(" AND status = ?");
        }
        if filters.valence.is_some() {
            sql.push_str(" AND valence = ?");
        }
        if filters.from_date.is_some() {
            sql.push_str(" AND review_date >= ?");
        }
        if filters.to_date_exclusive.is_some() {
            sql.push_str(" AND review_date < ?");
        }
        if filters.min_strength.is_some() {
            sql.push_str(" AND strength >= ?");
        }
        sql.push_str(" ORDER BY review_date ASC, id ASC LIMIT ?");

        let mut query = sqlx::query(&sql);
        if let Some(t) = filters.signal_type.as_ref() {
            query = query.bind(t.as_str());
        }
        if let Some(l) = filters.label_contains.as_ref() {
            query = query.bind(format!("%{l}%"));
        }
        if let Some(s) = filters.status.as_ref() {
            query = query.bind(need_status_to_str(s));
        }
        if let Some(v) = filters.valence.as_ref() {
            query = query.bind(behavior_valence_to_str(v));
        }
        if let Some(d) = filters.from_date.as_ref() {
            query = query.bind(d.to_string());
        }
        if let Some(d) = filters.to_date_exclusive.as_ref() {
            query = query.bind(d.to_string());
        }
        if let Some(s) = filters.min_strength {
            query = query.bind(s as f64);
        }
        query = query.bind(filters.limit);

        let rows = query.fetch_all(&self.pool).await?;
        rows.into_iter().map(row_to_signal).collect()
    }

    pub async fn find_by_user_in_range(
        &self,
        _user_id: &str,
        start_date: NaiveDate,
        end_date_exclusive: NaiveDate,
    ) -> Result<Vec<DailyReviewSignal>, DailyReviewSignalRepositoryError> {
        let rows = sqlx::query(
            r#"
            SELECT id, daily_review_id, review_date, signal_type, label, status,
                   valence, strength, confidence, evidence, model, prompt_version,
                   created_at, updated_at
            FROM daily_review_signals
            WHERE review_date >= ?
              AND review_date < ?
            ORDER BY review_date ASC, id ASC
            "#,
        )
        .bind(start_date.to_string())
        .bind(end_date_exclusive.to_string())
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(row_to_signal).collect()
    }
}

fn row_to_signal(row: SqliteRow) -> Result<DailyReviewSignal, DailyReviewSignalRepositoryError> {
    let signal_type_str = row.get::<String, _>("signal_type");
    let signal_type = SignalType::from_str(&signal_type_str).ok_or(
        DailyReviewSignalRepositoryError::InvalidSignalType(signal_type_str),
    )?;

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
        user_id: SINGLE_USER_ID.to_string(),
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

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;
    use sqlx::SqlitePool;

    use super::*;
    use crate::{
        database,
        journal::{
            extraction::NeedStatus,
            review::repository::DailyReviewRepository,
            review::signals::types::{DailyReviewSignalCandidate, SignalType},
        },
        messages::{IncomingMessage, MessageSource},
    };

    async fn setup() -> (
        DailyReviewSignalRepository,
        DailyReviewRepository,
        SqlitePool,
    ) {
        database::register_sqlite_vec_extension();
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        (
            DailyReviewSignalRepository::new(pool.clone()),
            DailyReviewRepository::new(pool.clone()),
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

        let review_repo = DailyReviewRepository::new(pool.clone());
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

    #[tokio::test]
    async fn replace_inserts_signals_and_returns_them() {
        let (repo, _reviews, pool) = setup().await;
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
        assert_eq!(stored[1].signal_type, SignalType::Need);
        assert_eq!(stored[1].status, Some(NeedStatus::Unmet));
    }

    #[tokio::test]
    async fn find_completed_reviews_missing_signals_returns_reviews_without_completed_status() {
        let (repo, reviews, pool) = setup().await;
        let review_id = insert_daily_review(&pool).await;

        let candidates = repo
            .find_completed_reviews_missing_signals(10)
            .await
            .unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].0, review_id);

        reviews.mark_signals_completed(review_id).await.unwrap();

        let candidates_after = repo
            .find_completed_reviews_missing_signals(10)
            .await
            .unwrap();
        assert!(candidates_after.is_empty());
    }

    #[tokio::test]
    async fn find_completed_reviews_missing_signals_ignores_pending() {
        let (repo, reviews, pool) = setup().await;
        let review_id = insert_daily_review(&pool).await;

        reviews
            .mark_signals_pending(review_id, "model", "v1")
            .await
            .unwrap();

        let candidates = repo
            .find_completed_reviews_missing_signals(10)
            .await
            .unwrap();
        assert!(candidates.is_empty());
    }

    #[tokio::test]
    async fn find_completed_reviews_missing_signals_includes_failed() {
        let (repo, reviews, pool) = setup().await;
        let review_id = insert_daily_review(&pool).await;

        reviews
            .mark_signals_failed(review_id, "error")
            .await
            .unwrap();

        let candidates = repo
            .find_completed_reviews_missing_signals(10)
            .await
            .unwrap();
        assert_eq!(candidates.len(), 1);
    }

    async fn insert_daily_review_for(pool: &SqlitePool, user_id: &str, date: NaiveDate) -> i64 {
        let review_repo = DailyReviewRepository::new(pool.clone());
        review_repo
            .upsert_completed(user_id, date, "review text", "model", "v1")
            .await
            .unwrap()
            .id
    }

    #[tokio::test]
    async fn find_by_user_in_range_returns_signals_in_range_ordered_by_date_then_id() {
        let (repo, _reviews, pool) = setup().await;
        let monday = NaiveDate::from_ymd_opt(2026, 4, 27).unwrap();
        let wednesday = NaiveDate::from_ymd_opt(2026, 4, 29).unwrap();
        let outside = NaiveDate::from_ymd_opt(2026, 5, 4).unwrap();

        let monday_review = insert_daily_review_for(&pool, "user-1", monday).await;
        let wednesday_review = insert_daily_review_for(&pool, "user-1", wednesday).await;
        let outside_review = insert_daily_review_for(&pool, "user-1", outside).await;

        repo.replace_in_transaction(
            monday_review,
            "user-1",
            monday,
            &[theme_candidate()],
            "m",
            "v1",
        )
        .await
        .unwrap();
        repo.replace_in_transaction(
            wednesday_review,
            "user-1",
            wednesday,
            &[theme_candidate(), need_candidate()],
            "m",
            "v1",
        )
        .await
        .unwrap();
        repo.replace_in_transaction(
            outside_review,
            "user-1",
            outside,
            &[theme_candidate()],
            "m",
            "v1",
        )
        .await
        .unwrap();

        let in_range = repo
            .find_by_user_in_range(
                "user-1",
                monday,
                NaiveDate::from_ymd_opt(2026, 5, 4).unwrap(),
            )
            .await
            .unwrap();

        let dates: Vec<_> = in_range.iter().map(|s| s.review_date).collect();
        assert_eq!(dates, vec![monday, wednesday, wednesday]);
    }

    fn behavior_candidate() -> DailyReviewSignalCandidate {
        DailyReviewSignalCandidate {
            signal_type: SignalType::Behavior,
            label: "plan switching".to_string(),
            status: None,
            valence: Some(BehaviorValence::Negative),
            strength: 0.4,
            confidence: 0.8,
            evidence: "Review notes plan was changed multiple times.".to_string(),
        }
    }

    async fn seed_three_signals(pool: &SqlitePool) {
        let monday = NaiveDate::from_ymd_opt(2026, 4, 27).unwrap();
        let tuesday = NaiveDate::from_ymd_opt(2026, 4, 28).unwrap();
        let wednesday = NaiveDate::from_ymd_opt(2026, 4, 29).unwrap();

        let monday_review = insert_daily_review_for(pool, "user-1", monday).await;
        let tuesday_review = insert_daily_review_for(pool, "user-1", tuesday).await;
        let wednesday_review = insert_daily_review_for(pool, "user-1", wednesday).await;

        let repo = DailyReviewSignalRepository::new(pool.clone());
        repo.replace_in_transaction(
            monday_review,
            "user-1",
            monday,
            &[theme_candidate()],
            "m",
            "v1",
        )
        .await
        .unwrap();
        repo.replace_in_transaction(
            tuesday_review,
            "user-1",
            tuesday,
            &[need_candidate()],
            "m",
            "v1",
        )
        .await
        .unwrap();
        repo.replace_in_transaction(
            wednesday_review,
            "user-1",
            wednesday,
            &[behavior_candidate()],
            "m",
            "v1",
        )
        .await
        .unwrap();
    }

    fn filters(limit: u32) -> SignalSearchFilters {
        SignalSearchFilters {
            limit,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn search_with_no_filters_returns_all_signals_for_user_in_date_then_id_order() {
        let (repo, _reviews, pool) = setup().await;
        seed_three_signals(&pool).await;

        let rows = repo.search("user-1", &filters(10)).await.unwrap();

        assert_eq!(rows.len(), 3);
        let dates: Vec<_> = rows.iter().map(|s| s.review_date.to_string()).collect();
        assert_eq!(dates, vec!["2026-04-27", "2026-04-28", "2026-04-29"]);
    }

    #[tokio::test]
    async fn search_filters_by_signal_type() {
        let (repo, _reviews, pool) = setup().await;
        seed_three_signals(&pool).await;

        let rows = repo
            .search(
                "user-1",
                &SignalSearchFilters {
                    signal_type: Some(SignalType::Need),
                    ..filters(10)
                },
            )
            .await
            .unwrap();

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].signal_type, SignalType::Need);
    }

    #[tokio::test]
    async fn search_filters_by_status() {
        let (repo, _reviews, pool) = setup().await;
        seed_three_signals(&pool).await;

        let rows = repo
            .search(
                "user-1",
                &SignalSearchFilters {
                    status: Some(NeedStatus::Unmet),
                    ..filters(10)
                },
            )
            .await
            .unwrap();

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].status, Some(NeedStatus::Unmet));
    }

    #[tokio::test]
    async fn search_filters_by_valence() {
        let (repo, _reviews, pool) = setup().await;
        seed_three_signals(&pool).await;

        let rows = repo
            .search(
                "user-1",
                &SignalSearchFilters {
                    valence: Some(BehaviorValence::Negative),
                    ..filters(10)
                },
            )
            .await
            .unwrap();

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].valence, Some(BehaviorValence::Negative));
    }

    #[tokio::test]
    async fn search_filters_by_label_contains_case_insensitive() {
        let (repo, _reviews, pool) = setup().await;
        seed_three_signals(&pool).await;

        let rows = repo
            .search(
                "user-1",
                &SignalSearchFilters {
                    label_contains: Some("PHYSICAL".to_string()),
                    ..filters(10)
                },
            )
            .await
            .unwrap();

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].label, "physical appearance");
    }

    #[tokio::test]
    async fn search_filters_by_date_range_with_exclusive_end() {
        let (repo, _reviews, pool) = setup().await;
        seed_three_signals(&pool).await;

        let rows = repo
            .search(
                "user-1",
                &SignalSearchFilters {
                    from_date: Some(NaiveDate::from_ymd_opt(2026, 4, 28).unwrap()),
                    to_date_exclusive: Some(NaiveDate::from_ymd_opt(2026, 4, 29).unwrap()),
                    ..filters(10)
                },
            )
            .await
            .unwrap();

        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].review_date,
            NaiveDate::from_ymd_opt(2026, 4, 28).unwrap()
        );
    }

    #[tokio::test]
    async fn search_filters_by_min_strength() {
        let (repo, _reviews, pool) = setup().await;
        seed_three_signals(&pool).await;

        let rows = repo
            .search(
                "user-1",
                &SignalSearchFilters {
                    min_strength: Some(0.75),
                    ..filters(10)
                },
            )
            .await
            .unwrap();

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].label, "physical appearance");
    }

    #[tokio::test]
    async fn search_respects_limit() {
        let (repo, _reviews, pool) = setup().await;
        seed_three_signals(&pool).await;

        let rows = repo.search("user-1", &filters(2)).await.unwrap();

        assert_eq!(rows.len(), 2);
    }

    #[tokio::test]
    async fn search_uses_single_user_scope() {
        let (repo, _reviews, pool) = setup().await;
        let target = NaiveDate::from_ymd_opt(2026, 4, 27).unwrap();
        let user_one_review = insert_daily_review_for(&pool, "user-1", target).await;
        let user_two_review = insert_daily_review_for(&pool, "user-2", target).await;

        repo.replace_in_transaction(
            user_one_review,
            "user-1",
            target,
            &[theme_candidate()],
            "m",
            "v1",
        )
        .await
        .unwrap();
        repo.replace_in_transaction(
            user_two_review,
            "user-2",
            target,
            &[theme_candidate()],
            "m",
            "v1",
        )
        .await
        .unwrap();

        let rows = repo.search("user-1", &filters(10)).await.unwrap();

        assert_eq!(rows.len(), 1);
        assert!(rows.iter().all(|row| row.user_id == SINGLE_USER_ID));
    }

    #[tokio::test]
    async fn search_combines_multiple_filters() {
        let (repo, _reviews, pool) = setup().await;
        seed_three_signals(&pool).await;

        let rows = repo
            .search(
                "user-1",
                &SignalSearchFilters {
                    signal_type: Some(SignalType::Behavior),
                    valence: Some(BehaviorValence::Negative),
                    min_strength: Some(0.3),
                    ..filters(10)
                },
            )
            .await
            .unwrap();

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].signal_type, SignalType::Behavior);
    }

    #[tokio::test]
    async fn search_returns_empty_when_no_signals_match() {
        let (repo, _reviews, pool) = setup().await;
        seed_three_signals(&pool).await;

        let rows = repo
            .search(
                "user-1",
                &SignalSearchFilters {
                    signal_type: Some(SignalType::Tension),
                    ..filters(10)
                },
            )
            .await
            .unwrap();

        assert!(rows.is_empty());
    }

    #[tokio::test]
    async fn find_by_user_in_range_uses_single_user_scope() {
        let (repo, _reviews, pool) = setup().await;
        let target = NaiveDate::from_ymd_opt(2026, 4, 27).unwrap();
        let user_one_review = insert_daily_review_for(&pool, "user-1", target).await;
        let user_two_review = insert_daily_review_for(&pool, "user-2", target).await;

        repo.replace_in_transaction(
            user_one_review,
            "user-1",
            target,
            &[theme_candidate()],
            "m",
            "v1",
        )
        .await
        .unwrap();
        repo.replace_in_transaction(
            user_two_review,
            "user-2",
            target,
            &[theme_candidate()],
            "m",
            "v1",
        )
        .await
        .unwrap();

        let rows = repo
            .find_by_user_in_range(
                "user-1",
                target,
                NaiveDate::from_ymd_opt(2026, 5, 4).unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(rows.len(), 1);
        assert!(rows.iter().all(|row| row.user_id == SINGLE_USER_ID));
    }
}
