use std::env;

use sqlx::SqlitePool;
use tracing::warn;

use crate::journal::{
    extraction::{
        EntryExtractionConfig, EntryExtractionPromptConfig, RigOpenAiEntryExtractionGenerator,
        repository::JournalEntryExtractionRepository, service::EntryExtractionService,
    },
    service::JournalService,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryExtractionRuntimeConfig {
    pub openai_api_key: Option<String>,
    pub extraction: EntryExtractionConfig,
    pub prompt: EntryExtractionPromptConfig,
}

impl EntryExtractionRuntimeConfig {
    pub fn from_env() -> Self {
        Self {
            openai_api_key: env::var("OPENAI_API_KEY").ok(),
            extraction: EntryExtractionConfig::from_env(),
            prompt: EntryExtractionPromptConfig::from_env(),
        }
    }
}

pub fn configure_entry_extraction(
    journal_service: JournalService,
    pool: SqlitePool,
    config: EntryExtractionRuntimeConfig,
) -> Result<JournalService, Box<dyn std::error::Error>> {
    let Some(openai_api_key) = config
        .openai_api_key
        .filter(|value| !value.trim().is_empty())
    else {
        warn!("journal entry extraction is not configured");
        return Ok(journal_service);
    };

    let prompt = config.prompt.load()?;
    let generator = RigOpenAiEntryExtractionGenerator::from_optional_api_key(
        config.extraction,
        prompt,
        Some(openai_api_key),
    )?;
    let service =
        EntryExtractionService::new(JournalEntryExtractionRepository::new(pool), generator);

    Ok(journal_service.with_entry_extraction_runner(service))
}
