use std::{env, error::Error, fmt};

use super::{DEFAULT_EMBEDDING_MODEL, SUPPORTED_EMBEDDING_DIMENSIONS};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingConfig {
    pub model: String,
    pub dimensions: usize,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            model: DEFAULT_EMBEDDING_MODEL.to_string(),
            dimensions: SUPPORTED_EMBEDDING_DIMENSIONS,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmbeddingConfigError {
    InvalidDimensions(String),
    UnsupportedDimensions { configured: usize, supported: usize },
}

impl fmt::Display for EmbeddingConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDimensions(value) => {
                write!(
                    f,
                    "FROID_EMBEDDING_DIMENSIONS must be a positive integer, got {value:?}"
                )
            }
            Self::UnsupportedDimensions {
                configured,
                supported,
            } => write!(
                f,
                "FROID_EMBEDDING_DIMENSIONS={configured} is not supported; this build supports only {supported}"
            ),
        }
    }
}

impl Error for EmbeddingConfigError {}

impl EmbeddingConfig {
    pub fn from_env() -> Result<Self, EmbeddingConfigError> {
        Self::from_values(
            env::var("FROID_EMBEDDING_MODEL").ok(),
            env::var("FROID_EMBEDDING_DIMENSIONS").ok(),
        )
    }

    pub(crate) fn from_values(
        model: Option<String>,
        dimensions: Option<String>,
    ) -> Result<Self, EmbeddingConfigError> {
        let model = model
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_EMBEDDING_MODEL.to_string());
        let dimensions = match dimensions {
            Some(value) if !value.trim().is_empty() => value
                .parse::<usize>()
                .map_err(|_| EmbeddingConfigError::InvalidDimensions(value))?,
            _ => SUPPORTED_EMBEDDING_DIMENSIONS,
        };

        if dimensions != SUPPORTED_EMBEDDING_DIMENSIONS {
            return Err(EmbeddingConfigError::UnsupportedDimensions {
                configured: dimensions,
                supported: SUPPORTED_EMBEDDING_DIMENSIONS,
            });
        }

        Ok(Self { model, dimensions })
    }
}
