use std::env;

use sqlx::SqlitePool;

use crate::journal::{
    extraction::repository::JournalEntryExtractionRepository,
    repository::JournalRepository,
    review::{
        repository::DailyReviewRepository,
        signals::{
            generator::{DailyReviewSignalConfig, RigOpenAiDailyReviewSignalGenerator},
            prompt::DailyReviewSignalPromptConfig,
            repository::{DailyReviewSignalRepository},
            service::DailyReviewSignalService,
        },
    },
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DailyReviewSignalRuntimeConfig {
    pub openai_api_key: Option<String>,
    pub signal: DailyReviewSignalConfig,
    pub prompt: DailyReviewSignalPromptConfig,
}

impl DailyReviewSignalRuntimeConfig {
    pub fn from_env() -> Self {
        Self {
            openai_api_key: env::var("OPENAI_API_KEY").ok(),
            signal: DailyReviewSignalConfig::from_env(),
            prompt: DailyReviewSignalPromptConfig::from_env(),
        }
    }
}

pub fn build_signal_service(
    pool: SqlitePool,
    config: DailyReviewSignalRuntimeConfig,
) -> Result<Option<DailyReviewSignalService>, Box<dyn std::error::Error>> {
    let Some(openai_api_key) = config
        .openai_api_key
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok(None);
    };

    let prompt = config.prompt.load()?;
    let generator = RigOpenAiDailyReviewSignalGenerator::from_optional_api_key(
        config.signal,
        prompt,
        Some(openai_api_key),
    )?;

    let service = DailyReviewSignalService::new(
        DailyReviewRepository::new(pool.clone()),
        JournalRepository::new(pool.clone()),
        JournalEntryExtractionRepository::new(pool.clone()),
        DailyReviewSignalRepository::new(pool.clone()),
        generator,
    );

    Ok(Some(service))
}
