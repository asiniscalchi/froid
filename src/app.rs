use std::error::Error;
use std::future::{Future, pending};
use std::sync::Arc;

use sqlx::SqlitePool;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::{
    adapters::{Adapter, analyzer_telegram::AnalyzerTelegramAdapter, telegram::TelegramAdapter},
    cli::ServeConfig,
    database,
    journal::{
        analyzer::{
            DefaultSemanticJournalSearcher, RigOpenAiAnalyzerAgent, build_analyzer_tool_registry,
        },
        embedding::{
            EmbeddingBackfillService, EmbeddingConfig, RigOpenAiEmbedder, SqliteEmbeddingRepository,
        },
        extraction::{
            ExtractionBackfillService, JournalEntryExtractionRuntimeConfig,
            configure_journal_entry_extraction, repository::JournalEntryExtractionRepository,
            service::JournalEntryExtractionService,
        },
        repository::JournalRepository,
        review::{
            DailyReviewRuntimeConfig, build_daily_review_service, configure_daily_review,
            signals::{
                backfill::DailyReviewSignalBackfillService,
                repository::DailyReviewSignalRepository,
                wiring::{DailyReviewSignalRuntimeConfig, build_signal_service},
            },
        },
        search::SemanticSearchService,
        service::JournalService,
        status::EmbeddingStatusConfig,
        week_review::{
            WeeklyReviewRuntimeConfig, build_weekly_review_service, configure_weekly_review,
        },
    },
    version,
    workers::{
        ReconciliationWorker,
        daily_review::{DailyReviewDeliveryWorker, TelegramDailyReviewSender},
        embedding::EmbeddingCycle,
        extraction::ExtractionCycle,
        signals::DailyReviewSignalCycle,
        weekly_review::{TelegramWeeklyReviewSender, WeeklyReviewDeliveryWorker},
    },
};

pub async fn serve(config: ServeConfig) -> Result<(), Box<dyn Error>> {
    info!(
        version = version::VERSION,
        command = "serve",
        adapter = "telegram",
        database_path = %config.database_path,
        "starting service"
    );

    let pool = database::connect_pool(&config.database_url).await?;

    sqlx::migrate!().run(&pool).await?;

    let embedding_config = EmbeddingConfig::from_env().ok();
    let daily_review_config = DailyReviewRuntimeConfig::from_env();
    let weekly_review_config = WeeklyReviewRuntimeConfig::from_env();
    let entry_extraction_config = JournalEntryExtractionRuntimeConfig::from_env();
    let signal_runtime_config = DailyReviewSignalRuntimeConfig::from_env();

    let shutdown = CancellationToken::new();
    let mut workers: JoinSet<&'static str> = JoinSet::new();

    spawn_embedding_worker(
        &mut workers,
        &shutdown,
        &pool,
        &config,
        embedding_config.as_ref(),
    )?;
    spawn_daily_review_embedding_worker(
        &mut workers,
        &shutdown,
        &pool,
        &config,
        embedding_config.as_ref(),
    )?;
    spawn_extraction_worker(
        &mut workers,
        &shutdown,
        &pool,
        &config,
        &entry_extraction_config,
    )?;
    let delivery_configured = spawn_daily_review_delivery_worker(
        &mut workers,
        &shutdown,
        &pool,
        &config,
        daily_review_config.clone(),
    )?;
    spawn_weekly_review_delivery_worker(
        &mut workers,
        &shutdown,
        &pool,
        &config,
        weekly_review_config.clone(),
    )?;
    spawn_signal_worker(
        &mut workers,
        &shutdown,
        &pool,
        &config,
        signal_runtime_config,
    )?;
    spawn_analyzer_telegram_worker(
        &mut workers,
        &shutdown,
        &pool,
        &config,
        embedding_config.as_ref(),
    );
    let journal_service = build_journal_service(
        pool,
        embedding_config,
        entry_extraction_config,
        daily_review_config,
        weekly_review_config,
        delivery_configured,
    )?;

    let adapter = TelegramAdapter::new(config.telegram_bot_token, journal_service);
    supervise(workers, shutdown, shutdown_signal(), adapter.run()).await
}

