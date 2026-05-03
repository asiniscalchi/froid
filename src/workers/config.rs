use std::time::Duration;

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
        enabled: Option<bool>,
        batch_size: Option<u32>,
        interval_seconds: Option<u64>,
    ) -> Self {
        let defaults = Self::default();
        Self {
            enabled: enabled.unwrap_or(defaults.enabled),
            batch_size: batch_size.unwrap_or(defaults.batch_size),
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
        let config = ReconciliationWorkerConfig::from_values(None, None, None);

        assert!(!config.enabled);
        assert_eq!(config.batch_size, 20);
        assert_eq!(config.interval, Duration::from_secs(300));
    }

    #[test]
    fn applies_provided_values() {
        let config = ReconciliationWorkerConfig::from_values(Some(true), Some(50), Some(60));

        assert!(config.enabled);
        assert_eq!(config.batch_size, 50);
        assert_eq!(config.interval, Duration::from_secs(60));
    }
}
