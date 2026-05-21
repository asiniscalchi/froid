use async_trait::async_trait;
use chrono::{Duration, NaiveDate};

use crate::journal::review::repository::{DailyReviewRepository, DailyReviewRepositoryError};
use crate::journal::review::{DailyReview, DailyReviewStatus};
use crate::journal::week_review::repository::{
    WeeklyReviewRepository, WeeklyReviewRepositoryError,
};
use crate::journal::week_review::{WeeklyReview, WeeklyReviewStatus};

use super::types::{
    AnalyzerError, DailyReviewView, GetReviewsRequest, UserContext, WeeklyReviewView,
};
use super::validation::validate_range;

#[async_trait]
pub trait ReviewReadService: Send + Sync {
    async fn get_daily_reviews(
        &self,
        ctx: &UserContext,
        request: GetReviewsRequest,
    ) -> Result<Vec<DailyReviewView>, AnalyzerError>;

    async fn get_weekly_reviews(
        &self,
        ctx: &UserContext,
        request: GetReviewsRequest,
    ) -> Result<Vec<WeeklyReviewView>, AnalyzerError>;

    /// Return the completed daily review for `review_date`, or `None` if no
    /// completed review exists for that date.
    async fn get_daily_review(
        &self,
        ctx: &UserContext,
        review_date: NaiveDate,
    ) -> Result<Option<DailyReviewView>, AnalyzerError>;

    /// Return the completed weekly review whose week starts on `week_start`,
    /// or `None` if no completed review exists for that week.
    async fn get_weekly_review(
        &self,
        ctx: &UserContext,
        week_start: NaiveDate,
    ) -> Result<Option<WeeklyReviewView>, AnalyzerError>;
}

#[derive(Debug, Clone)]
pub struct DefaultReviewReadService {
    daily: DailyReviewRepository,
    weekly: WeeklyReviewRepository,
}

impl DefaultReviewReadService {
    pub fn new(daily: DailyReviewRepository, weekly: WeeklyReviewRepository) -> Self {
        Self { daily, weekly }
    }
}

fn map_daily_error(err: DailyReviewRepositoryError) -> AnalyzerError {
    AnalyzerError::Internal(Box::new(err))
}

fn map_weekly_error(err: WeeklyReviewRepositoryError) -> AnalyzerError {
    AnalyzerError::Internal(Box::new(err))
}

#[async_trait]
impl ReviewReadService for DefaultReviewReadService {
    async fn get_daily_reviews(
        &self,
        ctx: &UserContext,
        request: GetReviewsRequest,
    ) -> Result<Vec<DailyReviewView>, AnalyzerError> {
        validate_range(request.from_date, request.to_date_exclusive)?;

        let rows = self
            .daily
            .fetch_completed_in_range(request.from_date, request.to_date_exclusive)
            .await
            .map_err(map_daily_error)?;

        Ok(rows
            .into_iter()
            .map(|review| DailyReviewView {
                review_date: review.review_date,
                review_text: review.review_text.unwrap_or_default(),
                created_at: review.created_at,
            })
            .collect())
    }

    async fn get_weekly_reviews(
        &self,
        ctx: &UserContext,
        request: GetReviewsRequest,
    ) -> Result<Vec<WeeklyReviewView>, AnalyzerError> {
        validate_range(request.from_date, request.to_date_exclusive)?;

        let rows = self
            .weekly
            .fetch_completed_in_range(request.from_date, request.to_date_exclusive)
            .await
            .map_err(map_weekly_error)?;

        Ok(rows
            .into_iter()
            .map(|review| WeeklyReviewView {
                week_start: review.week_start_date,
                week_end: review.week_start_date + Duration::days(6),
                review_text: review.review_text.unwrap_or_default(),
                created_at: review.created_at,
            })
            .collect())
    }

    async fn get_daily_review(
        &self,
        ctx: &UserContext,
        review_date: NaiveDate,
    ) -> Result<Option<DailyReviewView>, AnalyzerError> {
        let row = self
            .daily
            .find_by_user_and_date(review_date)
            .await
            .map_err(map_daily_error)?;

        Ok(row.and_then(daily_view_if_completed))
    }

    async fn get_weekly_review(
        &self,
        ctx: &UserContext,
        week_start: NaiveDate,
    ) -> Result<Option<WeeklyReviewView>, AnalyzerError> {
        let row = self
            .weekly
            .find_by_user_and_week(week_start)
            .await
            .map_err(map_weekly_error)?;

        Ok(row.and_then(weekly_view_if_completed))
    }
}

