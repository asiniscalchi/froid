use std::{env, fs, path::PathBuf};

use thiserror::Error;

pub const DEFAULT_WEEK_REVIEW_PROMPT_PATH: &str = "prompts/weekly_review_v1.md";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WeeklyReviewPrompt {
    pub version: String,
    pub text: String,
}

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
    pub fn from_env() -> Self {
        Self::from_values(env::var("FROID_WEEK_REVIEW_PROMPT_PATH").ok())
    }

    pub(crate) fn from_values(path: Option<String>) -> Self {
        Self {
            path: path
                .filter(|value| !value.trim().is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| Self::default().path),
        }
    }

    pub fn load(&self) -> Result<WeeklyReviewPrompt, WeeklyReviewPromptError> {
        let text = fs::read_to_string(&self.path).map_err(|source| {
            WeeklyReviewPromptError::ReadFailed {
                path: self.path.clone(),
                message: source.to_string(),
            }
        })?;

        if text.trim().is_empty() {
            return Err(WeeklyReviewPromptError::Empty {
                path: self.path.clone(),
            });
        }

        let version = self
            .path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();

        Ok(WeeklyReviewPrompt { version, text })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum WeeklyReviewPromptError {
    #[error("failed to load weekly review prompt from {}: {message}", path.display())]
    ReadFailed { path: PathBuf, message: String },
    #[error("weekly review prompt file is empty: {}", path.display())]
    Empty { path: PathBuf },
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

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

    #[test]
    fn loads_prompt_file_and_derives_version_from_filename() {
        let path = temp_prompt_path("weekly-review-load");
        fs::write(&path, "# Prompt\n\nSynthesize the week.").unwrap();

        let prompt = WeeklyReviewPromptConfig { path: path.clone() }
            .load()
            .unwrap();

        assert_eq!(prompt.version, path.file_stem().unwrap().to_string_lossy());
        assert_eq!(prompt.text, "# Prompt\n\nSynthesize the week.");

        fs::remove_file(path).unwrap();
    }

    #[test]
    fn missing_prompt_file_returns_clear_error() {
        let path = temp_prompt_path("weekly-review-missing");

        let error = WeeklyReviewPromptConfig { path: path.clone() }
            .load()
            .unwrap_err();

        assert!(matches!(error, WeeklyReviewPromptError::ReadFailed { .. }));
        assert!(error.to_string().contains(path.to_str().unwrap()));
    }

    #[test]
    fn empty_prompt_file_returns_clear_error() {
        let path = temp_prompt_path("weekly-review-empty");
        fs::write(&path, "  \n").unwrap();

        let error = WeeklyReviewPromptConfig { path: path.clone() }
            .load()
            .unwrap_err();

        assert_eq!(error, WeeklyReviewPromptError::Empty { path: path.clone() });

        fs::remove_file(path).unwrap();
    }

    fn temp_prompt_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "froid-{name}-{}.md",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }
}
