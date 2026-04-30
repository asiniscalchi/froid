use clap::Parser;
use froid::{
    app,
    cli::{Cli, Command},
    version,
};
use tracing::info;
use tracing_subscriber::{EnvFilter, fmt};

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();
    let cli = Cli::parse();

    init_tracing();
    info!(version = version::VERSION, "starting froid");

    match cli.selected_command() {
        Command::Serve => {
            let config = cli.serve_config()?;
            app::serve(config).await?;
        }
    }

    Ok(())
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    fmt().with_env_filter(filter).init();
}
