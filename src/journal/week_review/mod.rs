pub mod generator;
pub mod repository;

use chrono::{DateTime, NaiveDate, Utc};

use crate::journal::review::signals::types::DailyReviewSignal;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WeeklyReview {
    pub id: i64,
    pub user_id: String,
    pub week_start_date: NaiveDate,
    pub review_text: Option<String>,
    pub model: String,
    pub prompt_version: String,
    pub status: WeeklyReviewStatus,
    pub error_message: Option<String>,
    pub delivered_at: Option<DateTime<Utc>>,
    pub delivery_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeeklyReviewStatus {
    Completed,
    Failed,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WeeklyReviewInput {
    pub week_start: NaiveDate,
    pub days: Vec<DailyReviewSlice>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DailyReviewSlice {
    pub date: NaiveDate,
    pub review_text: String,
    pub signals: Vec<DailyReviewSignal>,
}
