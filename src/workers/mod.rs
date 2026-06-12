pub mod config;
pub mod daily_review;
pub mod embedding;
pub mod extraction;
pub mod reconciliation;
pub mod review_delivery;
pub mod signals;
pub mod telegram;
pub mod tenant_sweep;
pub mod weekly_review;

pub use config::ReconciliationWorkerConfig;
pub use reconciliation::{ReconciliationCycle, ReconciliationWorker};
pub use telegram::{ReviewSendOutcome, TelegramReviewSender};
pub use tenant_sweep::TenantSweepCycle;

/// Shared test doubles for the delivery workers.
#[cfg(test)]
pub(crate) mod test_support {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;

    use super::{ReviewSendOutcome, review_delivery::ReviewSender};

    /// Sender that records every delivery and returns one fixed outcome.
    #[derive(Debug, Clone)]
    pub(crate) struct FakeSender {
        sent: Arc<Mutex<Vec<(String, String)>>>,
        result: Result<ReviewSendOutcome, String>,
    }

    impl FakeSender {
        pub(crate) fn succeeding() -> Self {
            Self::returning(Ok(ReviewSendOutcome::Sent))
        }

        pub(crate) fn skipped() -> Self {
            Self::returning(Ok(ReviewSendOutcome::Skipped))
        }

        pub(crate) fn failing(error: &str) -> Self {
            Self::returning(Err(error.to_string()))
        }

        fn returning(result: Result<ReviewSendOutcome, String>) -> Self {
            Self {
                sent: Arc::new(Mutex::new(Vec::new())),
                result,
            }
        }

        /// `(source_conversation_id, text)` pairs in send order.
        pub(crate) fn sent(&self) -> Vec<(String, String)> {
            self.sent.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl ReviewSender for FakeSender {
        async fn send_review(
            &self,
            _review_kind: &'static str,
            source_conversation_id: &str,
            text: &str,
        ) -> Result<ReviewSendOutcome, String> {
            self.sent
                .lock()
                .unwrap()
                .push((source_conversation_id.to_string(), text.to_string()));
            self.result.clone()
        }
    }
}
