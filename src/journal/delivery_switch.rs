use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Shared on/off state for the automated review delivery workers.
///
/// Both the daily and weekly delivery loops consult this switchboard before
/// each cycle; flipping a flag at runtime pauses or resumes delivery without
/// restarting the process. The state is intentionally process-local and not
/// persisted — restarts revert to the initial configuration.
#[derive(Clone, Debug)]
pub struct DeliverySwitchboard {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    daily: AtomicBool,
    weekly: AtomicBool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewKind {
    Daily,
    Weekly,
}

impl DeliverySwitchboard {
    pub fn new(daily_enabled: bool, weekly_enabled: bool) -> Self {
        Self {
            inner: Arc::new(Inner {
                daily: AtomicBool::new(daily_enabled),
                weekly: AtomicBool::new(weekly_enabled),
            }),
        }
    }

    pub fn is_enabled(&self, kind: ReviewKind) -> bool {
        self.flag(kind).load(Ordering::SeqCst)
    }

    /// Set the flag for `kind` to `enabled`. Returns the previous value.
    pub fn set(&self, kind: ReviewKind, enabled: bool) -> bool {
        self.flag(kind).swap(enabled, Ordering::SeqCst)
    }

    fn flag(&self, kind: ReviewKind) -> &AtomicBool {
        match kind {
            ReviewKind::Daily => &self.inner.daily,
            ReviewKind::Weekly => &self.inner.weekly,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_provided_values() {
        let switch = DeliverySwitchboard::new(true, false);
        assert!(switch.is_enabled(ReviewKind::Daily));
        assert!(!switch.is_enabled(ReviewKind::Weekly));
    }

    #[test]
    fn set_returns_previous_value_and_updates_state() {
        let switch = DeliverySwitchboard::new(false, false);
        assert!(!switch.set(ReviewKind::Daily, true));
        assert!(switch.is_enabled(ReviewKind::Daily));
        assert!(switch.set(ReviewKind::Daily, false));
        assert!(!switch.is_enabled(ReviewKind::Daily));
    }

    #[test]
    fn flags_are_independent() {
        let switch = DeliverySwitchboard::new(true, true);
        switch.set(ReviewKind::Daily, false);
        assert!(!switch.is_enabled(ReviewKind::Daily));
        assert!(switch.is_enabled(ReviewKind::Weekly));
    }

    #[test]
    fn clones_share_state() {
        let switch = DeliverySwitchboard::new(false, false);
        let other = switch.clone();
        switch.set(ReviewKind::Weekly, true);
        assert!(other.is_enabled(ReviewKind::Weekly));
    }
}
