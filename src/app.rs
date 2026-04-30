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
        repository::JournalRepository,
        review::{DailyReviewRuntimeConfig, configure_daily_review},
        search::SemanticSearchService,
        service::JournalService,
        status::EmbeddingStatusConfig,
    },
    version,
    workers::embedding::EmbeddingReconciliationWorker,
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

    spawn_embedding_worker(&pool, &config, embedding_config.as_ref())?;
    let journal_service = build_journal_service(pool, embedding_config)?;

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

fn build_journal_service(
    pool: SqlitePool,
    embedding_config: Option<EmbeddingConfig>,
) -> Result<JournalService, Box<dyn Error>> {
    let mut journal_service = JournalService::new(JournalRepository::new(pool.clone()));

    journal_service = configure_daily_review(
        journal_service,
        pool.clone(),
        DailyReviewRuntimeConfig::from_env(),
    )?;

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
