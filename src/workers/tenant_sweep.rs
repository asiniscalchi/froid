//! Multi-tenant sweeping: one global worker per domain visits every tenant
//! database on each pass, instead of spawning a full worker set per tenant.
//! This keeps the number of polling loops constant as users are added.

use std::{collections::HashMap, convert::Infallible, sync::Arc};

use sqlx::SqlitePool;
use tokio::sync::RwLock;
use tracing::{error, info};

use crate::journal::registry::JournalServiceRegistry;
use crate::workers::config::ReconciliationWorkerConfig;
use crate::workers::reconciliation::ReconciliationCycle;

/// A [`ReconciliationCycle`] that fans one pass out over all tenants known to
/// the registry. Per-tenant inner cycles are built on first sight of a tenant
/// (via the `build` closure) and cached; tenants for which `build` returns
/// `None` (e.g. missing credentials) are skipped on that pass and retried on
/// the next one.
pub struct TenantSweepCycle<C, F> {
    label: &'static str,
    registry: JournalServiceRegistry,
    build: F,
    cycles: RwLock<HashMap<String, Arc<C>>>,
}

impl<C, F> TenantSweepCycle<C, F>
where
    C: ReconciliationCycle + Sync,
    F: Fn(&str, SqlitePool) -> Option<C> + Send + Sync,
{
    pub fn new(label: &'static str, registry: JournalServiceRegistry, build: F) -> Self {
        Self {
            label,
            registry,
            build,
            cycles: RwLock::new(HashMap::new()),
        }
    }

    async fn cycle_for(&self, chat_id: &str, pool: SqlitePool) -> Option<Arc<C>> {
        {
            let guard = self.cycles.read().await;
            if let Some(cycle) = guard.get(chat_id) {
                return Some(cycle.clone());
            }
        }

        let cycle = Arc::new((self.build)(chat_id, pool)?);

        let mut guard = self.cycles.write().await;
        // Double-check to avoid race condition
        if let Some(existing) = guard.get(chat_id) {
            return Some(existing.clone());
        }
        guard.insert(chat_id.to_string(), cycle.clone());
        Some(cycle)
    }
}

