use std::{error::Error, fmt, time::Duration};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DailyReviewDeliveryWorkerConfigError {
    InvalidInterval(String),
}

impl fmt::Display for DailyReviewDeliveryWorkerConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInterval(value) => write!(
                f,
                "FROID_DAILY_REVIEW_DELIVERY_INTERVAL_SECONDS must be a positive integer, got {value:?}"
            ),
        }
    }
}

impl Error for DailyReviewDeliveryWorkerConfigError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DailyReviewDeliveryWorkerConfig {
    pub enabled: bool,
    pub interval: Duration,
}

impl Default for DailyReviewDeliveryWorkerConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            interval: Duration::from_secs(300),
        }
    }
}

impl DailyReviewDeliveryWorkerConfig {
    pub fn from_values(
        enabled: Option<String>,
        interval_seconds: Option<String>,
    ) -> Result<Self, DailyReviewDeliveryWorkerConfigError> {
        let enabled = enabled
            .filter(|v| !v.trim().is_empty())
            .map(|v| v.trim() == "true")
            .unwrap_or(false);

        let interval_secs = match interval_seconds {
            Some(ref value) if !value.trim().is_empty() => {
                let parsed = value.trim().parse::<u64>().map_err(|_| {
                    DailyReviewDeliveryWorkerConfigError::InvalidInterval(value.clone())
                })?;
                if parsed == 0 {
                    return Err(DailyReviewDeliveryWorkerConfigError::InvalidInterval(
                        value.clone(),
                    ));
                }
                parsed
            }
            _ => 300,
        };

        Ok(Self {
            enabled,
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
        let config = DailyReviewDeliveryWorkerConfig::from_values(None, None).unwrap();

        assert!(!config.enabled);
        assert_eq!(config.interval, Duration::from_secs(300));
    }

    #[test]
    fn enables_when_set_to_true() {
        let config =
            DailyReviewDeliveryWorkerConfig::from_values(Some("true".to_string()), None).unwrap();

        assert!(config.enabled);
    }

    #[test]
    fn remains_disabled_when_set_to_false() {
        let config =
            DailyReviewDeliveryWorkerConfig::from_values(Some("false".to_string()), None).unwrap();

        assert!(!config.enabled);
    }

    #[test]
    fn accepts_custom_interval() {
        let config =
            DailyReviewDeliveryWorkerConfig::from_values(None, Some("60".to_string())).unwrap();

        assert_eq!(config.interval, Duration::from_secs(60));
    }

    #[test]
    fn rejects_zero_interval() {
        let error =
            DailyReviewDeliveryWorkerConfig::from_values(None, Some("0".to_string())).unwrap_err();

        assert_eq!(
            error,
            DailyReviewDeliveryWorkerConfigError::InvalidInterval("0".to_string())
        );
        assert!(
            error
                .to_string()
                .contains("FROID_DAILY_REVIEW_DELIVERY_INTERVAL_SECONDS")
        );
    }

    #[test]
    fn rejects_invalid_interval() {
        let error = DailyReviewDeliveryWorkerConfig::from_values(None, Some("abc".to_string()))
            .unwrap_err();

        assert_eq!(
            error,
            DailyReviewDeliveryWorkerConfigError::InvalidInterval("abc".to_string())
        );
    }
}
