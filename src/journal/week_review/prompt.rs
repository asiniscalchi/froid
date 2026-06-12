use std::path::PathBuf;

use crate::prompts::file::{self, PromptFile, PromptFileError};

pub const DEFAULT_WEEK_REVIEW_PROMPT_PATH: &str = "prompts/weekly_review_v1.md";

const PROMPT_KIND: &str = "weekly review";

pub type WeeklyReviewPrompt = PromptFile;
pub type WeeklyReviewPromptError = PromptFileError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WeeklyReviewPromptConfig {
    pub path: PathBuf,
}

impl Default for WeeklyReviewPromptConfig {
    fn default() -> Self {
        Self {
            path: PathBuf::from(DEFAULT_WEEK_REVIEW_PROMPT_PATH),
        }
    }
}

impl WeeklyReviewPromptConfig {
    pub(crate) fn from_values(path: Option<String>) -> Self {
        Self {
            path: file::resolve_path(path, DEFAULT_WEEK_REVIEW_PROMPT_PATH),
        }
    }

    pub fn load(&self) -> Result<WeeklyReviewPrompt, WeeklyReviewPromptError> {
        file::load(PROMPT_KIND, &self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_config_uses_default_path() {
        let config = WeeklyReviewPromptConfig::from_values(None);

        assert_eq!(config.path, PathBuf::from(DEFAULT_WEEK_REVIEW_PROMPT_PATH));
    }

    #[test]
    fn prompt_config_accepts_path_override() {
        let config = WeeklyReviewPromptConfig::from_values(Some("custom.md".to_string()));

        assert_eq!(config.path, PathBuf::from("custom.md"));
    }
}
