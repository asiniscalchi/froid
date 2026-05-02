pub(crate) mod backfill;
pub(crate) mod generator;
pub(crate) mod prompt;
pub mod repository;
pub(crate) mod service;
pub(crate) mod types;
pub(crate) mod validation;
pub(crate) mod wiring;

use chrono::{DateTime, Utc};

pub use backfill::{ExtractionBackfillError, ExtractionBackfillResult, ExtractionBackfillService};
pub use generator::{
    JournalEntryExtractionConfig, JournalEntryExtractionGenerationError,
    JournalEntryExtractionGenerator, RigOpenAiJournalEntryExtractionGenerator,
};
pub use prompt::{JournalEntryExtractionPrompt, JournalEntryExtractionPromptConfig};
pub use types::{BehaviorValence, JournalEntryExtractionResult, NeedStatus, PatternExtraction};
pub use wiring::{JournalEntryExtractionRuntimeConfig, configure_journal_entry_extraction};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalEntryExtractionCandidate {
    pub journal_entry_id: i64,
    pub raw_text: String,
}

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
