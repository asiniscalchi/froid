use std::{env, error::Error, fmt, fs, path::PathBuf};

pub const DEFAULT_SIGNAL_EXTRACTION_PROMPT_PATH: &str =
    "prompts/daily_review_signal_extraction_v1.md";
pub const DEFAULT_SIGNAL_EXTRACTION_PROMPT_VERSION: &str = "signal-extraction-v1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DailyReviewSignalPrompt {
    pub version: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DailyReviewSignalPromptConfig {
    pub path: PathBuf,
    pub version: String,
}

impl Default for DailyReviewSignalPromptConfig {
    fn default() -> Self {
        Self {
            path: PathBuf::from(DEFAULT_SIGNAL_EXTRACTION_PROMPT_PATH),
            version: DEFAULT_SIGNAL_EXTRACTION_PROMPT_VERSION.to_string(),
        }
    }
}

impl DailyReviewSignalPromptConfig {
    pub fn from_env() -> Self {
        Self::from_values(
            env::var("FROID_SIGNAL_EXTRACTION_PROMPT_PATH").ok(),
            env::var("FROID_SIGNAL_EXTRACTION_PROMPT_VERSION").ok(),
        )
    }

    pub(crate) fn from_values(path: Option<String>, version: Option<String>) -> Self {
        let defaults = Self::default();
        Self {
            path: path
                .filter(|v| !v.trim().is_empty())
                .map(PathBuf::from)
                .unwrap_or(defaults.path),
            version: version
                .filter(|v| !v.trim().is_empty())
                .unwrap_or(defaults.version),
        }
    }

    pub fn load(&self) -> Result<DailyReviewSignalPrompt, DailyReviewSignalPromptError> {
        let text = fs::read_to_string(&self.path).map_err(|source| {
            DailyReviewSignalPromptError::ReadFailed {
                path: self.path.clone(),
                message: source.to_string(),
            }
        })?;

        if text.trim().is_empty() {
            return Err(DailyReviewSignalPromptError::Empty {
                path: self.path.clone(),
            });
        }

        Ok(DailyReviewSignalPrompt {
            version: self.version.clone(),
            text,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DailyReviewSignalPromptError {
    ReadFailed { path: PathBuf, message: String },
    Empty { path: PathBuf },
}

impl fmt::Display for DailyReviewSignalPromptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadFailed { path, message } => write!(
                f,
                "failed to load signal extraction prompt from {}: {message}",
                path.display()
            ),
            Self::Empty { path } => write!(
                f,
                "signal extraction prompt file is empty: {}",
                path.display()
            ),
        }
    }
}

impl Error for DailyReviewSignalPromptError {}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "froid-signal-prompt-{name}-{}.md",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn prompt_config_uses_defaults() {
        let config = DailyReviewSignalPromptConfig::from_values(None, None);

        assert_eq!(
            config.path,
            PathBuf::from(DEFAULT_SIGNAL_EXTRACTION_PROMPT_PATH)
        );
        assert_eq!(config.version, DEFAULT_SIGNAL_EXTRACTION_PROMPT_VERSION);
    }

    #[test]
    fn prompt_config_accepts_overrides() {
        let config = DailyReviewSignalPromptConfig::from_values(
            Some("custom.md".to_string()),
            Some("custom-v2".to_string()),
        );

        assert_eq!(config.path, PathBuf::from("custom.md"));
        assert_eq!(config.version, "custom-v2");
    }

    #[test]
    fn loads_prompt_file() {
        let path = temp_path("load");
        fs::write(&path, "Extract signals.").unwrap();

        let prompt = DailyReviewSignalPromptConfig {
            path: path.clone(),
            version: "v1".to_string(),
        }
        .load()
        .unwrap();

        assert_eq!(prompt.version, "v1");
        assert_eq!(prompt.text, "Extract signals.");

        fs::remove_file(path).unwrap();
    }

    #[test]
    fn missing_prompt_file_returns_error() {
        let path = temp_path("missing");

        let error = DailyReviewSignalPromptConfig {
            path: path.clone(),
            version: "v1".to_string(),
        }
        .load()
        .unwrap_err();

        assert!(matches!(
            error,
            DailyReviewSignalPromptError::ReadFailed { .. }
        ));
        assert!(error.to_string().contains(path.to_str().unwrap()));
    }

    #[test]
    fn empty_prompt_file_returns_error() {
        let path = temp_path("empty");
        fs::write(&path, "   \n").unwrap();

        let error = DailyReviewSignalPromptConfig {
            path: path.clone(),
            version: "v1".to_string(),
        }
        .load()
        .unwrap_err();

        assert_eq!(
            error,
            DailyReviewSignalPromptError::Empty { path: path.clone() }
        );

        fs::remove_file(path).unwrap();
    }
}
