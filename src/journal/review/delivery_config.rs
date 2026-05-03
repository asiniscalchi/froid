use std::time::Duration;

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
    pub fn from_values(enabled: Option<bool>, interval_seconds: Option<u64>) -> Self {
        let defaults = Self::default();
        Self {
            enabled: enabled.unwrap_or(defaults.enabled),
            interval: interval_seconds
                .map(Duration::from_secs)
                .unwrap_or(defaults.interval),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uses_default_values_when_not_configured() {
        let config = DailyReviewDeliveryWorkerConfig::from_values(None, None);

        assert!(!config.enabled);
        assert_eq!(config.interval, Duration::from_secs(300));
    }

    #[test]
    fn applies_provided_values() {
        let config = DailyReviewDeliveryWorkerConfig::from_values(Some(true), Some(60));

        assert!(config.enabled);
        assert_eq!(config.interval, Duration::from_secs(60));
    }
}