/// Race the adapter against the worker JoinSet and the shutdown signal.
///
/// Returns Ok only when the OS asked us to stop. Any other exit (a worker
/// panicking or returning, the adapter loop unwinding) is fatal — the
/// returned error bubbles up to `main` so the process exits non-zero and a
/// supervisor (systemd, Docker) restarts the binary.
async fn supervise(
    mut workers: JoinSet<&'static str>,
    shutdown: CancellationToken,
    shutdown_signal: impl Future<Output = ()>,
    adapter: impl Future<Output = ()>,
) -> Result<(), Box<dyn Error>> {
    let outcome: Result<(), Box<dyn Error>> = tokio::select! {
        () = adapter => {
            error!("adapter loop exited unexpectedly");
            Err("adapter loop exited unexpectedly".into())
        }
        Some(result) = workers.join_next(), if !workers.is_empty() => {
            match result {
                Ok(label) => {
                    error!(worker = label, "worker exited unexpectedly");
                    Err(format!("worker '{label}' exited unexpectedly").into())
                }
                Err(err) if err.is_panic() => {
                    error!(error = %err, "worker task panicked");
                    Err(format!("worker task panicked: {err}").into())
                }
                Err(err) => {
                    error!(error = %err, "worker task failed");
                    Err(format!("worker task failed: {err}").into())
                }
            }
        }
        () = shutdown_signal => {
            info!("shutdown signal received, draining workers");
            Ok(())
        }
    };

    shutdown.cancel();
    while workers.join_next().await.is_some() {}

    outcome
}

#[cfg(unix)]
async fn shutdown_signal() {
    use tokio::signal::unix::{SignalKind, signal};

    let mut sigterm = match signal(SignalKind::terminate()) {
        Ok(stream) => stream,
        Err(err) => {
            warn!(error = %err, "failed to install SIGTERM handler; only SIGINT will trigger shutdown");
            tokio::select! {
                result = tokio::signal::ctrl_c() => {
                    if let Err(err) = result {
                        warn!(error = %err, "ctrl-c handler error");
                    }
                    info!("received SIGINT, shutting down");
                }
                () = pending::<()>() => {}
            }
            return;
        }
    };

    tokio::select! {
        result = tokio::signal::ctrl_c() => {
            if let Err(err) = result {
                warn!(error = %err, "ctrl-c handler error");
            }
            info!("received SIGINT, shutting down");
        }
        _ = sigterm.recv() => {
            info!("received SIGTERM, shutting down");
        }
    }
}

#[cfg(not(unix))]
async fn shutdown_signal() {
    if let Err(err) = tokio::signal::ctrl_c().await {
        warn!(error = %err, "ctrl-c handler error");
        pending::<()>().await;
    }
    info!("received SIGINT, shutting down");
}

fn spawn_embedding_worker(
    workers: &mut JoinSet<&'static str>,
    shutdown: &CancellationToken,
    pool: &SqlitePool,
    config: &ServeConfig,
    embedding_config: Option<&EmbeddingConfig>,
) -> Result<(), Box<dyn Error>> {
    if config.embedding_worker.enabled
        && let Some(cfg) = embedding_config
    {
        let embedder = RigOpenAiEmbedder::from_env(cfg.clone())?;
        let index = SqliteEmbeddingRepository::new(pool.clone());
        let backfill_service = EmbeddingBackfillService::new(index, embedder);
        let worker = ReconciliationWorker::new(
            EmbeddingCycle::new(backfill_service),
            config.embedding_worker.clone(),
        );
        let token = shutdown.clone();
        workers.spawn(async move {
            worker.run_forever(token).await;
            "embedding"
        });
    }

    Ok(())
}

fn spawn_daily_review_embedding_worker(
    workers: &mut JoinSet<&'static str>,
    shutdown: &CancellationToken,
    pool: &SqlitePool,
    config: &ServeConfig,
    embedding_config: Option<&EmbeddingConfig>,
) -> Result<(), Box<dyn Error>> {
    if config.daily_review_embedding_worker.enabled
        && let Some(cfg) = embedding_config
    {
        let embedder = RigOpenAiEmbedder::from_env(cfg.clone())?;
        let index =
            crate::journal::review::embedding_repository::SqliteDailyReviewEmbeddingRepository::new(
                pool.clone(),
            );
        let backfill_service = EmbeddingBackfillService::new(index, embedder);
        let worker = ReconciliationWorker::new(
            EmbeddingCycle::new(backfill_service),
            config.daily_review_embedding_worker.clone(),
        );
        let token = shutdown.clone();
        workers.spawn(async move {
            worker.run_forever(token).await;
            "daily_review_embedding"
        });
    }

    Ok(())
}

