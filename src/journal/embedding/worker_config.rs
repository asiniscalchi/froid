use std::{env, error::Error, fmt, time::Duration};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmbeddingWorkerConfigError {
    InvalidBatchSize(String),
    InvalidInterval(String),
}

impl fmt::Display for EmbeddingWorkerConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidBatchSize(value) => write!(
                f,
                "FROID_EMBEDDING_WORKER_BATCH_SIZE must be a positive integer, got {value:?}"
            ),
            Self::InvalidInterval(value) => write!(
                f,
                "FROID_EMBEDDING_WORKER_INTERVAL_SECONDS must be a positive integer, got {value:?}"
            ),
        }
    }
}

impl Error for EmbeddingWorkerConfigError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingWorkerConfig {
    pub enabled: bool,
    pub batch_size: u32,
    pub interval: Duration,
}

impl Default for EmbeddingWorkerConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            batch_size: 20,
            interval: Duration::from_secs(300),
        }
    }
}

impl EmbeddingWorkerConfig {
    #[allow(dead_code)]
    pub fn from_env() -> Result<Self, EmbeddingWorkerConfigError> {
        Self::from_values(
            env::var("FROID_EMBEDDING_WORKER_ENABLED").ok(),
            env::var("FROID_EMBEDDING_WORKER_BATCH_SIZE").ok(),
            env::var("FROID_EMBEDDING_WORKER_INTERVAL_SECONDS").ok(),
        )
    }

    pub fn from_values(
        enabled: Option<String>,
        batch_size: Option<String>,
        interval_seconds: Option<String>,
    ) -> Result<Self, EmbeddingWorkerConfigError> {
        let enabled = enabled
            .filter(|v| !v.trim().is_empty())
            .map(|v| v.trim() == "true")
            .unwrap_or(false);

        let batch_size = match batch_size {
            Some(ref value) if !value.trim().is_empty() => {
                let parsed = value
                    .trim()
                    .parse::<u32>()
                    .map_err(|_| EmbeddingWorkerConfigError::InvalidBatchSize(value.clone()))?;
                if parsed == 0 {
                    return Err(EmbeddingWorkerConfigError::InvalidBatchSize(value.clone()));
                }
                parsed
            }
            _ => 20,
        };

        let interval_secs = match interval_seconds {
            Some(ref value) if !value.trim().is_empty() => {
                let parsed = value
                    .trim()
                    .parse::<u64>()
                    .map_err(|_| EmbeddingWorkerConfigError::InvalidInterval(value.clone()))?;
                if parsed == 0 {
                    return Err(EmbeddingWorkerConfigError::InvalidInterval(value.clone()));
                }
                parsed
            }
            _ => 300,
        };

        Ok(Self {
            enabled,
            batch_size,
            interval: Duration::from_secs(interval_secs),
        })
    }
}
