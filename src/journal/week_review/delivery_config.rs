use std::time::Duration;

use chrono::Weekday;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WeeklyReviewDeliveryWorkerConfig {
    pub enabled: bool,
    pub interval: Duration,
    pub kickoff_weekday: Weekday,
    pub min_daily_reviews: usize,
}

impl Default for WeeklyReviewDeliveryWorkerConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            interval: Duration::from_secs(3600),
            kickoff_weekday: Weekday::Mon,
            min_daily_reviews: super::service::DEFAULT_MIN_DAILY_REVIEWS,
        }
    }
}

impl WeeklyReviewDeliveryWorkerConfig {
    pub fn from_values(
        enabled: Option<bool>,
        interval_seconds: Option<u64>,
        kickoff_weekday: Option<Weekday>,
        min_daily_reviews: Option<usize>,
    ) -> Self {
        let defaults = Self::default();
        Self {
            enabled: enabled.unwrap_or(defaults.enabled),
            interval: interval_seconds
                .map(Duration::from_secs)
                .unwrap_or(defaults.interval),
            kickoff_weekday: kickoff_weekday.unwrap_or(defaults.kickoff_weekday),
            min_daily_reviews: min_daily_reviews.unwrap_or(defaults.min_daily_reviews),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uses_default_values_when_not_configured() {
        let config = WeeklyReviewDeliveryWorkerConfig::from_values(None, None, None, None);

        assert!(!config.enabled);
        assert_eq!(config.interval, Duration::from_secs(3600));
        assert_eq!(config.kickoff_weekday, Weekday::Mon);
        assert_eq!(config.min_daily_reviews, 3);
    }

    #[test]
    fn applies_provided_values() {
        let config = WeeklyReviewDeliveryWorkerConfig::from_values(
            Some(true),
            Some(900),
            Some(Weekday::Sun),
            Some(5),
        );

        assert!(config.enabled);
        assert_eq!(config.interval, Duration::from_secs(900));
        assert_eq!(config.kickoff_weekday, Weekday::Sun);
        assert_eq!(config.min_daily_reviews, 5);
    }
}
