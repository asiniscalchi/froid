use std::{env, fs, path::PathBuf};

use thiserror::Error;

pub const DEFAULT_JOURNAL_ENTRY_EXTRACTION_PROMPT_PATH: &str = "prompts/entry_extraction_v1.md";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalEntryExtractionPrompt {
    pub version: String,
    pub text: String,
}

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
            path: path
                .filter(|value| !value.trim().is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| Self::default().path),
        }
    }

    pub fn load(&self) -> Result<JournalEntryExtractionPrompt, JournalEntryExtractionPromptError> {
        let text = fs::read_to_string(&self.path).map_err(|source| {
            JournalEntryExtractionPromptError::ReadFailed {
                path: self.path.clone(),
                message: source.to_string(),
            }
        })?;

        if text.trim().is_empty() {
            return Err(JournalEntryExtractionPromptError::Empty {
                path: self.path.clone(),
            });
        }

        let version = self
            .path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();

        Ok(JournalEntryExtractionPrompt { version, text })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum JournalEntryExtractionPromptError {
    #[error("failed to load journal entry extraction prompt from {}: {message}", path.display())]
    ReadFailed { path: PathBuf, message: String },
    #[error("journal entry extraction prompt file is empty: {}", path.display())]
    Empty { path: PathBuf },
}
