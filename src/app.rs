use std::error::Error;

use sqlx::SqlitePool;
use tokio_util::sync::CancellationToken;
use tracing::info;

use tracing::warn;

use crate::{
    adapters::{Adapter, telegram::TelegramAdapter},
    cli::ServeConfig,
    database,
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
    },
    version,
    workers::{
        ReconciliationWorker,
        daily_review::{DailyReviewDeliveryWorker, TelegramDailyReviewSender},
        embedding::EmbeddingCycle,
        extraction::ExtractionCycle,
        signals::DailyReviewSignalCycle,
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
    let entry_extraction_config = JournalEntryExtractionRuntimeConfig::from_env();
    let signal_runtime_config = DailyReviewSignalRuntimeConfig::from_env();

    spawn_embedding_worker(&pool, &config, embedding_config.as_ref())?;
    spawn_daily_review_embedding_worker(&pool, &config, embedding_config.as_ref())?;
    spawn_extraction_worker(&pool, &config, &entry_extraction_config)?;
    let delivery_configured =
        spawn_daily_review_delivery_worker(&pool, &config, daily_review_config.clone())?;
    spawn_signal_worker(&pool, &config, signal_runtime_config)?;
    let journal_service = build_journal_service(
        pool,
        embedding_config,
        entry_extraction_config,
        daily_review_config,
        delivery_configured,
    )?;

    TelegramAdapter::new(config.telegram_bot_token, journal_service)
        .run()
        .await;

    Ok(())
}

fn spawn_embedding_worker(
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
        tokio::spawn(async move { worker.run_forever(CancellationToken::new()).await });
    }

    Ok(())
}

fn spawn_daily_review_embedding_worker(
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
        tokio::spawn(async move { worker.run_forever(CancellationToken::new()).await });
    }

    Ok(())
}

fn spawn_extraction_worker(
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
    tokio::spawn(async move { worker.run_forever(CancellationToken::new()).await });

    Ok(())
}

fn spawn_daily_review_delivery_worker(
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
    tokio::spawn(async move { worker.run_forever().await });

    Ok(true)
}

fn spawn_signal_worker(
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
    tokio::spawn(async move { worker.run_forever(CancellationToken::new()).await });

    Ok(())
}

fn build_journal_service(
    pool: SqlitePool,
    embedding_config: Option<EmbeddingConfig>,
    entry_extraction_config: JournalEntryExtractionRuntimeConfig,
    daily_review_config: DailyReviewRuntimeConfig,
    delivery_configured: bool,
) -> Result<JournalService, Box<dyn Error>> {
    let mut journal_service = JournalService::new(JournalRepository::new(pool.clone()));

    journal_service =
        configure_journal_entry_extraction(journal_service, pool.clone(), entry_extraction_config)?;

    journal_service = configure_daily_review(journal_service, pool.clone(), daily_review_config)?;

    if delivery_configured {
        journal_service = journal_service.with_daily_review_delivery_configured();
    }

    if let Some(cfg) = embedding_config
        && let Ok(search_embedder) = RigOpenAiEmbedder::from_env(cfg.clone())
        && let Ok(capture_embedder) = RigOpenAiEmbedder::from_env(cfg.clone())
        && let Ok(review_search_embedder) = RigOpenAiEmbedder::from_env(cfg.clone())
    {
        let search_index = SqliteEmbeddingRepository::new(pool.clone());
        let capture_index = SqliteEmbeddingRepository::new(pool.clone());
        let status_index = SqliteEmbeddingRepository::new(pool.clone());
        let review_search_index =
            crate::journal::review::embedding_repository::SqliteDailyReviewEmbeddingRepository::new(
                pool.clone(),
            );
        let status_config = EmbeddingStatusConfig {
            model: cfg.model,
            dimensions: cfg.dimensions,
        };
        let search = SemanticSearchService::new(
            search_index,
            search_embedder,
            JournalRepository::new(pool.clone()),
        );
        let review_search = crate::journal::review::search::SemanticDailyReviewSearchService::new(
            review_search_index,
            review_search_embedder,
            crate::journal::review::repository::DailyReviewRepository::new(pool.clone()),
        );

        journal_service = journal_service.with_search(search);
        journal_service = journal_service.with_daily_review_search(review_search);
        journal_service = journal_service.with_capture_embedding(capture_index, capture_embedder);
        journal_service = journal_service.with_embedding_status_config(status_config);
        journal_service = journal_service.with_pending_embedding_counter(status_index);
    }

    Ok(journal_service)
}
