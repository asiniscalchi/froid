use std::error::Error;

use sqlx::SqlitePool;
use tracing::info;

use crate::{
    adapters::{Adapter, telegram::TelegramAdapter},
    cli::ServeConfig,
    database,
    journal::{
        embedding::{
            EmbeddingBackfillService, EmbeddingConfig, RigOpenAiEmbedder, SqliteEmbeddingRepository,
        },
        extraction::{EntryExtractionRuntimeConfig, configure_entry_extraction},
        repository::JournalRepository,
        review::{DailyReviewRuntimeConfig, build_daily_review_service, configure_daily_review},
        search::SemanticSearchService,
        service::JournalService,
        status::EmbeddingStatusConfig,
    },
    version,
    workers::{
        daily_review::{DailyReviewDeliveryWorker, TelegramDailyReviewSender},
        embedding::EmbeddingReconciliationWorker,
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
    let entry_extraction_config = EntryExtractionRuntimeConfig::from_env();

    spawn_embedding_worker(&pool, &config, embedding_config.as_ref())?;
    let delivery_configured =
        spawn_daily_review_delivery_worker(&pool, &config, daily_review_config.clone())?;
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
        let worker =
            EmbeddingReconciliationWorker::new(backfill_service, config.embedding_worker.clone());
        tokio::spawn(async move { worker.run_forever().await });
    }

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

fn build_journal_service(
    pool: SqlitePool,
    embedding_config: Option<EmbeddingConfig>,
    entry_extraction_config: EntryExtractionRuntimeConfig,
    daily_review_config: DailyReviewRuntimeConfig,
    delivery_configured: bool,
) -> Result<JournalService, Box<dyn Error>> {
    let mut journal_service = JournalService::new(JournalRepository::new(pool.clone()));

    journal_service =
        configure_entry_extraction(journal_service, pool.clone(), entry_extraction_config)?;

    journal_service = configure_daily_review(journal_service, pool.clone(), daily_review_config)?;

    if delivery_configured {
        journal_service = journal_service.with_daily_review_delivery_configured();
    }

    if let Some(cfg) = embedding_config
        && let Ok(search_embedder) = RigOpenAiEmbedder::from_env(cfg.clone())
        && let Ok(capture_embedder) = RigOpenAiEmbedder::from_env(cfg.clone())
    {
        let search_index = SqliteEmbeddingRepository::new(pool.clone());
        let capture_index = SqliteEmbeddingRepository::new(pool.clone());
        let status_index = SqliteEmbeddingRepository::new(pool.clone());
        let status_config = EmbeddingStatusConfig {
            model: cfg.model,
            dimensions: cfg.dimensions,
        };
        let search =
            SemanticSearchService::new(search_index, search_embedder, JournalRepository::new(pool));
        journal_service = journal_service.with_search(search);
        journal_service = journal_service.with_capture_embedding(capture_index, capture_embedder);
        journal_service = journal_service.with_embedding_status_config(status_config);
        journal_service = journal_service.with_pending_embedding_counter(status_index);
    }

    Ok(journal_service)
}
