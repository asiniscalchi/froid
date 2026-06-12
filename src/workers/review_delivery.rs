//! Shared delivery loop for the daily and weekly review workers.
//!
//! Both workers do the same thing for their period: find the conversations
//! with journal entries, obtain the (possibly freshly generated) review, skip
//! it if already delivered, send it, and record the outcome. This module owns
//! that loop; the per-kind modules supply the period arithmetic, the review
//! runner, and the formatting through [`ReviewDeliveryKind`].

use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, Utc};
use tracing::{info, warn};

use crate::errors::from_error_string;
use crate::journal::repository::JournalConversation;
use crate::workers::{
    ReviewSendOutcome, config::ReconciliationWorkerConfig, reconciliation::ReconciliationCycle,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewDeliveryResult {
    pub attempted: usize,
    pub delivered: usize,
    pub skipped: usize,
    pub failed: usize,
}

impl ReviewDeliveryResult {
    pub fn empty() -> Self {
        Self {
            attempted: 0,
            delivered: 0,
            skipped: 0,
            failed: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReviewDeliveryError {
    #[error("{0}")]
    Storage(String),
}

from_error_string!(ReviewDeliveryError::Storage, sqlx::Error);

/// Outcome of producing one period's review, normalized across review kinds.
pub enum ReviewStep<R> {
    /// A completed review that may need delivering.
    Ready(R),
    /// Nothing to deliver for this period (empty day, sparse week).
    Skipped,
    /// Generation or the runner failed; the kind has already logged and
    /// recorded what it needs to.
    Failed,
}

/// The kind-specific half of review delivery (daily vs weekly).
#[async_trait]
pub trait ReviewDeliveryKind: Send + Sync {
    type Review: Send + Sync;

    /// Identifier used by the reconciliation loop's logs.
    const WORKER_LABEL: &'static str;
    /// Human label used in delivery logs ("daily review").
    const KIND: &'static str;

    /// The period to deliver for `now`, or `None` when this cycle is not due.
    fn due_period(&self, now: DateTime<Utc>) -> Option<NaiveDate>;

    fn log_startup(&self, config: &ReconciliationWorkerConfig);

    async fn targets(
        &self,
        period: NaiveDate,
    ) -> Result<Vec<JournalConversation>, ReviewDeliveryError>;

    async fn run_review(
        &self,
        period: NaiveDate,
    ) -> Result<ReviewStep<Self::Review>, ReviewDeliveryError>;

    fn delivered_at(review: &Self::Review) -> Option<DateTime<Utc>>;

    fn format(review: &Self::Review, period: NaiveDate) -> String;

    async fn mark_delivered(&self, period: NaiveDate) -> Result<(), ReviewDeliveryError>;

    async fn mark_delivery_failed(
        &self,
        period: NaiveDate,
        error: &str,
    ) -> Result<(), ReviewDeliveryError>;
}

/// Delivers review text to the conversation it belongs to.
#[async_trait]
pub trait ReviewSender: Send + Sync {
    async fn send_review(
        &self,
        review_kind: &'static str,
        source_conversation_id: &str,
        text: &str,
    ) -> Result<ReviewSendOutcome, String>;
}

pub struct ReviewDeliveryCycle<K, S> {
    kind: K,
    sender: S,
}

impl<K, S> ReviewDeliveryCycle<K, S>
where
    K: ReviewDeliveryKind,
    S: ReviewSender,
{
    pub fn from_parts(kind: K, sender: S) -> Self {
        Self { kind, sender }
    }

    pub(crate) fn kind(&self) -> &K {
        &self.kind
    }

    pub async fn run_once(
        &self,
        now: DateTime<Utc>,
    ) -> Result<ReviewDeliveryResult, ReviewDeliveryError> {
        match self.kind.due_period(now) {
            Some(period) => self.run_once_for(period).await,
            None => Ok(ReviewDeliveryResult::empty()),
        }
    }

    pub async fn run_once_for(
        &self,
        period: NaiveDate,
    ) -> Result<ReviewDeliveryResult, ReviewDeliveryError> {
        let targets = self.kind.targets(period).await?;

        let mut result = ReviewDeliveryResult {
            attempted: targets.len(),
            ..ReviewDeliveryResult::empty()
        };

        for target in targets {
            let review = match self.kind.run_review(period).await? {
                ReviewStep::Ready(review) => review,
                ReviewStep::Skipped => {
                    result.skipped += 1;
                    continue;
                }
                ReviewStep::Failed => {
                    result.failed += 1;
                    continue;
                }
            };

            if K::delivered_at(&review).is_some() {
                result.skipped += 1;
                continue;
            }

            let text = K::format(&review, period);
            match self
                .sender
                .send_review(K::KIND, &target.source_conversation_id, &text)
                .await
            {
                Ok(ReviewSendOutcome::Sent) => {
                    self.kind.mark_delivered(period).await?;
                    result.delivered += 1;
                }
                Ok(ReviewSendOutcome::Skipped) => {
                    result.skipped += 1;
                }
                Err(error) => {
                    self.kind.mark_delivery_failed(period, &error).await?;
                    warn!(
                        review_kind = K::KIND,
                        source_conversation_id = %target.source_conversation_id,
                        period = %period,
                        error = %error,
                        "failed to deliver review"
                    );
                    result.failed += 1;
                }
            }
        }

        Ok(result)
    }
}

impl<K, S> ReconciliationCycle for ReviewDeliveryCycle<K, S>
where
    K: ReviewDeliveryKind + 'static,
    S: ReviewSender + 'static,
{
    type Outcome = ReviewDeliveryResult;
    type Error = ReviewDeliveryError;

    fn worker_label(&self) -> &'static str {
        K::WORKER_LABEL
    }

    fn log_startup(&self, config: &ReconciliationWorkerConfig) {
        self.kind.log_startup(config);
    }

    fn log_cycle_complete(&self, outcome: &Self::Outcome) {
        if outcome.attempted > 0 && (outcome.delivered > 0 || outcome.failed > 0) {
            info!(
                review_kind = K::KIND,
                attempted = outcome.attempted,
                delivered = outcome.delivered,
                skipped = outcome.skipped,
                failed = outcome.failed,
                "review delivery cycle completed"
            );
        }
    }

    async fn run_once(&self, _batch_size: u32) -> Result<Self::Outcome, Self::Error> {
        self.run_once(Utc::now()).await
    }
}
