use std::error::Error;
use std::future::{Future, pending};
use std::sync::Arc;

use sqlx::SqlitePool;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::{
    adapters::telegram::TelegramAdapter,
    auth::UserTokens,
    cli::ServeConfig,
    database, http,
    journal::{
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
    prompts::{PromptKey, PromptRepository, PromptSource},
    version,
    workers::{
        ReconciliationWorker, ReconciliationWorkerConfig,
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
        mcp_enabled = config.mcp_server.enabled,
        mcp_bind = %config.mcp_server.bind,
        dashboard_enabled = config.dashboard.enabled,
        "starting service"
    );

    let embedding_config = Some(EmbeddingConfig::from_env());
    let daily_review_config = DailyReviewRuntimeConfig::from_env();
    let weekly_review_config = WeeklyReviewRuntimeConfig::from_env();
    let entry_extraction_config = JournalEntryExtractionRuntimeConfig::from_env();
    let signal_runtime_config = DailyReviewSignalRuntimeConfig::from_env();

    let shutdown = CancellationToken::new();
    let mut workers: JoinSet<&'static str> = JoinSet::new();

    let delivery_configured = config.daily_review_delivery.enabled;

    let config = Arc::new(config);

    // Initialize the multiuser journal service registry
    let journal_registry = crate::journal::registry::JournalServiceRegistry::new(
        crate::journal::registry::JournalServiceRegistryConfig {
            config: (*config).clone(),
            embedding_config: embedding_config.clone(),
            entry_extraction_config: entry_extraction_config.clone(),
            daily_review_config: daily_review_config.clone(),
            weekly_review_config: weekly_review_config.clone(),
            signal_runtime_config: signal_runtime_config.clone(),
            delivery_configured,
            shutdown: shutdown.clone(),
        },
    );

    // Central store for tokens issued via the Telegram /token command. It
    // lives in the default database so one lookup resolves any token.
    let issued_tokens = if config.http_auth.dynamic_tokens {
        let pool = database::connect_pool(&config.database_url).await?;
        sqlx::migrate!().run(&pool).await?;
        Some(crate::tokens::UserTokenStore::new(pool))
    } else {
        None
    };

    // Spawn the global HTTP server (MCP and Dashboard)
    spawn_http_server(
        &mut workers,
        &shutdown,
        &config,
        embedding_config.as_ref(),
        &journal_registry,
        issued_tokens.clone(),
    )
    .await?;

    // Discover and auto-load existing tenant databases on disk
    journal_registry
        .discover_and_register_existing()
        .await
        .map_err(|e| -> Box<dyn std::error::Error> { e })?;

    // Spawn the global sweep workers that service every tenant database
    spawn_global_workers(
        &mut workers,
        GlobalWorkersConfig {
            registry: journal_registry.clone(),
            config: config.clone(),
            embedding_config,
            entry_extraction_config,
            daily_review_config,
            weekly_review_config,
            signal_runtime_config,
            shutdown: shutdown.clone(),
        },
    );

    let adapter = TelegramAdapter::new(
        config.telegram_bot_token.clone(),
        config.telegram_allowed_user_ids.clone(),
        journal_registry,
    )
    .with_token_issuer(issued_tokens.map(crate::tokens::TokenIssuer::new));
    supervise(workers, shutdown, shutdown_signal(), adapter.run()).await
}

async fn spawn_http_server(
    workers: &mut JoinSet<&'static str>,
    shutdown: &CancellationToken,
    config: &ServeConfig,
    embedding_config: Option<&EmbeddingConfig>,
    registry: &crate::journal::registry::JournalServiceRegistry,
    issued_tokens: Option<crate::tokens::UserTokenStore>,
) -> Result<(), Box<dyn Error>> {
    if !config.mcp_server.enabled && !config.dashboard.enabled {
        return Ok(());
    }

    if config.mcp_server.enabled && embedding_config.is_none() {
        warn!(
            "MCP server is enabled but embedding configuration is missing; skipping (semantic search requires it)"
        );
        return Ok(());
    }

    let router_config = http::TenantRouterConfig {
        mcp_enabled: config.mcp_server.enabled,
        dashboard_enabled: config.dashboard.enabled,
        embedding_config: embedding_config.cloned(),
        shutdown: shutdown.clone(),
    };

    let per_user = !config.http_auth.user_tokens.is_empty() || issued_tokens.is_some();
    let (router, auth_mode) = if per_user {
        // Per-user tokens: every /mcp and /api request is served from the
        // database of the user owning the presented token. Tokens come from
        // FROID_AUTH_TOKENS and, when dynamic tokens are enabled, from the
        // store fed by the Telegram /token command.
        let resolver = Arc::new(crate::auth::TokenResolver::new(
            UserTokens::new(config.http_auth.user_tokens.clone()),
            issued_tokens,
        ));
        let tenants = http::TenantRouters::new(registry.clone(), router_config);
        (
            http::build_per_user_app(tenants, resolver, config.dashboard.enabled),
            "per-user",
        )
    } else {
        // Single tenant: in multiuser mode with a whitelist, the
        // administrative user is the first ID in the whitelist and we serve
        // their isolated database. Otherwise, the default/legacy database.
        let (pool, capture_conversation_id) = if let Some(first_id) = config
            .telegram_allowed_user_ids
            .as_ref()
            .and_then(|ids| ids.first())
        {
            info!(
                chat_id = %first_id,
                "HTTP server (MCP & Dashboard) running in multiuser mode; binding to first whitelisted user's database"
            );
            let chat_id = first_id.to_string();
            let pool = registry
                .pool(&chat_id)
                .await
                .map_err(|e| -> Box<dyn Error> { e })?;
            (pool, chat_id)
        } else {
            let pool = database::connect_pool(&config.database_url).await?;
            sqlx::migrate!().run(&pool).await?;
            (pool, crate::messages::SINGLE_USER_ID.to_string())
        };

        let tenant_router =
            http::build_tenant_router(&pool, &capture_conversation_id, &router_config)
                .map_err(|e| -> Box<dyn Error> { e })?;
        let token: Option<Arc<str>> = config.http_auth.token.clone().map(Arc::from);
        let auth_mode = if token.is_some() {
            "single-token"
        } else {
            warn!(
                "HTTP server (MCP & Dashboard) is running without authentication; set FROID_AUTH_TOKEN or FROID_AUTH_TOKENS to require a bearer token"
            );
            "none"
        };
        (
            http::build_single_tenant_app(tenant_router, token, config.dashboard.enabled),
            auth_mode,
        )
    };

    let listener = tokio::net::TcpListener::bind(config.mcp_server.bind).await?;
    let local_addr = listener.local_addr()?;
    info!(
        addr = %local_addr,
        mcp = config.mcp_server.enabled,
        dashboard = config.dashboard.enabled,
        auth = auth_mode,
        "HTTP server listening"
    );

    let token = shutdown.clone();
    workers.spawn(async move {
        let serve = axum::serve(listener, router).with_graceful_shutdown(async move {
            token.cancelled().await;
        });
        if let Err(err) = serve.await {
            error!(error = %err, "HTTP server exited with error");
        }
        "http"
    });

    Ok(())
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

pub(crate) fn build_journal_service(
    pool: SqlitePool,
    prompt_repository: &PromptRepository,
    embedding_config: Option<EmbeddingConfig>,
    entry_extraction_config: JournalEntryExtractionRuntimeConfig,
    daily_review_config: DailyReviewRuntimeConfig,
    weekly_review_config: WeeklyReviewRuntimeConfig,
    delivery_configured: bool,
) -> Result<JournalService, Box<dyn Error>> {
    let mut journal_service = JournalService::new(JournalRepository::new(pool.clone()));

    journal_service = configure_journal_entry_extraction(
        journal_service,
        pool.clone(),
        prompt_repository,
        entry_extraction_config,
    )?;

    journal_service = configure_daily_review(
        journal_service,
        pool.clone(),
        prompt_repository,
        daily_review_config,
    )?;

    journal_service = configure_weekly_review(
        journal_service,
        pool.clone(),
        prompt_repository,
        weekly_review_config,
    )?;

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
        let status_config = EmbeddingStatusConfig { model: cfg.model };
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

pub struct GlobalWorkersConfig {
    pub registry: crate::journal::registry::JournalServiceRegistry,
    pub config: Arc<ServeConfig>,
    pub embedding_config: Option<EmbeddingConfig>,
    pub entry_extraction_config: JournalEntryExtractionRuntimeConfig,
    pub daily_review_config: DailyReviewRuntimeConfig,
    pub weekly_review_config: WeeklyReviewRuntimeConfig,
    pub signal_runtime_config: DailyReviewSignalRuntimeConfig,
    pub shutdown: CancellationToken,
}

/// Spawn one sweep worker into the supervised JoinSet. Each pass it visits
/// every tenant database known to the registry.
fn spawn_sweep<C, F>(
    workers: &mut JoinSet<&'static str>,
    label: &'static str,
    registry: crate::journal::registry::JournalServiceRegistry,
    build: F,
    worker_config: ReconciliationWorkerConfig,
    shutdown: CancellationToken,
) where
    C: crate::workers::ReconciliationCycle + Sync,
    F: Fn(&str, SqlitePool) -> Option<C> + Send + Sync + 'static,
{
    let worker = ReconciliationWorker::new(
        crate::workers::TenantSweepCycle::new(label, registry, build),
        worker_config,
    );
    workers.spawn(async move {
        worker.run_forever(shutdown).await;
        label
    });
}

/// Spawn the global background workers. One worker per domain sweeps all
/// tenant databases, so the number of polling loops stays constant no matter
/// how many users the instance serves.
pub fn spawn_global_workers(workers: &mut JoinSet<&'static str>, config: GlobalWorkersConfig) {
    let GlobalWorkersConfig {
        registry,
        config,
        embedding_config,
        entry_extraction_config,
        daily_review_config,
        weekly_review_config,
        signal_runtime_config,
        shutdown,
    } = config;

    // 1. Embedding Worker
    if config.embedding_worker.enabled
        && let Some(cfg) = &embedding_config
        && let Ok(embedder) = RigOpenAiEmbedder::from_env(cfg.clone())
    {
        spawn_sweep(
            workers,
            "embedding-sweep",
            registry.clone(),
            move |_chat_id, pool| {
                let index = SqliteEmbeddingRepository::new(pool);
                Some(EmbeddingCycle::new(EmbeddingBackfillService::new(
                    index,
                    embedder.clone(),
                )))
            },
            config.embedding_worker.clone(),
            shutdown.clone(),
        );
    }

    // 2. Daily Review Embedding Worker
    if config.daily_review_embedding_worker.enabled
        && let Some(cfg) = &embedding_config
        && let Ok(embedder) = RigOpenAiEmbedder::from_env(cfg.clone())
    {
        spawn_sweep(
            workers,
            "daily-review-embedding-sweep",
            registry.clone(),
            move |_chat_id, pool| {
                let index =
                    crate::journal::review::embedding_repository::SqliteDailyReviewEmbeddingRepository::new(
                        pool,
                    );
                Some(EmbeddingCycle::new(EmbeddingBackfillService::new(
                    index,
                    embedder.clone(),
                )))
            },
            config.daily_review_embedding_worker.clone(),
            shutdown.clone(),
        );
    }

    // 3. Extraction Worker
    if config.extraction_worker.enabled
        && entry_extraction_config
            .openai_api_key
            .as_deref()
            .is_some_and(|key| !key.trim().is_empty())
    {
        let extraction_config = entry_extraction_config.clone();
        spawn_sweep(
            workers,
            "extraction-sweep",
            registry.clone(),
            move |_chat_id, pool| {
                let prompt = extraction_config.prompt.load().ok()?;
                let generator =
                    crate::journal::extraction::RigOpenAiJournalEntryExtractionGenerator::from_optional_api_key(
                        extraction_config.extraction.clone(),
                        prompt,
                        extraction_config.openai_api_key.clone(),
                    )
                    .ok()?;
                let prompt_source = PromptSource::new(
                    PromptRepository::new(pool.clone()),
                    PromptKey::EntryExtraction,
                    extraction_config.prompt.path.clone(),
                );
                let generator = generator.with_prompt_source(prompt_source);
                let repository = JournalEntryExtractionRepository::new(pool);
                let runner = JournalEntryExtractionService::new(repository.clone(), generator);
                Some(ExtractionCycle::new(ExtractionBackfillService::new(
                    repository, runner,
                )))
            },
            config.extraction_worker.clone(),
            shutdown.clone(),
        );
    }

    // 4. Daily Review Delivery Worker
    if config.daily_review_delivery.enabled {
        let serve_config = config.clone();
        spawn_sweep(
            workers,
            "daily-review-delivery-sweep",
            registry.clone(),
            move |_chat_id, pool| {
                let prompt_repository = PromptRepository::new(pool.clone());
                let daily_review_service = build_daily_review_service(
                    pool.clone(),
                    &prompt_repository,
                    daily_review_config.clone(),
                )
                .ok()
                .flatten()?;
                Some(DailyReviewDeliveryWorker::new(
                    JournalRepository::new(pool.clone()),
                    crate::journal::review::repository::DailyReviewRepository::new(pool),
                    daily_review_service,
                    TelegramDailyReviewSender::new(
                        serve_config.telegram_bot_token.clone(),
                        serve_config.telegram_allowed_user_ids.clone(),
                    ),
                    serve_config.daily_review_delivery.clone(),
                ))
            },
            ReconciliationWorkerConfig {
                enabled: config.daily_review_delivery.enabled,
                batch_size: 1,
                interval: config.daily_review_delivery.interval,
            },
            shutdown.clone(),
        );
    }

    // 5. Weekly Review Delivery Worker
    if config.weekly_review_delivery.enabled {
        let serve_config = config.clone();
        spawn_sweep(
            workers,
            "weekly-review-delivery-sweep",
            registry.clone(),
            move |_chat_id, pool| {
                let prompt_repository = PromptRepository::new(pool.clone());
                let weekly_review_service = build_weekly_review_service(
                    pool.clone(),
                    &prompt_repository,
                    weekly_review_config.clone(),
                )
                .ok()
                .flatten()?;
                Some(WeeklyReviewDeliveryWorker::new(
                    JournalRepository::new(pool.clone()),
                    crate::journal::week_review::repository::WeeklyReviewRepository::new(pool),
                    weekly_review_service,
                    TelegramWeeklyReviewSender::new(
                        serve_config.telegram_bot_token.clone(),
                        serve_config.telegram_allowed_user_ids.clone(),
                    ),
                    serve_config.weekly_review_delivery.clone(),
                ))
            },
            ReconciliationWorkerConfig {
                enabled: config.weekly_review_delivery.enabled,
                batch_size: 1,
                interval: config.weekly_review_delivery.interval,
            },
            shutdown.clone(),
        );
    }

    // 6. Signal Worker
    if config.signal_worker.enabled {
        spawn_sweep(
            workers,
            "signal-sweep",
            registry.clone(),
            move |_chat_id, pool| {
                let prompt_repository = PromptRepository::new(pool.clone());
                let service = build_signal_service(
                    pool.clone(),
                    &prompt_repository,
                    signal_runtime_config.clone(),
                )
                .ok()
                .flatten()?;
                let backfill = DailyReviewSignalBackfillService::new(
                    DailyReviewSignalRepository::new(pool),
                    service,
                );
                Some(DailyReviewSignalCycle::new(backfill))
            },
            config.signal_worker.clone(),
            shutdown.clone(),
        );
    }
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
