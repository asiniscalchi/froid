mod adapters;
mod cli;
mod database;
mod handler;
mod journal;
mod messages;
mod version;
mod workers;

use std::error::Error;

use adapters::{Adapter, telegram::TelegramAdapter};
use clap::Parser;
use cli::{Cli, Command};
use journal::{
    embedding::{
        EmbeddingBackfillService, EmbeddingConfig, RigOpenAiEmbedder, SqliteEmbeddingRepository,
    },
    repository::JournalRepository,
    search::SemanticSearchService,
    service::JournalService,
};
use tracing::info;
use tracing_subscriber::{EnvFilter, fmt};
use workers::embedding::EmbeddingReconciliationWorker;

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

            if config.embedding_worker.enabled
                && let Some(ref cfg) = embedding_config
            {
                let embedder = RigOpenAiEmbedder::from_env(cfg.clone())?;
                let index = SqliteEmbeddingRepository::new(pool.clone());
                let backfill_service = EmbeddingBackfillService::new(index, embedder);
                let worker =
                    EmbeddingReconciliationWorker::new(backfill_service, config.embedding_worker);
                tokio::spawn(async move { worker.run_forever().await });
            }

            let mut journal_service = JournalService::new(JournalRepository::new(pool.clone()));

            if let Some(cfg) = embedding_config
                && let Ok(embedder) = RigOpenAiEmbedder::from_env(cfg)
            {
                let index = SqliteEmbeddingRepository::new(pool.clone());
                let search =
                    SemanticSearchService::new(index, embedder, JournalRepository::new(pool));
                journal_service = journal_service.with_search(search);
            }

            TelegramAdapter::new(config.telegram_bot_token, journal_service)
                .run()
                .await;
        }
    }

    Ok(())
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    fmt().with_env_filter(filter).init();
}