fn spawn_extraction_worker(
    workers: &mut JoinSet<&'static str>,
    shutdown: &CancellationToken,
    pool: &SqlitePool,
    config: &ServeConfig,
    entry_extraction_config: &JournalEntryExtractionRuntimeConfig,
) -> Result<(), Box<dyn Error>> {
    if !config.extraction_worker.enabled {
        return Ok(());
    }

    let Some(openai_api_key) = entry_extraction_config
        .openai_api_key
        .as_ref()
        .filter(|v| !v.trim().is_empty())
    else {
        warn!("extraction reconciliation worker is enabled but OPENAI_API_KEY is not configured");
        return Ok(());
    };

    let prompt = entry_extraction_config.prompt.load()?;
    let generator = crate::journal::extraction::RigOpenAiJournalEntryExtractionGenerator::from_optional_api_key(
        entry_extraction_config.extraction.clone(),
        prompt,
        Some(openai_api_key.clone()),
    )?;
    let repository = JournalEntryExtractionRepository::new(pool.clone());
    let runner = JournalEntryExtractionService::new(repository.clone(), generator);
    let backfill = ExtractionBackfillService::new(repository, runner);
    let worker = ReconciliationWorker::new(
        ExtractionCycle::new(backfill),
        config.extraction_worker.clone(),
    );
    let token = shutdown.clone();
    workers.spawn(async move {
        worker.run_forever(token).await;
        "extraction"
    });

    Ok(())
}

fn spawn_daily_review_delivery_worker(
    workers: &mut JoinSet<&'static str>,
    shutdown: &CancellationToken,
    pool: &SqlitePool,
    config: &ServeConfig,
    daily_review_config: DailyReviewRuntimeConfig,
) -> Result<bool, Box<dyn Error>> {
    if !config.daily_review_delivery.enabled {
        return Ok(false);
    }

    let Some(daily_review_service) = build_daily_review_service(pool.clone(), daily_review_config)?
    else {
        return Ok(false);
    };

    let worker = DailyReviewDeliveryWorker::new(
        JournalRepository::new(pool.clone()),
        crate::journal::review::repository::DailyReviewRepository::new(pool.clone()),
        daily_review_service,
        TelegramDailyReviewSender::new(config.telegram_bot_token.clone()),
        config.daily_review_delivery.clone(),
    );
    let token = shutdown.clone();
    workers.spawn(async move {
        worker.run_forever(token).await;
        "daily_review_delivery"
    });

    Ok(true)
}

fn spawn_weekly_review_delivery_worker(
    workers: &mut JoinSet<&'static str>,
    shutdown: &CancellationToken,
    pool: &SqlitePool,
    config: &ServeConfig,
    weekly_review_config: WeeklyReviewRuntimeConfig,
) -> Result<(), Box<dyn Error>> {
    if !config.weekly_review_delivery.enabled {
        return Ok(());
    }

    let Some(weekly_review_service) =
        build_weekly_review_service(pool.clone(), weekly_review_config)?
    else {
        return Ok(());
    };

    let worker = WeeklyReviewDeliveryWorker::new(
        JournalRepository::new(pool.clone()),
        crate::journal::week_review::repository::WeeklyReviewRepository::new(pool.clone()),
        weekly_review_service,
        TelegramWeeklyReviewSender::new(config.telegram_bot_token.clone()),
        config.weekly_review_delivery.clone(),
    );
    let token = shutdown.clone();
    workers.spawn(async move {
        worker.run_forever(token).await;
        "weekly_review_delivery"
    });

    Ok(())
}

