use clap::{Parser, Subcommand};

use crate::{journal::embedding::EmbeddingWorkerConfig, version};

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

    #[arg(
        long,
        env = "DATABASE_PATH",
        global = true,
        default_value = "data/froid.sqlite3"
    )]
    database_path: String,

    #[arg(long, env = "FROID_EMBEDDING_WORKER_ENABLED", global = true)]
    embedding_worker_enabled: Option<String>,

    #[arg(long, env = "FROID_EMBEDDING_WORKER_BATCH_SIZE", global = true)]
    embedding_worker_batch_size: Option<String>,

    #[arg(long, env = "FROID_EMBEDDING_WORKER_INTERVAL_SECONDS", global = true)]
    embedding_worker_interval_seconds: Option<String>,

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

        Ok(ServeConfig {
            telegram_bot_token: telegram_bot_token.clone(),
            database_path: self.database_path.clone(),
            database_url: format!("sqlite:{}", self.database_path),
            embedding_worker,
        })
    }
}

#[cfg(test)]
mod tests {
    use clap::{CommandFactory, Parser};

    use super::*;

    #[test]
    fn parses_serve_config_from_cli_flags() {
        let cli = Cli::parse_from([
            "froid",
            "--telegram-bot-token",
            "token",
            "--database-path",
            "custom.db",
            "serve",
        ]);

        let config = cli.serve_config().unwrap();

        assert_eq!(config.telegram_bot_token, "token");
        assert_eq!(config.database_path, "custom.db");
        assert_eq!(config.database_url, "sqlite:custom.db");
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
        let cli = Cli {
            telegram_bot_token: None,
            database_path: "froid.sqlite3".to_string(),
            embedding_worker_enabled: None,
            embedding_worker_batch_size: None,
            embedding_worker_interval_seconds: None,
            command: None,
        };

        assert!(matches!(cli.selected_command(), Command::Serve));
    }

    #[test]
    fn rejects_missing_telegram_bot_token() {
        let cli = Cli {
            telegram_bot_token: None,
            database_path: "froid.sqlite3".to_string(),
            embedding_worker_enabled: None,
            embedding_worker_batch_size: None,
            embedding_worker_interval_seconds: None,
            command: None,
        };

        let error = cli.serve_config().unwrap_err();

        assert_eq!(error.kind(), clap::error::ErrorKind::ValueValidation);
        assert!(
            error
                .to_string()
                .contains("TELEGRAM_BOT_TOKEN environment variable or --telegram-bot-token")
        );
    }

    #[test]
    fn rejects_empty_telegram_bot_token() {
        let cli = Cli {
            telegram_bot_token: Some("  ".to_string()),
            database_path: "froid.sqlite3".to_string(),
            embedding_worker_enabled: None,
            embedding_worker_batch_size: None,
            embedding_worker_interval_seconds: None,
            command: None,
        };

        let error = cli.serve_config().unwrap_err();

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

    fn cli_with_token(token: &str) -> Cli {
        Cli {
            telegram_bot_token: Some(token.to_string()),
            database_path: "froid.sqlite3".to_string(),
            embedding_worker_enabled: None,
            embedding_worker_batch_size: None,
            embedding_worker_interval_seconds: None,
            command: None,
        }
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
            database_path: "froid.sqlite3".to_string(),
            embedding_worker_enabled: Some("true".to_string()),
            embedding_worker_batch_size: None,
            embedding_worker_interval_seconds: None,
            command: None,
        }
        .serve_config()
        .unwrap();

        assert!(config.embedding_worker.enabled);
    }

    #[test]
    fn serve_config_rejects_zero_batch_size() {
        let error = Cli {
            telegram_bot_token: Some("token".to_string()),
            database_path: "froid.sqlite3".to_string(),
            embedding_worker_enabled: None,
            embedding_worker_batch_size: Some("0".to_string()),
            embedding_worker_interval_seconds: None,
            command: None,
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
            database_path: "froid.sqlite3".to_string(),
            embedding_worker_enabled: None,
            embedding_worker_batch_size: None,
            embedding_worker_interval_seconds: Some("0".to_string()),
            command: None,
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
}
