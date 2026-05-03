use std::env;

use sqlx::SqlitePool;
use tracing::warn;

use crate::journal::{
    review::{repository::DailyReviewRepository, signals::repository::DailyReviewSignalRepository},
    week_review::{
        generator::{RigOpenAiWeeklyReviewGenerator, WeeklyReviewConfig},
        prompt::WeeklyReviewPromptConfig,
        repository::WeeklyReviewRepository,
        service::{DEFAULT_MIN_DAILY_REVIEWS, WeeklyReviewService},
    },
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WeeklyReviewRuntimeConfig {
    pub openai_api_key: Option<String>,
    pub review: WeeklyReviewConfig,
    pub prompt: WeeklyReviewPromptConfig,
    pub min_daily_reviews: usize,
}

impl WeeklyReviewRuntimeConfig {
    pub fn from_env() -> Self {
        let min_daily_reviews = env::var("FROID_WEEK_REVIEW_MIN_DAILY_REVIEWS")
            .ok()
            .and_then(|value| value.trim().parse::<usize>().ok())
            .unwrap_or(DEFAULT_MIN_DAILY_REVIEWS);

        Self {
            openai_api_key: env::var("OPENAI_API_KEY").ok(),
            review: WeeklyReviewConfig::from_env(),
            prompt: WeeklyReviewPromptConfig::from_env(),
            min_daily_reviews,
        }
    }
}

pub fn build_weekly_review_service(
    pool: SqlitePool,
    config: WeeklyReviewRuntimeConfig,
) -> Result<Option<WeeklyReviewService>, Box<dyn std::error::Error>> {
    let Some(openai_api_key) = config
        .openai_api_key
        .filter(|value| !value.trim().is_empty())
    else {
        warn!("weekly review generation is not configured");
        return Ok(None);
    };

    let weekly_prompt = config.prompt.load()?;
    let weekly_generator = RigOpenAiWeeklyReviewGenerator::from_optional_api_key(
        config.review,
        weekly_prompt,
        Some(openai_api_key),
    )?;

    let service = WeeklyReviewService::new(
        WeeklyReviewRepository::new(pool.clone()),
        DailyReviewRepository::new(pool.clone()),
        DailyReviewSignalRepository::new(pool),
        weekly_generator,
        config.min_daily_reviews,
    );

    Ok(Some(service))
}
