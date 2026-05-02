use clap::{Parser, Subcommand};

use crate::{
    journal::{
        embedding::EmbeddingWorkerConfig,
        extraction::ExtractionWorkerConfig,
        review::{
            DailyReviewDeliveryWorkerConfig, signals::worker_config::DailyReviewSignalWorkerConfig,
        },
    },
    version,
};

#[derive(Debug, Parser)]
#[command(version = version::VERSION, about)]
pub struct Cli {
    #[arg(
        long,
        env = "TELEGRAM_BOT_TOKEN",
        global = true,
        hide_env_values = true
    )]
    telegram_bot_token: Option<String>,

    #[arg(long, env = "DATA_DIR", global = true, default_value = "data")]
    data_dir: String,

    #[arg(
        long,
        env = "DATABASE_FILE",
        global = true,
        default_value = "froid.sqlite3"
    )]
    database_file: String,

    #[arg(long, env = "FROID_EMBEDDING_WORKER_ENABLED", global = true)]
    embedding_worker_enabled: Option<String>,

    #[arg(long, env = "FROID_EMBEDDING_WORKER_BATCH_SIZE", global = true)]
    embedding_worker_batch_size: Option<String>,

    #[arg(long, env = "FROID_EMBEDDING_WORKER_INTERVAL_SECONDS", global = true)]
    embedding_worker_interval_seconds: Option<String>,

    #[arg(long, env = "FROID_EXTRACTION_WORKER_ENABLED", global = true)]
    extraction_worker_enabled: Option<String>,

    #[arg(long, env = "FROID_EXTRACTION_WORKER_BATCH_SIZE", global = true)]
    extraction_worker_batch_size: Option<String>,

    #[arg(long, env = "FROID_EXTRACTION_WORKER_INTERVAL_SECONDS", global = true)]
    extraction_worker_interval_seconds: Option<String>,

    #[arg(long, env = "FROID_DAILY_REVIEW_DELIVERY_ENABLED", global = true)]
    daily_review_delivery_enabled: Option<String>,

    #[arg(
        long,
        env = "FROID_DAILY_REVIEW_DELIVERY_INTERVAL_SECONDS",
        global = true
    )]
    daily_review_delivery_interval_seconds: Option<String>,

    #[arg(long, env = "FROID_SIGNAL_WORKER_ENABLED", global = true)]
    signal_worker_enabled: Option<String>,

    #[arg(long, env = "FROID_SIGNAL_WORKER_BATCH_SIZE", global = true)]
    signal_worker_batch_size: Option<String>,

    #[arg(long, env = "FROID_SIGNAL_WORKER_INTERVAL_SECONDS", global = true)]
    signal_worker_interval_seconds: Option<String>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Default, Clone, Copy, Subcommand)]
pub enum Command {
    #[default]
    Serve,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServeConfig {
    pub telegram_bot_token: String,
    pub database_path: String,
    pub database_url: String,
    pub embedding_worker: EmbeddingWorkerConfig,
    pub extraction_worker: ExtractionWorkerConfig,
    pub daily_review_delivery: DailyReviewDeliveryWorkerConfig,
    pub signal_worker: DailyReviewSignalWorkerConfig,
}

impl Cli {
    pub fn selected_command(&self) -> Command {
        self.command.unwrap_or_default()
    }