fn spawn_signal_worker(
    workers: &mut JoinSet<&'static str>,
    shutdown: &CancellationToken,
    pool: &SqlitePool,
    config: &ServeConfig,
    signal_config: DailyReviewSignalRuntimeConfig,
) -> Result<(), Box<dyn Error>> {
    if !config.signal_worker.enabled {
        return Ok(());
    }

    let Some(service) = build_signal_service(pool.clone(), signal_config)? else {
        warn!("signal reconciliation worker is enabled but OPENAI_API_KEY is not configured");
        return Ok(());
    };

    let backfill = DailyReviewSignalBackfillService::new(
        DailyReviewSignalRepository::new(pool.clone()),
        service,
    );
    let worker = ReconciliationWorker::new(
        DailyReviewSignalCycle::new(backfill),
        config.signal_worker.clone(),
    );
    let token = shutdown.clone();
    workers.spawn(async move {
        worker.run_forever(token).await;
        "signal"
    });

    Ok(())
}

fn spawn_analyzer_telegram_worker(
    workers: &mut JoinSet<&'static str>,
    shutdown: &CancellationToken,
    pool: &SqlitePool,
    config: &ServeConfig,
    embedding_config: Option<&EmbeddingConfig>,
) {
    let Some(bot_token) = config.analyzer_telegram_bot_token.clone() else {
        return;
    };

    let Some(embedding_cfg) = embedding_config else {
        warn!(
            "analyzer telegram bot is configured but embedding configuration is missing; analyzer bot will not start"
        );
        return;
    };

    let embedder = match RigOpenAiEmbedder::from_env(embedding_cfg.clone()) {
        Ok(embedder) => embedder,
        Err(error) => {
            warn!(
                error = %error,
                "failed to construct OpenAI embedder for analyzer; analyzer bot will not start"
            );
            return;
        }
    };

    let semantic = Arc::new(DefaultSemanticJournalSearcher::new(
        SqliteEmbeddingRepository::new(pool.clone()),
        embedder,
        JournalRepository::new(pool.clone()),
    ));

    let registry = build_analyzer_tool_registry(pool.clone(), semantic);

    let agent = match RigOpenAiAnalyzerAgent::from_env(registry) {
        Ok(agent) => Arc::new(agent),
        Err(error) => {
            warn!(
                error = %error,
                "failed to build analyzer agent; analyzer bot will not start"
            );
            return;
        }
    };

    let adapter = AnalyzerTelegramAdapter::new(bot_token, agent);
    let token = shutdown.clone();
    workers.spawn(async move {
        adapter.run_until_cancelled(token).await;
        "analyzer_telegram"
    });
}

fn build_journal_service(
    pool: SqlitePool,
    embedding_config: Option<EmbeddingConfig>,
    entry_extraction_config: JournalEntryExtractionRuntimeConfig,
    daily_review_config: DailyReviewRuntimeConfig,
    weekly_review_config: WeeklyReviewRuntimeConfig,
    delivery_configured: bool,
) -> Result<JournalService, Box<dyn Error>> {
    let mut journal_service = JournalService::new(JournalRepository::new(pool.clone()));

    journal_service =
        configure_journal_entry_extraction(journal_service, pool.clone(), entry_extraction_config)?;

    journal_service = configure_daily_review(journal_service, pool.clone(), daily_review_config)?;

    journal_service = configure_weekly_review(journal_service, pool.clone(), weekly_review_config)?;

    if delivery_configured {
        journal_service = journal_service.with_daily_review_delivery_configured();
    }

    if let Some(cfg) = embedding_config {
        let embedder = RigOpenAiEmbedder::from_env(cfg.clone()).map_err(|error| {
            warn!(
                error = %error,
                "failed to construct OpenAI embedder for journal service; semantic search will be unavailable"
            );
            error
        })?;
        let embedder = Arc::new(embedder);

        let embedding_repository = SqliteEmbeddingRepository::new(pool.clone());
        let review_search_index =
            crate::journal::review::embedding_repository::SqliteDailyReviewEmbeddingRepository::new(
                pool.clone(),
            );
        let status_config = EmbeddingStatusConfig {
            model: cfg.model,
            dimensions: cfg.dimensions,
        };
        let search = SemanticSearchService::new(
            embedding_repository.clone(),
            Arc::clone(&embedder),
            JournalRepository::new(pool.clone()),
        );
        let review_search = crate::journal::review::search::SemanticDailyReviewSearchService::new(
            review_search_index,
            Arc::clone(&embedder),
            crate::journal::review::repository::DailyReviewRepository::new(pool.clone()),
        );

        journal_service = journal_service.with_search(search);
        journal_service = journal_service.with_daily_review_search(review_search);
        journal_service =
            journal_service.with_capture_embedding(embedding_repository.clone(), embedder);
        journal_service = journal_service.with_embedding_status_config(status_config);
        journal_service = journal_service.with_pending_embedding_counter(embedding_repository);
    }

    Ok(journal_service)
}

