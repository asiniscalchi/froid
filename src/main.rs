mod adapters;
mod cli;
mod echo;
mod journal;
mod messages;

use std::error::Error;

use adapters::{Adapter, telegram::TelegramAdapter};
use clap::Parser;
use cli::{Cli, Command};
use echo::EchoService;
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
    init_tracing();

    let cli = Cli::parse();

    match cli.selected_command() {
        Command::Serve => {
            let config = cli.serve_config()?;
            TelegramAdapter::new(config.telegram_bot_token, EchoService::new())
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
