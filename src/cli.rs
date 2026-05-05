use std::net::SocketAddr;

use clap::{Args, Parser, Subcommand};

use crate::{
    journal::{
        review::DailyReviewDeliveryWorkerConfig, week_review::WeeklyReviewDeliveryWorkerConfig,
    },
    messages::SINGLE_USER_ID,
    version,
    workers::{ReconciliationWorkerConfig, weekly_review::weekday_from_str},
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
    embedding_worker_enabled: Option<bool>,

    #[arg(
        long,
        env = "FROID_EMBEDDING_WORKER_BATCH_SIZE",
        global = true,
        value_parser = clap::value_parser!(u32).range(1..),
    )]
    embedding_worker_batch_size: Option<u32>,

    #[arg(
        long,
        env = "FROID_EMBEDDING_WORKER_INTERVAL_SECONDS",
        global = true,
        value_parser = clap::value_parser!(u64).range(1..),
    )]
    embedding_worker_interval_seconds: Option<u64>,

    #[arg(
        long,
        env = "FROID_DAILY_REVIEW_EMBEDDING_WORKER_ENABLED",
        global = true
    )]
    daily_review_embedding_worker_enabled: Option<bool>,

    #[arg(
        long,
        env = "FROID_DAILY_REVIEW_EMBEDDING_WORKER_BATCH_SIZE",
        global = true,
        value_parser = clap::value_parser!(u32).range(1..),
    )]
    daily_review_embedding_worker_batch_size: Option<u32>,

    #[arg(
        long,
        env = "FROID_DAILY_REVIEW_EMBEDDING_WORKER_INTERVAL_SECONDS",
        global = true,
        value_parser = clap::value_parser!(u64).range(1..),
    )]
    daily_review_embedding_worker_interval_seconds: Option<u64>,

    #[arg(long, env = "FROID_EXTRACTION_WORKER_ENABLED", global = true)]
    extraction_worker_enabled: Option<bool>,

    #[arg(
        long,
        env = "FROID_EXTRACTION_WORKER_BATCH_SIZE",
        global = true,
        value_parser = clap::value_parser!(u32).range(1..),
    )]
    extraction_worker_batch_size: Option<u32>,

    #[arg(
        long,
        env = "FROID_EXTRACTION_WORKER_INTERVAL_SECONDS",
        global = true,
        value_parser = clap::value_parser!(u64).range(1..),
    )]
    extraction_worker_interval_seconds: Option<u64>,

    #[arg(long, env = "FROID_DAILY_REVIEW_DELIVERY_ENABLED", global = true)]
    daily_review_delivery_enabled: Option<bool>,

    #[arg(
        long,
        env = "FROID_DAILY_REVIEW_DELIVERY_INTERVAL_SECONDS",
        global = true,
        value_parser = clap::value_parser!(u64).range(1..),
    )]
    daily_review_delivery_interval_seconds: Option<u64>,

    #[arg(long, env = "FROID_WEEK_REVIEW_WORKER_ENABLED", global = true)]
    week_review_worker_enabled: Option<bool>,

    #[arg(
        long,
        env = "FROID_WEEK_REVIEW_WORKER_INTERVAL_SECONDS",
        global = true,
        value_parser = clap::value_parser!(u64).range(1..),
    )]
    week_review_worker_interval_seconds: Option<u64>,

    #[arg(long, env = "FROID_WEEK_REVIEW_KICKOFF_DAY", global = true)]
    week_review_kickoff_day: Option<String>,

    #[arg(
        long,
        env = "FROID_WEEK_REVIEW_MIN_DAILY_REVIEWS",
        global = true,
        value_parser = clap::value_parser!(usize),
    )]
    week_review_min_daily_reviews: Option<usize>,

    #[arg(long, env = "FROID_SIGNAL_WORKER_ENABLED", global = true)]
    signal_worker_enabled: Option<bool>,

    #[arg(
        long,
        env = "FROID_SIGNAL_WORKER_BATCH_SIZE",
        global = true,
        value_parser = clap::value_parser!(u32).range(1..),
    )]
    signal_worker_batch_size: Option<u32>,

    #[arg(
        long,
        env = "FROID_SIGNAL_WORKER_INTERVAL_SECONDS",
        global = true,
        value_parser = clap::value_parser!(u64).range(1..),
    )]
    signal_worker_interval_seconds: Option<u64>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Default, Clone, Subcommand)]
pub enum Command {
    #[default]
    Serve,
    /// Run the MCP server exposing analyzer tools over Streamable HTTP.
    Mcp(McpArgs),
}

