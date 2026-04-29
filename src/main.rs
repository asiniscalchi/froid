use std::error::Error;

use clap::Parser;
use froid::{
    adapters::{Adapter, telegram::TelegramAdapter},
    cli::{Cli, Command, ServeConfig},
    database,
    journal::{
        embedding::{
            EmbeddingBackfillService, EmbeddingConfig, RigOpenAiEmbedder, SqliteEmbeddingRepository,
        },
        repository::JournalRepository,
        review::{
            RigOpenAiReviewGenerator, repository::DailyReviewRepository,
            service::DailyReviewService,
        },
        search::SemanticSearchService,
        service::JournalService,
    },
    version,
    workers::embedding::EmbeddingReconciliationWorker,
};
use sqlx::SqlitePool;
use tracing::{info, warn};
use tracing_subscriber::{EnvFilter, fmt};

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn Error>> {
    dotenvy::dotenv().ok();
    let cli = Cli::parse();

    init_tracing();
    info!(version = version::VERSION, "starting froid");

    match cli.selected_command() {
        Command::Serve => {
            let config = cli.serve_config()?;
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
            let journal_service = build_journal_service(pool, embedding_config);

            TelegramAdapter::new(config.telegram_bot_token, journal_service)
                .run()
                .await;
        }
    }

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
) -> JournalService {
    let mut journal_service = JournalService::new(JournalRepository::new(pool.clone()));

    if let Ok(review_generator) = RigOpenAiReviewGenerator::from_env() {
        let daily_review_service = DailyReviewService::new(
            DailyReviewRepository::new(pool.clone()),
            JournalRepository::new(pool.clone()),
            review_generator,
        );
        journal_service = journal_service.with_daily_review_runner(daily_review_service);
    } else {
        warn!("daily review generation is not configured");
    }

    if let Some(cfg) = embedding_config
        && let Ok(search_embedder) = RigOpenAiEmbedder::from_env(cfg.clone())
        && let Ok(capture_embedder) = RigOpenAiEmbedder::from_env(cfg)
    {
        let search_index = SqliteEmbeddingRepository::new(pool.clone());
        let capture_index = SqliteEmbeddingRepository::new(pool.clone());
        let search =
            SemanticSearchService::new(search_index, search_embedder, JournalRepository::new(pool));
        journal_service = journal_service.with_search(search);
        journal_service = journal_service.with_capture_embedding(capture_index, capture_embedder);
    }

    journal_service
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    fmt().with_env_filter(filter).init();
}
