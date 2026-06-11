use std::{env, path::PathBuf};

use crate::prompts::file::{self, PromptFile, PromptFileError};

pub const DEFAULT_SIGNAL_EXTRACTION_PROMPT_PATH: &str =
    "prompts/daily_review_signal_extraction_v1.md";

const PROMPT_KIND: &str = "signal extraction";

pub type DailyReviewSignalPrompt = PromptFile;
pub type DailyReviewSignalPromptError = PromptFileError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DailyReviewSignalPromptConfig {
    pub path: PathBuf,
}

impl Default for DailyReviewSignalPromptConfig {
    fn default() -> Self {
        Self {
            path: PathBuf::from(DEFAULT_SIGNAL_EXTRACTION_PROMPT_PATH),
        }
    }
}

impl DailyReviewSignalPromptConfig {
    pub fn from_env() -> Self {
        Self::from_values(env::var("FROID_SIGNAL_EXTRACTION_PROMPT_PATH").ok())
    }

    pub(crate) fn from_values(path: Option<String>) -> Self {
        Self {
            path: file::resolve_path(path, DEFAULT_SIGNAL_EXTRACTION_PROMPT_PATH),
        }
    }

    pub fn load(&self) -> Result<DailyReviewSignalPrompt, DailyReviewSignalPromptError> {
        file::load(PROMPT_KIND, &self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_config_uses_default_path() {
        let config = DailyReviewSignalPromptConfig::from_values(None);

        assert_eq!(
            config.path,
            PathBuf::from(DEFAULT_SIGNAL_EXTRACTION_PROMPT_PATH)
        );
    }

    #[test]
    fn prompt_config_accepts_path_override() {
        let config = DailyReviewSignalPromptConfig::from_values(Some("custom.md".to_string()));

        assert_eq!(config.path, PathBuf::from("custom.md"));
    }
}
