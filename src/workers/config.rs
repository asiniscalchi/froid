use std::{error::Error, fmt, time::Duration};

#[derive(Debug, Clone, Copy)]
pub struct WorkerEnvLabels {
    pub batch_size: &'static str,
    pub interval_seconds: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReconciliationWorkerConfigError {
    InvalidBatchSize { label: &'static str, value: String },
    InvalidInterval { label: &'static str, value: String },
}

impl fmt::Display for ReconciliationWorkerConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidBatchSize { label, value } => {
                write!(f, "{label} must be a positive integer, got {value:?}",)
            }
            Self::InvalidInterval { label, value } => {
                write!(f, "{label} must be a positive integer, got {value:?}",)
            }
        }
    }
}

impl Error for ReconciliationWorkerConfigError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconciliationWorkerConfig {
    pub enabled: bool,
    pub batch_size: u32,
    pub interval: Duration,
}

impl Default for ReconciliationWorkerConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            batch_size: 20,
            interval: Duration::from_secs(300),
        }
    }
}

impl ReconciliationWorkerConfig {
    pub fn from_values(
        labels: WorkerEnvLabels,
        enabled: Option<String>,
        batch_size: Option<String>,
        interval_seconds: Option<String>,
    ) -> Result<Self, ReconciliationWorkerConfigError> {
        let enabled = enabled
            .filter(|v| !v.trim().is_empty())
            .map(|v| v.trim() == "true")
            .unwrap_or(false);

        let batch_size = parse_positive_u32(batch_size, labels.batch_size, |label, value| {
            ReconciliationWorkerConfigError::InvalidBatchSize { label, value }
        })?
        .unwrap_or(20);

        let interval_secs =
            parse_positive_u64(interval_seconds, labels.interval_seconds, |label, value| {
                ReconciliationWorkerConfigError::InvalidInterval { label, value }
            })?
            .unwrap_or(300);

        Ok(Self {
            enabled,
            batch_size,
            interval: Duration::from_secs(interval_secs),
        })
    }
}

fn parse_positive_u32(
    value: Option<String>,
    label: &'static str,
    error: impl Fn(&'static str, String) -> ReconciliationWorkerConfigError,
) -> Result<Option<u32>, ReconciliationWorkerConfigError> {
    let Some(raw) = value.filter(|v| !v.trim().is_empty()) else {
        return Ok(None);
    };
    let parsed = raw
        .trim()
        .parse::<u32>()
        .map_err(|_| error(label, raw.clone()))?;
    if parsed == 0 {
        return Err(error(label, raw));
    }
    Ok(Some(parsed))
}

fn parse_positive_u64(
    value: Option<String>,
    label: &'static str,
    error: impl Fn(&'static str, String) -> ReconciliationWorkerConfigError,
) -> Result<Option<u64>, ReconciliationWorkerConfigError> {
    let Some(raw) = value.filter(|v| !v.trim().is_empty()) else {
        return Ok(None);
    };
    let parsed = raw
        .trim()
        .parse::<u64>()
        .map_err(|_| error(label, raw.clone()))?;
    if parsed == 0 {
        return Err(error(label, raw));
    }
    Ok(Some(parsed))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_LABELS: WorkerEnvLabels = WorkerEnvLabels {
        batch_size: "FROID_TEST_BATCH_SIZE",
        interval_seconds: "FROID_TEST_INTERVAL_SECONDS",
    };

    #[test]
    fn uses_default_values_when_not_configured() {
        let config =
            ReconciliationWorkerConfig::from_values(TEST_LABELS, None, None, None).unwrap();

        assert!(!config.enabled);
        assert_eq!(config.batch_size, 20);
        assert_eq!(config.interval, Duration::from_secs(300));
    }

    #[test]
    fn enables_when_set_to_true() {
        let config =
            ReconciliationWorkerConfig::from_values(TEST_LABELS, Some("true".into()), None, None)
                .unwrap();

        assert!(config.enabled);
    }

    #[test]
    fn remains_disabled_when_set_to_false_or_other() {
        for value in ["false", "yes", "1", "  "] {
            let config = ReconciliationWorkerConfig::from_values(
                TEST_LABELS,
                Some(value.to_string()),
                None,
                None,
            )
            .unwrap();
            assert!(!config.enabled, "expected disabled for value {value:?}");
        }
    }

    #[test]
    fn accepts_custom_batch_size_and_interval() {
        let config = ReconciliationWorkerConfig::from_values(
            TEST_LABELS,
            None,
            Some("50".into()),
            Some("60".into()),
        )
        .unwrap();

        assert_eq!(config.batch_size, 50);
        assert_eq!(config.interval, Duration::from_secs(60));
    }

    #[test]
    fn rejects_zero_batch_size_with_label() {
        let error =
            ReconciliationWorkerConfig::from_values(TEST_LABELS, None, Some("0".into()), None)
                .unwrap_err();

        assert_eq!(
            error,
            ReconciliationWorkerConfigError::InvalidBatchSize {
                label: "FROID_TEST_BATCH_SIZE",
                value: "0".to_string(),
            }
        );
        assert!(error.to_string().contains("FROID_TEST_BATCH_SIZE"));
    }

    #[test]
    fn rejects_invalid_batch_size_with_label() {
        let error =
            ReconciliationWorkerConfig::from_values(TEST_LABELS, None, Some("abc".into()), None)
                .unwrap_err();

        assert_eq!(
            error,
            ReconciliationWorkerConfigError::InvalidBatchSize {
                label: "FROID_TEST_BATCH_SIZE",
                value: "abc".to_string(),
            }
        );
    }

    #[test]
    fn rejects_zero_interval_with_label() {
        let error =
            ReconciliationWorkerConfig::from_values(TEST_LABELS, None, None, Some("0".into()))
                .unwrap_err();

        assert_eq!(
            error,
            ReconciliationWorkerConfigError::InvalidInterval {
                label: "FROID_TEST_INTERVAL_SECONDS",
                value: "0".to_string(),
            }
        );
        assert!(error.to_string().contains("FROID_TEST_INTERVAL_SECONDS"));
    }

    #[test]
    fn rejects_invalid_interval_with_label() {
        let error =
            ReconciliationWorkerConfig::from_values(TEST_LABELS, None, None, Some("abc".into()))
                .unwrap_err();

        assert_eq!(
            error,
            ReconciliationWorkerConfigError::InvalidInterval {
                label: "FROID_TEST_INTERVAL_SECONDS",
                value: "abc".to_string(),
            }
        );
    }
}