#[derive(Debug, Clone, Args)]
pub struct McpArgs {
    /// Address to bind the HTTP server to.
    #[arg(long, env = "FROID_MCP_BIND", default_value = "127.0.0.1:8080")]
    pub bind: SocketAddr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpConfig {
    pub bind: SocketAddr,
    pub user_id: String,
    pub database_path: String,
    pub database_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServeConfig {
    pub telegram_bot_token: String,
    pub database_path: String,
    pub database_url: String,
    pub embedding_worker: ReconciliationWorkerConfig,
    pub daily_review_embedding_worker: ReconciliationWorkerConfig,
    pub extraction_worker: ReconciliationWorkerConfig,
    pub daily_review_delivery: DailyReviewDeliveryWorkerConfig,
    pub weekly_review_delivery: WeeklyReviewDeliveryWorkerConfig,
    pub signal_worker: ReconciliationWorkerConfig,
}

impl Cli {
    pub fn selected_command(&self) -> Command {
        self.command.clone().unwrap_or_default()
    }

    pub fn mcp_config(&self, args: &McpArgs) -> Result<McpConfig, clap::Error> {
        let database_path = format!("{}/{}", self.data_dir, self.database_file);
        Ok(McpConfig {
            bind: args.bind,
            user_id: SINGLE_USER_ID.to_string(),
            database_url: format!("sqlite:{database_path}"),
            database_path,
        })
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

        let embedding_worker = ReconciliationWorkerConfig::from_values(
            self.embedding_worker_enabled,
            self.embedding_worker_batch_size,
            self.embedding_worker_interval_seconds,
        );

        let daily_review_embedding_worker = ReconciliationWorkerConfig::from_values(
            self.daily_review_embedding_worker_enabled,
            self.daily_review_embedding_worker_batch_size,
            self.daily_review_embedding_worker_interval_seconds,
        );

        let extraction_worker = ReconciliationWorkerConfig::from_values(
            self.extraction_worker_enabled,
            self.extraction_worker_batch_size,
            self.extraction_worker_interval_seconds,
        );

        let daily_review_delivery = DailyReviewDeliveryWorkerConfig::from_values(
            self.daily_review_delivery_enabled,
            self.daily_review_delivery_interval_seconds,
        );

        let kickoff_weekday = match self.week_review_kickoff_day.as_deref() {
            Some(value) => match weekday_from_str(value) {
                Some(day) => Some(day),
                None => {
                    return Err(clap::Error::raw(
                        clap::error::ErrorKind::ValueValidation,
                        format!(
                            "FROID_WEEK_REVIEW_KICKOFF_DAY must be a weekday name (e.g. Monday); got {value:?}"
                        ),
                    ));
                }
            },
            None => None,
        };

        let weekly_review_delivery = WeeklyReviewDeliveryWorkerConfig::from_values(
            self.week_review_worker_enabled,
            self.week_review_worker_interval_seconds,
            kickoff_weekday,
            self.week_review_min_daily_reviews,
        );

        let signal_worker = ReconciliationWorkerConfig::from_values(
            self.signal_worker_enabled,
            self.signal_worker_batch_size,
            self.signal_worker_interval_seconds,
        );

        let database_path = format!("{}/{}", self.data_dir, self.database_file);

        Ok(ServeConfig {
            telegram_bot_token: telegram_bot_token.clone(),
            database_url: format!("sqlite:{database_path}"),
            database_path,
            embedding_worker,
            daily_review_embedding_worker,
            extraction_worker,
            daily_review_delivery,
            weekly_review_delivery,
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
            daily_review_embedding_worker_enabled: None,
            daily_review_embedding_worker_batch_size: None,
            daily_review_embedding_worker_interval_seconds: None,
            extraction_worker_enabled: None,
            extraction_worker_batch_size: None,
            extraction_worker_interval_seconds: None,
            daily_review_delivery_enabled: None,
            daily_review_delivery_interval_seconds: None,
            week_review_worker_enabled: None,
            week_review_worker_interval_seconds: None,
            week_review_kickoff_day: None,
            week_review_min_daily_reviews: None,
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
    fn serve_config_worker_enabled_when_flag_set() {
        let cli = Cli::parse_from([
            "froid",
            "--telegram-bot-token",
            "token",
            "--embedding-worker-enabled",
            "true",
        ]);

        let config = cli.serve_config().unwrap();

        assert!(config.embedding_worker.enabled);
    }

    #[test]
    fn parse_rejects_zero_batch_size() {
        let error = Cli::try_parse_from([
            "froid",
            "--telegram-bot-token",
            "token",
            "--embedding-worker-batch-size",
            "0",
        ])
        .unwrap_err();

        assert_eq!(error.kind(), clap::error::ErrorKind::ValueValidation);
    }

    #[test]
    fn parse_rejects_non_numeric_batch_size() {
        let error = Cli::try_parse_from([
            "froid",
            "--telegram-bot-token",
            "token",
            "--embedding-worker-batch-size",
            "abc",
        ])
        .unwrap_err();

        assert_eq!(error.kind(), clap::error::ErrorKind::ValueValidation);
    }

    #[test]
    fn parse_rejects_non_bool_enabled_value() {
        let error = Cli::try_parse_from([
            "froid",
            "--telegram-bot-token",
            "token",
            "--embedding-worker-enabled",
            "yes",
        ])
        .unwrap_err();

        assert_eq!(error.kind(), clap::error::ErrorKind::InvalidValue);
    }

    #[test]
    fn parse_rejects_zero_interval() {
        let error = Cli::try_parse_from([
            "froid",
            "--telegram-bot-token",
            "token",
            "--embedding-worker-interval-seconds",
            "0",
        ])
        .unwrap_err();

        assert_eq!(error.kind(), clap::error::ErrorKind::ValueValidation);
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
    fn serve_config_daily_review_delivery_enabled_when_flag_set() {
        let cli = Cli::parse_from([
            "froid",
            "--telegram-bot-token",
            "token",
            "--daily-review-delivery-enabled",
            "true",
        ]);

        let config = cli.serve_config().unwrap();

        assert!(config.daily_review_delivery.enabled);
    }

    #[test]
    fn parse_rejects_zero_daily_review_delivery_interval() {
        let error = Cli::try_parse_from([
            "froid",
            "--telegram-bot-token",
            "token",
            "--daily-review-delivery-interval-seconds",
            "0",
        ])
        .unwrap_err();

        assert_eq!(error.kind(), clap::error::ErrorKind::ValueValidation);
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
    fn serve_config_extraction_worker_enabled_when_flag_set() {
        let cli = Cli::parse_from([
            "froid",
            "--telegram-bot-token",
            "token",
            "--extraction-worker-enabled",
            "true",
        ]);

        let config = cli.serve_config().unwrap();

        assert!(config.extraction_worker.enabled);
    }

    #[test]
    fn parse_rejects_zero_extraction_worker_batch_size() {
        let error = Cli::try_parse_from([
            "froid",
            "--telegram-bot-token",
            "token",
            "--extraction-worker-batch-size",
            "0",
        ])
        .unwrap_err();

        assert_eq!(error.kind(), clap::error::ErrorKind::ValueValidation);
    }

    #[test]
    fn parse_rejects_zero_extraction_worker_interval() {
        let error = Cli::try_parse_from([
            "froid",
            "--telegram-bot-token",
            "token",
            "--extraction-worker-interval-seconds",
            "0",
        ])
        .unwrap_err();

        assert_eq!(error.kind(), clap::error::ErrorKind::ValueValidation);
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
    fn serve_config_signal_worker_enabled_when_flag_set() {
        let cli = Cli::parse_from([
            "froid",
            "--telegram-bot-token",
            "token",
            "--signal-worker-enabled",
            "true",
        ]);

        let config = cli.serve_config().unwrap();

        assert!(config.signal_worker.enabled);
    }

    #[test]
    fn parse_rejects_zero_signal_worker_batch_size() {
        let error = Cli::try_parse_from([
            "froid",
            "--telegram-bot-token",
            "token",
            "--signal-worker-batch-size",
            "0",
        ])
        .unwrap_err();

        assert_eq!(error.kind(), clap::error::ErrorKind::ValueValidation);
    }

    #[test]
    fn parses_mcp_command_with_bind_and_single_user_scope() {
        let cli = Cli::parse_from(["froid", "mcp", "--bind", "0.0.0.0:9000"]);

        let Command::Mcp(args) = cli.selected_command() else {
            panic!("expected Mcp command");
        };
        let config = cli.mcp_config(&args).unwrap();

        assert_eq!(config.bind.to_string(), "0.0.0.0:9000");
        assert_eq!(config.user_id, SINGLE_USER_ID);
        assert_eq!(config.database_path, "data/froid.sqlite3");
        assert_eq!(config.database_url, "sqlite:data/froid.sqlite3");
    }

    #[test]
    fn mcp_command_uses_default_bind() {
        let cli = Cli::parse_from(["froid", "mcp"]);
        let Command::Mcp(args) = cli.selected_command() else {
            panic!("expected Mcp command");
        };
        let config = cli.mcp_config(&args).unwrap();
        assert_eq!(config.bind.to_string(), "127.0.0.1:8080");
        assert_eq!(config.user_id, SINGLE_USER_ID);
    }

    #[test]
    fn parse_rejects_zero_signal_worker_interval() {
        let error = Cli::try_parse_from([
            "froid",
            "--telegram-bot-token",
            "token",
            "--signal-worker-interval-seconds",
            "0",
        ])
        .unwrap_err();

        assert_eq!(error.kind(), clap::error::ErrorKind::ValueValidation);
    }
}