fn daily_view_if_completed(review: DailyReview) -> Option<DailyReviewView> {
    if review.status != DailyReviewStatus::Completed {
        return None;
    }
    let text = review.review_text.unwrap_or_default();
    if text.trim().is_empty() {
        return None;
    }
    Some(DailyReviewView {
        review_date: review.review_date,
        review_text: text,
        created_at: review.created_at,
    })
}

fn weekly_view_if_completed(review: WeeklyReview) -> Option<WeeklyReviewView> {
    if review.status != WeeklyReviewStatus::Completed {
        return None;
    }
    let text = review.review_text.unwrap_or_default();
    if text.trim().is_empty() {
        return None;
    }
    Some(WeeklyReviewView {
        week_start: review.week_start_date,
        week_end: review.week_start_date + Duration::days(6),
        review_text: text,
        created_at: review.created_at,
    })
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;
    use sqlx::SqlitePool;

    use super::*;
    use crate::database;

    async fn setup() -> (
        DefaultReviewReadService,
        DailyReviewRepository,
        WeeklyReviewRepository,
    ) {
        database::register_sqlite_vec_extension();
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        let daily = DailyReviewRepository::new(pool.clone());
        let weekly = WeeklyReviewRepository::new(pool);
        let service = DefaultReviewReadService::new(daily.clone(), weekly.clone());
        (service, daily, weekly)
    }

    fn ctx() -> UserContext {
        UserContext::new("user-1")
    }

    fn ymd(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn req(from: NaiveDate, to_exclusive: NaiveDate) -> GetReviewsRequest {
        GetReviewsRequest {
            from_date: from,
            to_date_exclusive: to_exclusive,
        }
    }

    #[tokio::test]
    async fn get_daily_reviews_returns_completed_reviews_in_range_ascending() {
        let (service, daily, _) = setup().await;
        daily
            .upsert_completed(ymd(2026, 4, 27), "first", "m", "v1")
            .await
            .unwrap();
        daily
            .upsert_completed(ymd(2026, 4, 28), "second", "m", "v1")
            .await
            .unwrap();
        daily
            .upsert_completed(ymd(2026, 4, 29), "third", "m", "v1")
            .await
            .unwrap();

        let result = service
            .get_daily_reviews(&ctx(), req(ymd(2026, 4, 27), ymd(2026, 4, 29)))
            .await
            .unwrap();

        assert_eq!(result.len(), 2);
        assert_eq!(result[0].review_date, ymd(2026, 4, 27));
        assert_eq!(result[0].review_text, "first");
        assert_eq!(result[1].review_date, ymd(2026, 4, 28));
        assert_eq!(result[1].review_text, "second");
    }

    #[tokio::test]
    async fn get_daily_reviews_excludes_failed_reviews() {
        let (service, daily, _) = setup().await;
        daily
            .upsert_completed(ymd(2026, 4, 27), "ok", "m", "v1")
            .await
            .unwrap();
        daily
            .upsert_failed(ymd(2026, 4, 28), "m", "v1", "boom")
            .await
            .unwrap();

        let result = service
            .get_daily_reviews(&ctx(), req(ymd(2026, 4, 27), ymd(2026, 4, 29)))
            .await
            .unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].review_date, ymd(2026, 4, 27));
    }

    #[tokio::test]
    async fn get_daily_reviews_uses_single_user_scope() {
        let (service, daily, _) = setup().await;
        daily
            .upsert_completed(ymd(2026, 4, 27), "mine", "m", "v1")
            .await
            .unwrap();
        daily
            .upsert_completed(ymd(2026, 4, 27), "theirs", "m", "v1")
            .await
            .unwrap();

        let result = service
            .get_daily_reviews(&ctx(), req(ymd(2026, 4, 27), ymd(2026, 4, 28)))
            .await
            .unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].review_text, "theirs");
    }

    #[tokio::test]
    async fn get_daily_reviews_rejects_inverted_range() {
        let (service, _, _) = setup().await;
        let err = service
            .get_daily_reviews(&ctx(), req(ymd(2026, 4, 28), ymd(2026, 4, 27)))
            .await
            .unwrap_err();
        assert!(matches!(err, AnalyzerError::InvalidArgument(_)));
    }

    #[tokio::test]
    async fn get_daily_reviews_rejects_equal_bounds() {
        let (service, _, _) = setup().await;
        let err = service
            .get_daily_reviews(&ctx(), req(ymd(2026, 4, 28), ymd(2026, 4, 28)))
            .await
            .unwrap_err();
        assert!(matches!(err, AnalyzerError::InvalidArgument(_)));
    }

    #[tokio::test]
    async fn get_weekly_reviews_returns_reviews_with_computed_week_end() {
        let (service, _, weekly) = setup().await;
        let w1 = ymd(2026, 4, 20);
        let w2 = ymd(2026, 4, 27);

        weekly
            .upsert_completed(w1, "first", "m", "v1", "{}")
            .await
            .unwrap();
        weekly
            .upsert_completed(w2, "second", "m", "v1", "{}")
            .await
            .unwrap();

        let result = service
            .get_weekly_reviews(&ctx(), req(w1, ymd(2026, 5, 4)))
            .await
            .unwrap();

        assert_eq!(result.len(), 2);
        assert_eq!(result[0].week_start, w1);
        assert_eq!(result[0].week_end, ymd(2026, 4, 26));
        assert_eq!(result[0].review_text, "first");
        assert_eq!(result[1].week_start, w2);
        assert_eq!(result[1].week_end, ymd(2026, 5, 3));
    }

    #[tokio::test]
    async fn get_weekly_reviews_excludes_failed_reviews() {
        let (service, _, weekly) = setup().await;
        let w1 = ymd(2026, 4, 20);
        let w2 = ymd(2026, 4, 27);

        weekly
            .upsert_completed(w1, "ok", "m", "v1", "{}")
            .await
            .unwrap();
        weekly.upsert_failed(w2, "m", "v1", "boom").await.unwrap();

        let result = service
            .get_weekly_reviews(&ctx(), req(w1, ymd(2026, 5, 4)))
            .await
            .unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].week_start, w1);
    }

    #[tokio::test]
    async fn get_weekly_reviews_uses_single_user_scope() {
        let (service, _, weekly) = setup().await;
        let w = ymd(2026, 4, 20);
        weekly
            .upsert_completed(w, "mine", "m", "v1", "{}")
            .await
            .unwrap();
        weekly
            .upsert_completed(w, "theirs", "m", "v1", "{}")
            .await
            .unwrap();

        let result = service
            .get_weekly_reviews(&ctx(), req(w, ymd(2026, 4, 27)))
            .await
            .unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].review_text, "theirs");
    }

    #[tokio::test]
    async fn get_daily_review_returns_completed_review_for_date() {
        let (service, daily, _) = setup().await;
        daily
            .upsert_completed(ymd(2026, 4, 28), "today", "m", "v1")
            .await
            .unwrap();

        let result = service
            .get_daily_review(&ctx(), ymd(2026, 4, 28))
            .await
            .unwrap()
            .expect("completed review present");

        assert_eq!(result.review_date, ymd(2026, 4, 28));
        assert_eq!(result.review_text, "today");
    }

    #[tokio::test]
    async fn get_daily_review_returns_none_when_missing() {
        let (service, _, _) = setup().await;
        let result = service
            .get_daily_review(&ctx(), ymd(2026, 4, 28))
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn get_daily_review_returns_none_when_failed() {
        let (service, daily, _) = setup().await;
        daily
            .upsert_failed(ymd(2026, 4, 28), "m", "v1", "boom")
            .await
            .unwrap();

        let result = service
            .get_daily_review(&ctx(), ymd(2026, 4, 28))
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn get_weekly_review_returns_completed_review_for_week() {
        let (service, _, weekly) = setup().await;
        let w = ymd(2026, 4, 20);
        weekly
            .upsert_completed(w, "weekly", "m", "v1", "{}")
            .await
            .unwrap();

        let result = service
            .get_weekly_review(&ctx(), w)
            .await
            .unwrap()
            .expect("completed review present");

        assert_eq!(result.week_start, w);
        assert_eq!(result.week_end, ymd(2026, 4, 26));
        assert_eq!(result.review_text, "weekly");
    }

    #[tokio::test]
    async fn get_weekly_review_returns_none_when_missing() {
        let (service, _, _) = setup().await;
        let result = service
            .get_weekly_review(&ctx(), ymd(2026, 4, 20))
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn get_weekly_review_returns_none_when_failed() {
        let (service, _, weekly) = setup().await;
        let w = ymd(2026, 4, 20);
        weekly.upsert_failed(w, "m", "v1", "boom").await.unwrap();

        let result = service.get_weekly_review(&ctx(), w).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn get_weekly_reviews_rejects_inverted_range() {
        let (service, _, _) = setup().await;
        let err = service
            .get_weekly_reviews(&ctx(), req(ymd(2026, 4, 28), ymd(2026, 4, 27)))
            .await
            .unwrap_err();
        assert!(matches!(err, AnalyzerError::InvalidArgument(_)));
    }
}
