use std::{error::Error, fmt, time::Duration};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DailyReviewSignalWorkerConfigError {
    InvalidBatchSize(String),
    InvalidInterval(String),
}

impl fmt::Display for DailyReviewSignalWorkerConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidBatchSize(value) => write!(
                f,
                "FROID_SIGNAL_WORKER_BATCH_SIZE must be a positive integer, got {value:?}"
            ),
            Self::InvalidInterval(value) => write!(
                f,
                "FROID_SIGNAL_WORKER_INTERVAL_SECONDS must be a positive integer, got {value:?}"
            ),
        }
    }
}

impl Error for DailyReviewSignalWorkerConfigError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DailyReviewSignalWorkerConfig {
    pub enabled: bool,
    pub batch_size: u32,
    pub interval: Duration,
}

impl Default for DailyReviewSignalWorkerConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            batch_size: 20,
            interval: Duration::from_secs(300),
        }
    }
}

impl DailyReviewSignalWorkerConfig {
    pub fn from_values(
        enabled: Option<String>,
        batch_size: Option<String>,
        interval_seconds: Option<String>,
    ) -> Result<Self, DailyReviewSignalWorkerConfigError> {
        let enabled = enabled
            .filter(|v| !v.trim().is_empty())
            .map(|v| v.trim() == "true")
            .unwrap_or(false);

        let batch_size = match batch_size {
            Some(ref value) if !value.trim().is_empty() => {
                let parsed = value.trim().parse::<u32>().map_err(|_| {
                    DailyReviewSignalWorkerConfigError::InvalidBatchSize(value.clone())
                })?;
                if parsed == 0 {
                    return Err(DailyReviewSignalWorkerConfigError::InvalidBatchSize(
                        value.clone(),
                    ));
                }
                parsed
            }
            _ => 20,
        };

        let interval_secs = match interval_seconds {
            Some(ref value) if !value.trim().is_empty() => {
                let parsed = value.trim().parse::<u64>().map_err(|_| {
                    DailyReviewSignalWorkerConfigError::InvalidInterval(value.clone())
                })?;
                if parsed == 0 {
                    return Err(DailyReviewSignalWorkerConfigError::InvalidInterval(
                        value.clone(),
                    ));
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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn uses_default_values_when_not_configured() {
        let config = DailyReviewSignalWorkerConfig::from_values(None, None, None).unwrap();

        assert!(!config.enabled);
        assert_eq!(config.batch_size, 20);
        assert_eq!(config.interval, Duration::from_secs(300));
    }

    #[test]
    fn enables_when_set_to_true() {
        let config =
            DailyReviewSignalWorkerConfig::from_values(Some("true".to_string()), None, None)
                .unwrap();

        assert!(config.enabled);
    }

    #[test]
    fn remains_disabled_when_set_to_false() {
        let config =
            DailyReviewSignalWorkerConfig::from_values(Some("false".to_string()), None, None)
                .unwrap();

        assert!(!config.enabled);
    }

    #[test]
    fn accepts_custom_batch_size_and_interval() {
        let config = DailyReviewSignalWorkerConfig::from_values(
            None,
            Some("50".to_string()),
            Some("60".to_string()),
        )
        .unwrap();

        assert_eq!(config.batch_size, 50);
        assert_eq!(config.interval, Duration::from_secs(60));
    }

    #[test]
    fn rejects_zero_batch_size() {
        let error = DailyReviewSignalWorkerConfig::from_values(None, Some("0".to_string()), None)
            .unwrap_err();

        assert_eq!(
            error,
            DailyReviewSignalWorkerConfigError::InvalidBatchSize("0".to_string())
        );
        assert!(error.to_string().contains("FROID_SIGNAL_WORKER_BATCH_SIZE"));
    }

    #[test]
    fn rejects_invalid_batch_size() {
        let error = DailyReviewSignalWorkerConfig::from_values(None, Some("abc".to_string()), None)
            .unwrap_err();

        assert_eq!(
            error,
            DailyReviewSignalWorkerConfigError::InvalidBatchSize("abc".to_string())
        );
    }

    #[test]
    fn rejects_zero_interval() {
        let error = DailyReviewSignalWorkerConfig::from_values(None, None, Some("0".to_string()))
            .unwrap_err();

        assert_eq!(
            error,
            DailyReviewSignalWorkerConfigError::InvalidInterval("0".to_string())
        );
        assert!(
            error
                .to_string()
                .contains("FROID_SIGNAL_WORKER_INTERVAL_SECONDS")
        );
    }

    #[test]
    fn rejects_invalid_interval() {
        let error = DailyReviewSignalWorkerConfig::from_values(None, None, Some("abc".to_string()))
            .unwrap_err();

        assert_eq!(
            error,
            DailyReviewSignalWorkerConfigError::InvalidInterval("abc".to_string())
        );
    }
}
