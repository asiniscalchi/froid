pub mod generator;
pub mod prompt;
pub mod repository;
pub mod service;
pub mod validation;
pub mod wiring;

use chrono::{DateTime, Utc};

pub use generator::{
    EntryExtractionConfig, EntryExtractionGenerationError, EntryExtractionGenerator,
    RigOpenAiEntryExtractionGenerator,
};
pub use prompt::{EntryExtractionPrompt, EntryExtractionPromptConfig};
pub use wiring::{EntryExtractionRuntimeConfig, configure_entry_extraction};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JournalEntryExtractionStatus {
    Pending,
    Completed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalEntryExtraction {
    pub id: i64,
    pub journal_entry_id: i64,
    pub extraction_json: Option<String>,
    pub model: String,
    pub prompt_version: String,
    pub status: JournalEntryExtractionStatus,
    pub error_message: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
