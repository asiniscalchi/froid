mod adapters;
mod config;
mod echo;
mod messages;

use std::error::Error;

use config::AppConfig;
use echo::EchoService;
use tracing_subscriber::{EnvFilter, fmt};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    init_tracing();

    let config = AppConfig::from_env()?;
    adapters::telegram::run(config.telegram_bot_token, EchoService::new()).await;

    Ok(())
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    fmt().with_env_filter(filter).init();
}