    pub fn serve_config(&self) -> Result<ServeConfig, clap::Error> {
        let Some(telegram_bot_token) = self.telegram_bot_token.as_ref() else {
            return Err(clap::Error::raw(
                clap::error::ErrorKind::ValueValidation,
                "TELEGRAM_BOT_TOKEN environment variable or --telegram-bot-token is required",
            ));
        };

        if telegram_bot_token.trim().is_empty() {
            return Err(clap::Error::raw(
                clap::error::ErrorKind::ValueValidation,
                "TELEGRAM_BOT_TOKEN environment variable or --telegram-bot-token must not be empty",
            ));
        }

        let embedding_worker = EmbeddingWorkerConfig::from_values(
            self.embedding_worker_enabled.clone(),
            self.embedding_worker_batch_size.clone(),
            self.embedding_worker_interval_seconds.clone(),
        )
        .map_err(|e| clap::Error::raw(clap::error::ErrorKind::ValueValidation, e.to_string()))?;

        let extraction_worker = ExtractionWorkerConfig::from_values(
            self.extraction_worker_enabled.clone(),
            self.extraction_worker_batch_size.clone(),
            self.extraction_worker_interval_seconds.clone(),
        )
        .map_err(|e| clap::Error::raw(clap::error::ErrorKind::ValueValidation, e.to_string()))?;

        let daily_review_delivery = DailyReviewDeliveryWorkerConfig::from_values(
            self.daily_review_delivery_enabled.clone(),
            self.daily_review_delivery_interval_seconds.clone(),
        )
        .map_err(|e| clap::Error::raw(clap::error::ErrorKind::ValueValidation, e.to_string()))?;

        let signal_worker = DailyReviewSignalWorkerConfig::from_values(
            self.signal_worker_enabled.clone(),
            self.signal_worker_batch_size.clone(),
            self.signal_worker_interval_seconds.clone(),
        )
        .map_err(|e| clap::Error::raw(clap::error::ErrorKind::ValueValidation, e.to_string()))?;

        let database_path = format!("{}/{}", self.data_dir, self.database_file);

        Ok(ServeConfig {
            telegram_bot_token: telegram_bot_token.clone(),
            database_url: format!("sqlite:{database_path}"),
            database_path,
            embedding_worker,
            extraction_worker,
            daily_review_delivery,
            signal_worker,
        })
    }
}

#[cfg(test)]
mod tests {
    use clap::{CommandFactory, Parser};

    use super::*;

    fn default_cli() -> Cli {
        Cli {
            telegram_bot_token: None,
            data_dir: "data".to_string(),
            database_file: "froid.sqlite3".to_string(),
            embedding_worker_enabled: None,
            embedding_worker_batch_size: None,
            embedding_worker_interval_seconds: None,
            extraction_worker_enabled: None,
            extraction_worker_batch_size: None,
            extraction_worker_interval_seconds: None,
            daily_review_delivery_enabled: None,
            daily_review_delivery_interval_seconds: None,
            signal_worker_enabled: None,
            signal_worker_batch_size: None,
            signal_worker_interval_seconds: None,
            command: None,
        }
    }

    fn cli_with_token(token: &str) -> Cli {
        Cli {
            telegram_bot_token: Some(token.to_string()),
            ..default_cli()
        }
    }

    #[test]
    fn parses_serve_config_from_cli_flags() {
        let cli = Cli::parse_from([
            "froid",
            "--telegram-bot-token",
            "token",
            "--data-dir",
            "custom",
            "--database-file",
            "app.db",
            "serve",
        ]);

        let config = cli.serve_config().unwrap();

        assert_eq!(config.telegram_bot_token, "token");
        assert_eq!(config.database_path, "custom/app.db");
        assert_eq!(config.database_url, "sqlite:custom/app.db");
    }

    #[test]
    fn uses_default_database_path() {
        let cli = Cli::parse_from(["froid", "--telegram-bot-token", "token"]);

        let config = cli.serve_config().unwrap();

        assert_eq!(config.database_path, "data/froid.sqlite3");
        assert_eq!(config.database_url, "sqlite:data/froid.sqlite3");
    }

    #[test]
    fn defaults_to_serve_command() {
        assert!(matches!(default_cli().selected_command(), Command::Serve));
    }

    #[test]
    fn rejects_missing_telegram_bot_token() {
        let error = default_cli().serve_config().unwrap_err();

        assert_eq!(error.kind(), clap::error::ErrorKind::ValueValidation);
        assert!(
            error
                .to_string()
                .contains("TELEGRAM_BOT_TOKEN environment variable or --telegram-bot-token")
        );
    }

