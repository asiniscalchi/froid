pub mod repository;

use chrono::{DateTime, NaiveDate, Utc};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DailyReview {
    pub id: i64,
    pub user_id: String,
    pub review_date: NaiveDate,
    pub review_text: Option<String>,
    pub model: String,
    pub prompt_version: String,
    pub status: DailyReviewStatus,
    pub error_message: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DailyReviewStatus {
    Completed,
    Failed,
}
