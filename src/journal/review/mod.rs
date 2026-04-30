mod delivery_config;
pub mod generator;
pub mod prompt;
pub mod repository;
pub mod service;
pub mod wiring;

use chrono::{DateTime, NaiveDate, Utc};

pub use delivery_config::DailyReviewDeliveryWorkerConfig;
pub use generator::{ReviewConfig, RigOpenAiReviewGenerator};
pub use prompt::{DailyReviewPrompt, DailyReviewPromptConfig, DailyReviewPromptError};
pub use wiring::{DailyReviewRuntimeConfig, configure_daily_review};

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
    pub delivered_at: Option<DateTime<Utc>>,
    pub delivery_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DailyReviewStatus {
    Completed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DailyReviewResult {
    Existing(DailyReview),
    Generated(DailyReview),
    EmptyDay,
    GenerationFailed(DailyReviewFailure),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DailyReviewFailure {
    pub user_id: String,
    pub review_date: NaiveDate,
    pub model: String,
    pub prompt_version: String,
    pub error_message: String,
}
