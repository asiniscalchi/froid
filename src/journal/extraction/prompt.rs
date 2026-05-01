use std::{env, error::Error, fmt, fs, path::PathBuf};

pub const DEFAULT_ENTRY_EXTRACTION_PROMPT_PATH: &str = "prompts/entry_extraction_v1.md";
pub const DEFAULT_ENTRY_EXTRACTION_PROMPT_VERSION: &str = "entry_extraction_v1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryExtractionPrompt {
    pub version: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryExtractionPromptConfig {
    pub path: PathBuf,
    pub version: String,
}

impl Default for EntryExtractionPromptConfig {
    fn default() -> Self {
        Self {
            path: PathBuf::from(DEFAULT_ENTRY_EXTRACTION_PROMPT_PATH),
            version: DEFAULT_ENTRY_EXTRACTION_PROMPT_VERSION.to_string(),
        }
    }
}

impl EntryExtractionPromptConfig {
    pub fn from_env() -> Self {
        Self::from_values(
            env::var("FROID_ENTRY_EXTRACTION_PROMPT_PATH").ok(),
            env::var("FROID_ENTRY_EXTRACTION_PROMPT_VERSION").ok(),
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

    pub fn load(&self) -> Result<EntryExtractionPrompt, EntryExtractionPromptError> {
        let text = fs::read_to_string(&self.path).map_err(|source| {
            EntryExtractionPromptError::ReadFailed {
                path: self.path.clone(),
                message: source.to_string(),
            }
        })?;

        if text.trim().is_empty() {
            return Err(EntryExtractionPromptError::Empty {
                path: self.path.clone(),
            });
        }

        Ok(EntryExtractionPrompt {
            version: self.version.clone(),
            text,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryExtractionPromptError {
    ReadFailed { path: PathBuf, message: String },
    Empty { path: PathBuf },
}

impl fmt::Display for EntryExtractionPromptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadFailed { path, message } => {
                write!(
                    f,
                    "failed to load journal entry extraction prompt from {}: {message}",
                    path.display()
                )
            }
            Self::Empty { path } => {
                write!(
                    f,
                    "journal entry extraction prompt file is empty: {}",
                    path.display()
                )
            }
        }
    }
}

impl Error for EntryExtractionPromptError {}
