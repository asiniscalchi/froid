use std::{env, error::Error, fmt, fs, path::PathBuf};

pub const DEFAULT_REVIEW_PROMPT_PATH: &str = "prompts/daily_review_with_entry_extractions_v1.md";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DailyReviewPrompt {
    pub version: String,
    pub text: String,
}

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
    pub fn from_env() -> Self {
        Self::from_values(env::var("FROID_REVIEW_PROMPT_PATH").ok())
    }

    pub(crate) fn from_values(path: Option<String>) -> Self {
        Self {
            path: path
                .filter(|value| !value.trim().is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| Self::default().path),
        }
    }

    pub fn load(&self) -> Result<DailyReviewPrompt, DailyReviewPromptError> {
        let text = fs::read_to_string(&self.path).map_err(|source| {
            DailyReviewPromptError::ReadFailed {
                path: self.path.clone(),
                message: source.to_string(),
            }
        })?;

        if text.trim().is_empty() {
            return Err(DailyReviewPromptError::Empty {
                path: self.path.clone(),
            });
        }

        let version = self
            .path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();

        Ok(DailyReviewPrompt { version, text })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DailyReviewPromptError {
    ReadFailed { path: PathBuf, message: String },
    Empty { path: PathBuf },
}

impl fmt::Display for DailyReviewPromptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadFailed { path, message } => {
                write!(
                    f,
                    "failed to load daily review prompt from {}: {message}",
                    path.display()
                )
            }
            Self::Empty { path } => {
                write!(f, "daily review prompt file is empty: {}", path.display())
            }
        }
    }
}

impl Error for DailyReviewPromptError {}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

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

    #[test]
    fn loads_prompt_file_and_derives_version_from_filename() {
        let path = temp_prompt_path("daily-review-load");
        fs::write(&path, "# Prompt\n\nUse only today's entries.").unwrap();

        let prompt = DailyReviewPromptConfig { path: path.clone() }
            .load()
            .unwrap();

        assert_eq!(prompt.version, path.file_stem().unwrap().to_string_lossy());
        assert_eq!(prompt.text, "# Prompt\n\nUse only today's entries.");

        fs::remove_file(path).unwrap();
    }

    #[test]
    fn missing_prompt_file_returns_clear_error() {
        let path = temp_prompt_path("daily-review-missing");

        let error = DailyReviewPromptConfig { path: path.clone() }
            .load()
            .unwrap_err();

        assert!(matches!(error, DailyReviewPromptError::ReadFailed { .. }));
        assert!(error.to_string().contains(path.to_str().unwrap()));
    }

    #[test]
    fn empty_prompt_file_returns_clear_error() {
        let path = temp_prompt_path("daily-review-empty");
        fs::write(&path, "  \n").unwrap();

        let error = DailyReviewPromptConfig { path: path.clone() }
            .load()
            .unwrap_err();

        assert_eq!(error, DailyReviewPromptError::Empty { path: path.clone() });

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
