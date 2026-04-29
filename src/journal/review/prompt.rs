use std::{env, error::Error, fmt, fs, path::PathBuf};

pub const DEFAULT_REVIEW_PROMPT_PATH: &str = "prompts/daily_review_v1.md";
pub const DEFAULT_REVIEW_PROMPT_VERSION: &str = "daily-review-v1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DailyReviewPrompt {
    pub version: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DailyReviewPromptConfig {
    pub path: PathBuf,
    pub version: String,
}

impl Default for DailyReviewPromptConfig {
    fn default() -> Self {
        Self {
            path: PathBuf::from(DEFAULT_REVIEW_PROMPT_PATH),
            version: DEFAULT_REVIEW_PROMPT_VERSION.to_string(),
        }
    }
}

impl DailyReviewPromptConfig {
    pub fn from_env() -> Self {
        Self::from_values(
            env::var("FROID_REVIEW_PROMPT_PATH").ok(),
            env::var("FROID_REVIEW_PROMPT_VERSION").ok(),
        )
    }

    pub(crate) fn from_values(path: Option<String>, version: Option<String>) -> Self {
        let defaults = Self::default();
        Self {
            path: path
                .filter(|value| !value.trim().is_empty())
                .map(PathBuf::from)
                .unwrap_or(defaults.path),
            version: version
                .filter(|value| !value.trim().is_empty())
                .unwrap_or(defaults.version),
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

        Ok(DailyReviewPrompt {
            version: self.version.clone(),
            text,
        })
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
    fn prompt_config_uses_defaults() {
        let config = DailyReviewPromptConfig::from_values(None, None);

        assert_eq!(config.path, PathBuf::from(DEFAULT_REVIEW_PROMPT_PATH));
        assert_eq!(config.version, DEFAULT_REVIEW_PROMPT_VERSION);
    }

    #[test]
    fn prompt_config_accepts_overrides() {
        let config = DailyReviewPromptConfig::from_values(
            Some("custom.md".to_string()),
            Some("custom-version".to_string()),
        );

        assert_eq!(config.path, PathBuf::from("custom.md"));
        assert_eq!(config.version, "custom-version");
    }

    #[test]
    fn loads_prompt_file() {
        let path = temp_prompt_path("daily-review-load");
        fs::write(&path, "# Prompt\n\nUse only today's entries.").unwrap();

        let prompt = DailyReviewPromptConfig {
            path: path.clone(),
            version: "v1".to_string(),
        }
        .load()
        .unwrap();

        assert_eq!(prompt.version, "v1");
        assert_eq!(prompt.text, "# Prompt\n\nUse only today's entries.");

        fs::remove_file(path).unwrap();
    }

    #[test]
    fn missing_prompt_file_returns_clear_error() {
        let path = temp_prompt_path("daily-review-missing");

        let error = DailyReviewPromptConfig {
            path: path.clone(),
            version: "v1".to_string(),
        }
        .load()
        .unwrap_err();

        assert!(matches!(error, DailyReviewPromptError::ReadFailed { .. }));
        assert!(error.to_string().contains(path.to_str().unwrap()));
    }

    #[test]
    fn empty_prompt_file_returns_clear_error() {
        let path = temp_prompt_path("daily-review-empty");
        fs::write(&path, "  \n").unwrap();

        let error = DailyReviewPromptConfig {
            path: path.clone(),
            version: "v1".to_string(),
        }
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
