use std::path::PathBuf;

use crate::prompts::file::{self, PromptFile, PromptFileError};

pub const DEFAULT_REVIEW_PROMPT_PATH: &str = "prompts/daily_review_with_entry_extractions_v2.md";

const PROMPT_KIND: &str = "daily review";

pub type DailyReviewPrompt = PromptFile;
pub type DailyReviewPromptError = PromptFileError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DailyReviewPromptConfig {
    pub path: PathBuf,
}

impl Default for DailyReviewPromptConfig {
    fn default() -> Self {
        Self {
            path: PathBuf::from(DEFAULT_REVIEW_PROMPT_PATH),
        }
    }
}

impl DailyReviewPromptConfig {
    pub(crate) fn from_values(path: Option<String>) -> Self {
        Self {
            path: file::resolve_path(path, DEFAULT_REVIEW_PROMPT_PATH),
        }
    }

    pub fn load(&self) -> Result<DailyReviewPrompt, DailyReviewPromptError> {
        file::load(PROMPT_KIND, &self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_config_uses_default_path() {
        let config = DailyReviewPromptConfig::from_values(None);

        assert_eq!(config.path, PathBuf::from(DEFAULT_REVIEW_PROMPT_PATH));
    }

    #[test]
    fn prompt_config_accepts_path_override() {
        let config = DailyReviewPromptConfig::from_values(Some("custom.md".to_string()));

        assert_eq!(config.path, PathBuf::from("custom.md"));
    }
}
