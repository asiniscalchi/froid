mod adapters;
mod cli;
mod database;
mod handler;
mod journal;
mod messages;
mod version;

use std::error::Error;

use adapters::{Adapter, telegram::TelegramAdapter};
use clap::Parser;
use cli::{Cli, Command};
use journal::{repository::JournalRepository, service::JournalService};
use tracing::info;
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

            let journal_service = JournalService::new(JournalRepository::new(pool));

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
