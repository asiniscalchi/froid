use std::env;

use sqlx::SqlitePool;
use tracing::warn;

use crate::journal::{
    extraction::{
        JournalEntryExtractionConfig, JournalEntryExtractionPromptConfig,
        RigOpenAiJournalEntryExtractionGenerator, repository::JournalEntryExtractionRepository,
        service::JournalEntryExtractionService,
    },
    service::JournalService,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalEntryExtractionRuntimeConfig {
    pub openai_api_key: Option<String>,
    pub extraction: JournalEntryExtractionConfig,
    pub prompt: JournalEntryExtractionPromptConfig,
}

impl JournalEntryExtractionRuntimeConfig {
    pub fn from_env() -> Self {
        Self {
            openai_api_key: env::var("OPENAI_API_KEY").ok(),
            extraction: JournalEntryExtractionConfig::from_env(),
            prompt: JournalEntryExtractionPromptConfig::from_env(),
        }
    }
}

pub fn configure_journal_entry_extraction(
    journal_service: JournalService,
    pool: SqlitePool,
    config: JournalEntryExtractionRuntimeConfig,
) -> Result<JournalService, Box<dyn std::error::Error>> {
    let Some(openai_api_key) = config
        .openai_api_key
        .filter(|value| !value.trim().is_empty())
    else {
        warn!("journal entry extraction is not configured");
        return Ok(journal_service);
    };

    let prompt = config.prompt.load()?;
    let generator = RigOpenAiJournalEntryExtractionGenerator::from_optional_api_key(
        config.extraction,
        prompt,
        Some(openai_api_key),
    )?;
    let service =
        JournalEntryExtractionService::new(JournalEntryExtractionRepository::new(pool), generator);

    Ok(journal_service.with_entry_extraction_runner(service))
}