    #[test]
    fn rejects_empty_telegram_bot_token() {
        let error = Cli {
            telegram_bot_token: Some("  ".to_string()),
            ..default_cli()
        }
        .serve_config()
        .unwrap_err();

        assert_eq!(error.kind(), clap::error::ErrorKind::ValueValidation);
        assert!(error.to_string().contains("must not be empty"));
    }

    #[test]
    fn command_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn command_version_uses_build_version() {
        assert_eq!(Cli::command().get_version(), Some(version::VERSION));
    }

    #[test]
    fn serve_config_worker_disabled_by_default() {
        let config = cli_with_token("token").serve_config().unwrap();

        assert!(!config.embedding_worker.enabled);
    }

    #[test]
    fn serve_config_worker_defaults_to_batch_size_20_and_interval_300s() {
        let config = cli_with_token("token").serve_config().unwrap();

        assert_eq!(config.embedding_worker.batch_size, 20);
        assert_eq!(
            config.embedding_worker.interval,
            std::time::Duration::from_secs(300)
        );
    }

    #[test]
    fn serve_config_worker_enabled_when_set_to_true() {
        let config = Cli {
            telegram_bot_token: Some("token".to_string()),
            embedding_worker_enabled: Some("true".to_string()),
            ..default_cli()
        }
        .serve_config()
        .unwrap();

        assert!(config.embedding_worker.enabled);
    }

    #[test]
    fn serve_config_rejects_zero_batch_size() {
        let error = Cli {
            telegram_bot_token: Some("token".to_string()),
            embedding_worker_batch_size: Some("0".to_string()),
            ..default_cli()
        }
        .serve_config()
        .unwrap_err();

        assert_eq!(error.kind(), clap::error::ErrorKind::ValueValidation);
        assert!(
            error
                .to_string()
                .contains("FROID_EMBEDDING_WORKER_BATCH_SIZE")
        );
    }

    #[test]
    fn serve_config_rejects_zero_interval() {
        let error = Cli {
            telegram_bot_token: Some("token".to_string()),
            embedding_worker_interval_seconds: Some("0".to_string()),
            ..default_cli()
        }
        .serve_config()
        .unwrap_err();

        assert_eq!(error.kind(), clap::error::ErrorKind::ValueValidation);
        assert!(
            error
                .to_string()
                .contains("FROID_EMBEDDING_WORKER_INTERVAL_SECONDS")
        );
    }

    #[test]
    fn serve_config_daily_review_delivery_disabled_by_default() {
        let config = cli_with_token("token").serve_config().unwrap();

        assert!(!config.daily_review_delivery.enabled);
    }

    #[test]
    fn serve_config_daily_review_delivery_defaults_to_interval_300s() {
        let config = cli_with_token("token").serve_config().unwrap();

        assert_eq!(
            config.daily_review_delivery.interval,
            std::time::Duration::from_secs(300)
        );
    }

    #[test]
    fn serve_config_daily_review_delivery_enabled_when_set_to_true() {
        let config = Cli {
            telegram_bot_token: Some("token".to_string()),
            daily_review_delivery_enabled: Some("true".to_string()),
            ..default_cli()
        }
        .serve_config()
        .unwrap();

        assert!(config.daily_review_delivery.enabled);
    }

    #[test]
    fn serve_config_rejects_zero_daily_review_delivery_interval() {
        let error = Cli {
            telegram_bot_token: Some("token".to_string()),
            daily_review_delivery_interval_seconds: Some("0".to_string()),
            ..default_cli()
        }
        .serve_config()
        .unwrap_err();

        assert_eq!(error.kind(), clap::error::ErrorKind::ValueValidation);
        assert!(
            error
                .to_string()
                .contains("FROID_DAILY_REVIEW_DELIVERY_INTERVAL_SECONDS")
        );
    }