#[cfg(test)]
mod tests {
    use std::future::pending;
    use std::time::Duration;

    use tokio::task::JoinSet;
    use tokio_util::sync::CancellationToken;

    use super::supervise;

    #[tokio::test]
    async fn supervise_returns_ok_when_signal_fires() {
        let workers: JoinSet<&'static str> = JoinSet::new();
        let shutdown = CancellationToken::new();
        let signal = async {};
        let adapter = pending::<()>();

        let result = supervise(workers, shutdown.clone(), signal, adapter).await;

        assert!(result.is_ok());
        assert!(shutdown.is_cancelled());
    }

    #[tokio::test]
    async fn supervise_drains_workers_on_signal() {
        let mut workers: JoinSet<&'static str> = JoinSet::new();
        let shutdown = CancellationToken::new();
        let token_for_worker = shutdown.clone();
        workers.spawn(async move {
            token_for_worker.cancelled().await;
            "fake"
        });
        let adapter = pending::<()>();

        let result = supervise(workers, shutdown.clone(), async {}, adapter).await;

        assert!(result.is_ok());
        assert!(shutdown.is_cancelled());
    }

    #[tokio::test]
    async fn supervise_returns_err_when_a_worker_exits_cleanly() {
        let mut workers: JoinSet<&'static str> = JoinSet::new();
        workers.spawn(async { "embedding" });
        let shutdown = CancellationToken::new();
        let signal = pending::<()>();
        let adapter = pending::<()>();

        let result = supervise(workers, shutdown.clone(), signal, adapter).await;

        let err = result.expect_err("worker exit must surface as error");
        assert!(
            err.to_string().contains("embedding"),
            "error should name the worker that died, got: {err}"
        );
        assert!(shutdown.is_cancelled());
    }

    #[tokio::test]
    async fn supervise_returns_err_when_a_worker_panics() {
        let mut workers: JoinSet<&'static str> = JoinSet::new();
        workers.spawn(async {
            panic!("boom");
        });
        let shutdown = CancellationToken::new();
        let signal = pending::<()>();
        let adapter = pending::<()>();

        let result = supervise(workers, shutdown.clone(), signal, adapter).await;

        let err = result.expect_err("panic must surface as error");
        assert!(
            err.to_string().contains("panicked"),
            "error should describe a panic, got: {err}"
        );
        assert!(shutdown.is_cancelled());
    }

    #[tokio::test]
    async fn supervise_returns_err_when_adapter_exits() {
        let workers: JoinSet<&'static str> = JoinSet::new();
        let shutdown = CancellationToken::new();
        let signal = pending::<()>();
        let adapter = async {};

        let result = supervise(workers, shutdown.clone(), signal, adapter).await;

        let err = result.expect_err("adapter exit must surface as error");
        assert!(err.to_string().contains("adapter"));
        assert!(shutdown.is_cancelled());
    }

    #[tokio::test]
    async fn supervise_cancels_siblings_when_one_worker_dies() {
        // A second worker observes the shared token; when the first one dies,
        // supervise() must cancel and the sibling must drain cleanly.
        let mut workers: JoinSet<&'static str> = JoinSet::new();
        let shutdown = CancellationToken::new();

        workers.spawn(async { "embedding" });
        let sibling_token = shutdown.clone();
        workers.spawn(async move {
            sibling_token.cancelled().await;
            "sibling"
        });

        let adapter = pending::<()>();
        let signal = pending::<()>();

        let result = tokio::time::timeout(
            Duration::from_secs(1),
            supervise(workers, shutdown.clone(), signal, adapter),
        )
        .await
        .expect("supervise must drain quickly when token is cancelled");

        assert!(result.is_err());
        assert!(shutdown.is_cancelled());
    }
}
