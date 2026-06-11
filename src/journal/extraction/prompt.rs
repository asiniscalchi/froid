use std::{env, path::PathBuf};

use crate::prompts::file::{self, PromptFile, PromptFileError};

pub const DEFAULT_JOURNAL_ENTRY_EXTRACTION_PROMPT_PATH: &str = "prompts/entry_extraction_v1.md";

const PROMPT_KIND: &str = "journal entry extraction";

pub type JournalEntryExtractionPrompt = PromptFile;
pub type JournalEntryExtractionPromptError = PromptFileError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalEntryExtractionPromptConfig {
    pub path: PathBuf,
}

impl Default for JournalEntryExtractionPromptConfig {
    fn default() -> Self {
        Self {
            path: PathBuf::from(DEFAULT_JOURNAL_ENTRY_EXTRACTION_PROMPT_PATH),
        }
    }
}

impl JournalEntryExtractionPromptConfig {
    pub fn from_env() -> Self {
        Self::from_values(env::var("FROID_ENTRY_EXTRACTION_PROMPT_PATH").ok())
    }

    pub(crate) fn from_values(path: Option<String>) -> Self {
        Self {
            path: file::resolve_path(path, DEFAULT_JOURNAL_ENTRY_EXTRACTION_PROMPT_PATH),
        }
    }

    pub fn load(&self) -> Result<JournalEntryExtractionPrompt, JournalEntryExtractionPromptError> {
        file::load(PROMPT_KIND, &self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_config_uses_default_path() {
        let config = JournalEntryExtractionPromptConfig::from_values(None);

        assert_eq!(
            config.path,
            PathBuf::from(DEFAULT_JOURNAL_ENTRY_EXTRACTION_PROMPT_PATH)
        );
    }

    #[test]
    fn prompt_config_accepts_path_override() {
        let config = JournalEntryExtractionPromptConfig::from_values(Some("custom.md".to_string()));

        assert_eq!(config.path, PathBuf::from("custom.md"));
    }
}
