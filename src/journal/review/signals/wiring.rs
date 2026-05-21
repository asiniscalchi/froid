use std::env;

use sqlx::SqlitePool;

use crate::{
    journal::{
        extraction::repository::JournalEntryExtractionRepository,
        repository::JournalRepository,
        review::{
            repository::DailyReviewRepository,
            signals::{
                generator::{DailyReviewSignalConfig, RigOpenAiDailyReviewSignalGenerator},
                prompt::DailyReviewSignalPromptConfig,
                repository::DailyReviewSignalRepository,
                service::DailyReviewSignalService,
            },
        },
    },
    prompts::{PromptKey, PromptRepository, PromptSource},
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
    prompt_repository: &PromptRepository,
    config: DailyReviewSignalRuntimeConfig,
) -> Result<Option<DailyReviewSignalService>, Box<dyn std::error::Error>> {
    let Some(openai_api_key) = config
        .openai_api_key
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok(None);
    };

    let prompt = config.prompt.load()?;
    let prompt_source = PromptSource::new(
        prompt_repository.clone(),
        PromptKey::SignalExtraction,
        config.prompt.path.clone(),
    );
    let generator = RigOpenAiDailyReviewSignalGenerator::from_optional_api_key(
        config.signal,
        prompt,
        Some(openai_api_key),
    )?
    .with_prompt_source(prompt_source);

    let service = DailyReviewSignalService::new(
        DailyReviewRepository::new(pool.clone()),
        JournalRepository::new(pool.clone()),
        JournalEntryExtractionRepository::new(pool.clone()),
        DailyReviewSignalRepository::new(pool.clone()),
        generator,
    );

    Ok(Some(service))
}