impl<C, F> ReconciliationCycle for TenantSweepCycle<C, F>
where
    C: ReconciliationCycle + Sync,
    F: Fn(&str, SqlitePool) -> Option<C> + Send + Sync + 'static,
{
    /// Number of tenants swept on this pass.
    type Outcome = usize;
    type Error = Infallible;

    fn worker_label(&self) -> &'static str {
        self.label
    }

    fn log_startup(&self, config: &ReconciliationWorkerConfig) {
        info!(
            worker = self.label,
            batch_size = config.batch_size,
            interval_seconds = config.interval.as_secs(),
            "tenant sweep worker started"
        );
    }

    fn log_cycle_complete(&self, _outcome: &Self::Outcome) {
        // Per-tenant outcomes are logged by the inner cycles as they run.
    }

    async fn run_once(&self, batch_size: u32) -> Result<Self::Outcome, Self::Error> {
        let tenants = self.registry.tenants().await;
        let mut swept = 0;

        for (chat_id, pool) in tenants {
            let Some(cycle) = self.cycle_for(&chat_id, pool).await else {
                continue;
            };
            match cycle.run_once(batch_size).await {
                Ok(outcome) => cycle.log_cycle_complete(&outcome),
                Err(err) => {
                    error!(
                        worker = self.label,
                        chat_id = %chat_id,
                        error = %err,
                        "tenant sweep cycle failed",
                    );
                }
            }
            swept += 1;
        }

        Ok(swept)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    };

    use clap::Parser;
    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::cli::Cli;

    use crate::journal::registry::JournalServiceRegistryConfig;

    #[derive(Debug)]
    struct NeverError;

    impl std::fmt::Display for NeverError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "never")
        }
    }

    impl std::error::Error for NeverError {}

    struct FakeCycle {
        chat_id: String,
        runs: Arc<Mutex<Vec<String>>>,
        fail: bool,
    }

    impl ReconciliationCycle for FakeCycle {
        type Outcome = ();
        type Error = NeverError;

        fn worker_label(&self) -> &'static str {
            "fake"
        }

        fn log_startup(&self, _config: &ReconciliationWorkerConfig) {}

        fn log_cycle_complete(&self, _outcome: &Self::Outcome) {}

        async fn run_once(&self, _batch_size: u32) -> Result<Self::Outcome, Self::Error> {
            self.runs.lock().unwrap().push(self.chat_id.clone());
            if self.fail { Err(NeverError) } else { Ok(()) }
        }
    }

    async fn registry() -> JournalServiceRegistry {
        let test_id = ulid::Ulid::new().to_string();
        let temp_base_dir = std::env::temp_dir().join(format!("froid_test_sweep_{test_id}"));
        tokio::fs::create_dir_all(&temp_base_dir).await.unwrap();

        let cli = Cli::try_parse_from([
            "froid",
            "--telegram-bot-token",
            "mock_telegram_token_123",
            "--data-dir",
            temp_base_dir.to_str().unwrap(),
        ])
        .unwrap();

        let mut config = cli.serve_config().unwrap();
        config.openai_api_key = None;
        JournalServiceRegistry::new(JournalServiceRegistryConfig {
            config,
            shutdown: CancellationToken::new(),
        })
        .with_base_dir(temp_base_dir)
    }

    #[tokio::test]
    async fn sweeps_every_known_tenant_once_per_pass() {
        let registry = registry().await;
        registry.pool("111").await.unwrap();
        registry.pool("222").await.unwrap();

        let runs = Arc::new(Mutex::new(Vec::new()));
        let sweep = TenantSweepCycle::new("fake-sweep", registry.clone(), {
            let runs = runs.clone();
            move |chat_id: &str, _pool: SqlitePool| {
                Some(FakeCycle {
                    chat_id: chat_id.to_string(),
                    runs: runs.clone(),
                    fail: false,
                })
            }
        });

        let swept = sweep.run_once(10).await.unwrap();

        assert_eq!(swept, 2);
        let mut seen = runs.lock().unwrap().clone();
        seen.sort();
        assert_eq!(seen, vec!["111".to_string(), "222".to_string()]);
    }

    #[tokio::test]
    async fn picks_up_tenants_registered_after_the_first_pass() {
        let registry = registry().await;
        registry.pool("111").await.unwrap();

        let runs = Arc::new(Mutex::new(Vec::new()));
        let sweep = TenantSweepCycle::new("fake-sweep", registry.clone(), {
            let runs = runs.clone();
            move |chat_id: &str, _pool: SqlitePool| {
                Some(FakeCycle {
                    chat_id: chat_id.to_string(),
                    runs: runs.clone(),
                    fail: false,
                })
            }
        });

        assert_eq!(sweep.run_once(10).await.unwrap(), 1);

        registry.pool("222").await.unwrap();
        assert_eq!(sweep.run_once(10).await.unwrap(), 2);
    }

    #[tokio::test]
    async fn skips_tenants_whose_cycle_cannot_be_built() {
        let registry = registry().await;
        registry.pool("111").await.unwrap();
        registry.pool("222").await.unwrap();

        let runs = Arc::new(Mutex::new(Vec::new()));
        let sweep = TenantSweepCycle::new("fake-sweep", registry.clone(), {
            let runs = runs.clone();
            move |chat_id: &str, _pool: SqlitePool| {
                if chat_id == "111" {
                    return None;
                }
                Some(FakeCycle {
                    chat_id: chat_id.to_string(),
                    runs: runs.clone(),
                    fail: false,
                })
            }
        });

        let swept = sweep.run_once(10).await.unwrap();

        assert_eq!(swept, 1);
        assert_eq!(*runs.lock().unwrap(), vec!["222".to_string()]);
    }

    #[tokio::test]
    async fn continues_past_failing_tenants() {
        let registry = registry().await;
        registry.pool("111").await.unwrap();
        registry.pool("222").await.unwrap();

        let runs = Arc::new(Mutex::new(Vec::new()));
        let sweep = TenantSweepCycle::new("fake-sweep", registry.clone(), {
            let runs = runs.clone();
            move |chat_id: &str, _pool: SqlitePool| {
                Some(FakeCycle {
                    chat_id: chat_id.to_string(),
                    runs: runs.clone(),
                    fail: chat_id == "111",
                })
            }
        });

        let swept = sweep.run_once(10).await.unwrap();

        assert_eq!(swept, 2);
        assert_eq!(runs.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn builds_each_tenant_cycle_once() {
        let registry = registry().await;
        registry.pool("111").await.unwrap();

        let built_twice = Arc::new(AtomicBool::new(false));
        let already_built = Arc::new(Mutex::new(std::collections::HashSet::new()));
        let runs = Arc::new(Mutex::new(Vec::new()));

        let sweep = TenantSweepCycle::new("fake-sweep", registry.clone(), {
            let built_twice = built_twice.clone();
            let already_built = already_built.clone();
            let runs = runs.clone();
            move |chat_id: &str, _pool: SqlitePool| {
                if !already_built.lock().unwrap().insert(chat_id.to_string()) {
                    built_twice.store(true, Ordering::SeqCst);
                }
                Some(FakeCycle {
                    chat_id: chat_id.to_string(),
                    runs: runs.clone(),
                    fail: false,
                })
            }
        });

        sweep.run_once(10).await.unwrap();
        sweep.run_once(10).await.unwrap();

        assert!(!built_twice.load(Ordering::SeqCst), "cycle was rebuilt");
        assert_eq!(runs.lock().unwrap().len(), 2);
    }
}
