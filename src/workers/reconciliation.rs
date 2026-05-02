use std::{error::Error, future::Future};

use tokio_util::sync::CancellationToken;
use tracing::error;

use crate::workers::config::ReconciliationWorkerConfig;

/// A single reconciliation pass and the per-cycle bookkeeping (logging,
/// optional summary message) for one worker domain.
///
/// Implementors plug their backfill service into [`ReconciliationWorker`],
/// which owns the loop, the sleep, and the failure logging.
pub trait ReconciliationCycle: Send + 'static {
    type Outcome: Send;
    type Error: Error + Send + Sync + 'static;

    /// Identifier used in error logs from the shared loop.
    fn worker_label(&self) -> &'static str;

    /// Emit a one-time startup log line with worker-specific structured
    /// fields. Called once before the loop starts.
    fn log_startup(&self, config: &ReconciliationWorkerConfig);

    /// Decide whether and how to log the result of a successful cycle.
    /// Implementors typically suppress logs when nothing was attempted.
    fn log_cycle_complete(&self, outcome: &Self::Outcome);

    fn run_once(
        &self,
        batch_size: u32,
    ) -> impl Future<Output = Result<Self::Outcome, Self::Error>> + Send;
}

pub struct ReconciliationWorker<C> {
    cycle: C,
    config: ReconciliationWorkerConfig,
}

impl<C> ReconciliationWorker<C>
where
    C: ReconciliationCycle,
{
    pub fn new(cycle: C, config: ReconciliationWorkerConfig) -> Self {
        Self { cycle, config }
    }

    pub async fn run_once(&self) -> Result<C::Outcome, C::Error> {
        self.cycle.run_once(self.config.batch_size).await
    }

    pub async fn run_forever(self, shutdown: CancellationToken) {
        self.cycle.log_startup(&self.config);

        loop {
            if shutdown.is_cancelled() {
                return;
            }

            match self.cycle.run_once(self.config.batch_size).await {
                Ok(outcome) => self.cycle.log_cycle_complete(&outcome),
                Err(err) => {
                    error!(
                        worker = self.cycle.worker_label(),
                        error = %err,
                        "reconciliation cycle failed",
                    );
                }
            }

            tokio::select! {
                _ = tokio::time::sleep(self.config.interval) => {}
                _ = shutdown.cancelled() => return,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fmt,
        sync::{
            Mutex,
            atomic::{AtomicU32, Ordering},
        },
        time::Duration,
    };

    use super::*;

    #[derive(Debug)]
    struct FakeError;

    impl fmt::Display for FakeError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "fake error")
        }
    }

    impl Error for FakeError {}

    struct FakeCycle {
        run_count: AtomicU32,
        startup_count: AtomicU32,
        cycle_logs: Mutex<Vec<u32>>,
    }

    impl FakeCycle {
        fn new() -> Self {
            Self {
                run_count: AtomicU32::new(0),
                startup_count: AtomicU32::new(0),
                cycle_logs: Mutex::new(Vec::new()),
            }
        }
    }

    impl ReconciliationCycle for FakeCycle {
        type Outcome = u32;
        type Error = FakeError;

        fn worker_label(&self) -> &'static str {
            "fake"
        }

        fn log_startup(&self, _config: &ReconciliationWorkerConfig) {
            self.startup_count.fetch_add(1, Ordering::SeqCst);
        }

        fn log_cycle_complete(&self, outcome: &Self::Outcome) {
            self.cycle_logs.lock().unwrap().push(*outcome);
        }

        async fn run_once(&self, _batch_size: u32) -> Result<Self::Outcome, Self::Error> {
            Ok(self.run_count.fetch_add(1, Ordering::SeqCst) + 1)
        }
    }

    fn config() -> ReconciliationWorkerConfig {
        ReconciliationWorkerConfig {
            enabled: true,
            batch_size: 10,
            interval: Duration::from_secs(60),
        }
    }

    #[tokio::test]
    async fn run_once_delegates_to_cycle() {
        let worker = ReconciliationWorker::new(FakeCycle::new(), config());

        let outcome = worker.run_once().await.unwrap();

        assert_eq!(outcome, 1);
        assert_eq!(worker.cycle.run_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn run_forever_exits_when_cancellation_token_fires() {
        // Use a tiny interval so the loop iterates a few times before we cancel it.
        let worker = ReconciliationWorker::new(
            FakeCycle::new(),
            ReconciliationWorkerConfig {
                enabled: true,
                batch_size: 10,
                interval: Duration::from_millis(1),
            },
        );

        let shutdown = CancellationToken::new();
        let handle = tokio::spawn({
            let shutdown = shutdown.clone();
            async move {
                worker.run_forever(shutdown).await;
            }
        });

        tokio::time::sleep(Duration::from_millis(20)).await;
        shutdown.cancel();
        // The task must terminate on its own once the token is cancelled —
        // never call abort(), because that would mask a wedged loop.
        handle.await.expect("worker task ran to completion");
    }

    #[tokio::test]
    async fn run_forever_returns_immediately_when_token_already_cancelled() {
        let cycle = FakeCycle::new();
        let worker = ReconciliationWorker::new(
            cycle,
            ReconciliationWorkerConfig {
                enabled: true,
                batch_size: 10,
                interval: Duration::from_secs(60),
            },
        );

        let shutdown = CancellationToken::new();
        shutdown.cancel();

        worker.run_forever(shutdown).await;
    }
}
