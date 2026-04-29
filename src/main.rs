use std::{env, error::Error};

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
            DailyReviewPromptConfig, ReviewConfig, RigOpenAiReviewGenerator,
            repository::DailyReviewRepository, service::DailyReviewService,
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
            let journal_service = build_journal_service(pool, embedding_config)?;

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
) -> Result<JournalService, Box<dyn Error>> {
    let mut journal_service = JournalService::new(JournalRepository::new(pool.clone()));

    journal_service = configure_daily_review(
        journal_service,
        pool.clone(),
        env::var("OPENAI_API_KEY").ok(),
    )?;

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

    Ok(journal_service)
}

fn configure_daily_review(
    journal_service: JournalService,
    pool: SqlitePool,
    openai_api_key: Option<String>,
) -> Result<JournalService, Box<dyn Error>> {
    let Some(openai_api_key) = openai_api_key.filter(|value| !value.trim().is_empty()) else {
        warn!("daily review generation is not configured");
        return Ok(journal_service);
    };

    let review_prompt = DailyReviewPromptConfig::from_env().load()?;
    let review_generator = RigOpenAiReviewGenerator::from_optional_api_key(
        ReviewConfig::from_env(),
        review_prompt,
        Some(openai_api_key),
    )?;
    let daily_review_service = DailyReviewService::new(
        DailyReviewRepository::new(pool.clone()),
        JournalRepository::new(pool.clone()),
        review_generator,
    );

    Ok(journal_service.with_daily_review_runner(daily_review_service))
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    fmt().with_env_filter(filter).init();
}

#[cfg(test)]
mod tests {
    use std::sync::{Mutex, OnceLock};

    use froid::{
        database,
        journal::{
            command::{JournalCommand, JournalCommandRequest},
            review::prompt::DEFAULT_REVIEW_PROMPT_PATH,
        },
    };

    use super::*;

    static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    #[tokio::test]
    async fn missing_prompt_file_does_not_break_startup_without_review_api_key() {
        let _guard = env_lock();
        let original_path = set_env_var("FROID_REVIEW_PROMPT_PATH", "missing-review-prompt.md");

        let pool = setup_pool().await;
        let service = configure_daily_review(
            JournalService::new(JournalRepository::new(pool.clone())),
            pool,
            None,
        )
        .unwrap();
        let response = service
            .command(&JournalCommandRequest {
                user_id: "7".to_string(),
                received_at: chrono::Utc::now(),
                command: JournalCommand::ReviewToday,
            })
            .await
            .unwrap();

        assert_eq!(
            response.text,
            "Daily review generation is not configured yet."
        );

        restore_env_var("FROID_REVIEW_PROMPT_PATH", original_path);
    }

    #[tokio::test]
    async fn missing_prompt_file_fails_startup_when_review_api_key_is_configured() {
        let _guard = env_lock();
        let original_path = set_env_var("FROID_REVIEW_PROMPT_PATH", "missing-review-prompt.md");

        let pool = setup_pool().await;
        let error = configure_daily_review(
            JournalService::new(JournalRepository::new(pool.clone())),
            pool,
            Some("test-api-key".to_string()),
        )
        .err()
        .unwrap();

        assert!(
            error
                .to_string()
                .contains("failed to load daily review prompt")
        );

        restore_env_var("FROID_REVIEW_PROMPT_PATH", original_path);
    }

    #[tokio::test]
    async fn default_prompt_file_allows_startup_when_review_api_key_is_configured() {
        let _guard = env_lock();
        let original_path = set_env_var("FROID_REVIEW_PROMPT_PATH", DEFAULT_REVIEW_PROMPT_PATH);

        let pool = setup_pool().await;
        configure_daily_review(
            JournalService::new(JournalRepository::new(pool.clone())),
            pool,
            Some("test-api-key".to_string()),
        )
        .unwrap();

        restore_env_var("FROID_REVIEW_PROMPT_PATH", original_path);
    }

    async fn setup_pool() -> SqlitePool {
        database::register_sqlite_vec_extension();
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        pool
    }

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
    }

    fn set_env_var(key: &str, value: &str) -> Option<String> {
        let original = env::var(key).ok();
        unsafe {
            env::set_var(key, value);
        }
        original
    }

    fn restore_env_var(key: &str, original: Option<String>) {
        unsafe {
            match original {
                Some(value) => env::set_var(key, value),
                None => env::remove_var(key),
            }
        }
    }
}
