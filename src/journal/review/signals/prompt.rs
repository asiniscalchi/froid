use std::{env, error::Error, fmt, fs, path::PathBuf};

pub const DEFAULT_SIGNAL_EXTRACTION_PROMPT_PATH: &str =
    "prompts/daily_review_signal_extraction_v1.md";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DailyReviewSignalPrompt {
    pub version: String,
    pub text: String,
}

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
            path: path
                .filter(|v| !v.trim().is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| Self::default().path),
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

        let version = self
            .path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();

        Ok(DailyReviewSignalPrompt { version, text })
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

    #[test]
    fn loads_prompt_file_and_derives_version_from_filename() {
        let path = temp_path("load");
        fs::write(&path, "Extract signals.").unwrap();

        let prompt = DailyReviewSignalPromptConfig { path: path.clone() }
            .load()
            .unwrap();

        assert_eq!(prompt.version, path.file_stem().unwrap().to_string_lossy());
        assert_eq!(prompt.text, "Extract signals.");

        fs::remove_file(path).unwrap();
    }

    #[test]
    fn missing_prompt_file_returns_error() {
        let path = temp_path("missing");

        let error = DailyReviewSignalPromptConfig { path: path.clone() }
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

        let error = DailyReviewSignalPromptConfig { path: path.clone() }
            .load()
            .unwrap_err();

        assert_eq!(
            error,
            DailyReviewSignalPromptError::Empty { path: path.clone() }
        );

        fs::remove_file(path).unwrap();
    }
}