    #[test]
    fn serve_config_extraction_worker_disabled_by_default() {
        let config = cli_with_token("token").serve_config().unwrap();

        assert!(!config.extraction_worker.enabled);
    }

    #[test]
    fn serve_config_extraction_worker_defaults_to_batch_size_20_and_interval_300s() {
        let config = cli_with_token("token").serve_config().unwrap();

        assert_eq!(config.extraction_worker.batch_size, 20);
        assert_eq!(
            config.extraction_worker.interval,
            std::time::Duration::from_secs(300)
        );
    }

    #[test]
    fn serve_config_extraction_worker_enabled_when_set_to_true() {
        let config = Cli {
            telegram_bot_token: Some("token".to_string()),
            extraction_worker_enabled: Some("true".to_string()),
            ..default_cli()
        }
        .serve_config()
        .unwrap();

        assert!(config.extraction_worker.enabled);
    }

    #[test]
    fn serve_config_rejects_zero_extraction_worker_batch_size() {
        let error = Cli {
            telegram_bot_token: Some("token".to_string()),
            extraction_worker_batch_size: Some("0".to_string()),
            ..default_cli()
        }
        .serve_config()
        .unwrap_err();

        assert_eq!(error.kind(), clap::error::ErrorKind::ValueValidation);
        assert!(
            error
                .to_string()
                .contains("FROID_EXTRACTION_WORKER_BATCH_SIZE")
        );
    }

    #[test]
    fn serve_config_rejects_zero_extraction_worker_interval() {
        let error = Cli {
            telegram_bot_token: Some("token".to_string()),
            extraction_worker_interval_seconds: Some("0".to_string()),
            ..default_cli()
        }
        .serve_config()
        .unwrap_err();

        assert_eq!(error.kind(), clap::error::ErrorKind::ValueValidation);
        assert!(
            error
                .to_string()
                .contains("FROID_EXTRACTION_WORKER_INTERVAL_SECONDS")
        );
    }

    #[test]
    fn serve_config_signal_worker_disabled_by_default() {
        let config = cli_with_token("token").serve_config().unwrap();

        assert!(!config.signal_worker.enabled);
    }

    #[test]
    fn serve_config_signal_worker_defaults_to_batch_size_20_and_interval_300s() {
        let config = cli_with_token("token").serve_config().unwrap();

        assert_eq!(config.signal_worker.batch_size, 20);
        assert_eq!(
            config.signal_worker.interval,
            std::time::Duration::from_secs(300)
        );
    }

    #[test]
    fn serve_config_signal_worker_enabled_when_set_to_true() {
        let config = Cli {
            telegram_bot_token: Some("token".to_string()),
            signal_worker_enabled: Some("true".to_string()),
            ..default_cli()
        }
        .serve_config()
        .unwrap();

        assert!(config.signal_worker.enabled);
    }

    #[test]
    fn serve_config_rejects_zero_signal_worker_batch_size() {
        let error = Cli {
            telegram_bot_token: Some("token".to_string()),
            signal_worker_batch_size: Some("0".to_string()),
            ..default_cli()
        }
        .serve_config()
        .unwrap_err();

        assert_eq!(error.kind(), clap::error::ErrorKind::ValueValidation);
        assert!(error.to_string().contains("FROID_SIGNAL_WORKER_BATCH_SIZE"));
    }

    #[test]
    fn serve_config_rejects_zero_signal_worker_interval() {
        let error = Cli {
            telegram_bot_token: Some("token".to_string()),
            signal_worker_interval_seconds: Some("0".to_string()),
            ..default_cli()
        }
        .serve_config()
        .unwrap_err();

        assert_eq!(error.kind(), clap::error::ErrorKind::ValueValidation);
        assert!(
            error
                .to_string()
                .contains("FROID_SIGNAL_WORKER_INTERVAL_SECONDS")
        );
    }
}
