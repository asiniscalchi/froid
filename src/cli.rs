use std::net::SocketAddr;
use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::{
    journal::{
        embedding::EmbeddingConfig,
        extraction::{
            JournalEntryExtractionConfig, JournalEntryExtractionPromptConfig,
            JournalEntryExtractionRuntimeConfig,
        },
        review::{
            DailyReviewDeliveryWorkerConfig, DailyReviewPromptConfig, DailyReviewRuntimeConfig,
            ReviewConfig,
            signals::{
                generator::DailyReviewSignalConfig, prompt::DailyReviewSignalPromptConfig,
                wiring::DailyReviewSignalRuntimeConfig,
            },
        },
        review_models::ModelOverride,
        week_review::{
            WeeklyReviewDeliveryWorkerConfig, WeeklyReviewRuntimeConfig,
            generator::WeeklyReviewConfig, prompt::WeeklyReviewPromptConfig,
            service::DEFAULT_MIN_DAILY_REVIEWS,
        },
    },
    version,
    workers::{ReconciliationWorkerConfig, weekly_review::weekday_from_str},
};

#[derive(Debug, Parser)]
#[command(version = version::VERSION, about)]
pub struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    #[arg(
        long,
        env = "TELEGRAM_BOT_TOKEN",
        global = true,
        hide_env_values = true
    )]
    telegram_bot_token: Option<String>,

    #[arg(
        long,
        env = "TELEGRAM_ALLOWED_USER_IDS",
        global = true,
        value_delimiter = ','
    )]
    telegram_allowed_user_ids: Option<Vec<u64>>,

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

    #[arg(long, env = "FROID_MCP_ENABLED", global = true)]
    mcp_enabled: Option<bool>,

    /// OpenAI API key used for embeddings, reviews, and extractions
    #[arg(long, env = "OPENAI_API_KEY", global = true, hide_env_values = true)]
    openai_api_key: Option<String>,

    #[arg(long, env = "FROID_EMBEDDING_MODEL", global = true)]
    embedding_model: Option<String>,

    #[arg(long, env = "FROID_REVIEW_MODEL", global = true)]
    review_model: Option<String>,

    #[arg(long, env = "FROID_REVIEW_PROMPT_PATH", global = true)]
    review_prompt_path: Option<String>,

    #[arg(long, env = "FROID_WEEK_REVIEW_MODEL", global = true)]
    week_review_model: Option<String>,

    #[arg(long, env = "FROID_WEEK_REVIEW_PROMPT_PATH", global = true)]
    week_review_prompt_path: Option<String>,

    #[arg(long, env = "FROID_ENTRY_EXTRACTION_MODEL", global = true)]
    entry_extraction_model: Option<String>,

    #[arg(long, env = "FROID_ENTRY_EXTRACTION_PROMPT_PATH", global = true)]
    entry_extraction_prompt_path: Option<String>,

    #[arg(long, env = "FROID_SIGNAL_EXTRACTION_MODEL", global = true)]
    signal_extraction_model: Option<String>,

    #[arg(long, env = "FROID_SIGNAL_EXTRACTION_PROMPT_PATH", global = true)]
    signal_extraction_prompt_path: Option<String>,

    #[arg(
        long,
        env = "FROID_MCP_BIND",
        global = true,
        default_value = "127.0.0.1:8080"
    )]
    mcp_bind: SocketAddr,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run the Telegram bot, background workers, and HTTP listener (default)
    Serve,
    /// Manage per-user journal databases (run while the server is stopped)
    Users {
        #[command(subcommand)]
        command: UsersCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum UsersCommand {
    /// List the per-user journal databases on disk
    List,
    /// Permanently delete a user's journal database (their entire journal)
    Delete {
        /// Telegram chat id of the user to delete
        chat_id: String,
        /// Confirm the irreversible deletion
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServerConfig {
    pub enabled: bool,
    pub bind: SocketAddr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServeConfig {
    pub telegram_bot_token: String,
    pub telegram_allowed_user_ids: Option<Vec<u64>>,
    pub database_path: String,
    pub database_url: String,
    pub embedding_worker: ReconciliationWorkerConfig,
    pub daily_review_embedding_worker: ReconciliationWorkerConfig,
    pub extraction_worker: ReconciliationWorkerConfig,
    pub daily_review_delivery: DailyReviewDeliveryWorkerConfig,
    pub weekly_review_delivery: WeeklyReviewDeliveryWorkerConfig,
    pub signal_worker: ReconciliationWorkerConfig,
    pub mcp_server: McpServerConfig,
    pub openai_api_key: Option<String>,
    pub embedding: EmbeddingConfig,
    pub daily_review: DailyReviewRuntimeConfig,
    pub weekly_review: WeeklyReviewRuntimeConfig,
    pub entry_extraction: JournalEntryExtractionRuntimeConfig,
    pub signal_runtime: DailyReviewSignalRuntimeConfig,
}

impl ServeConfig {
    /// The OpenAI API key, when set to a non-blank value. LLM-backed
    /// features (semantic search, reviews, extractions) require it.
    pub fn openai_api_key(&self) -> Option<&str> {
        self.openai_api_key
            .as_deref()
            .map(str::trim)
            .filter(|key| !key.is_empty())
    }
}

impl Cli {
    pub fn subcommand(&self) -> Option<&Command> {
        self.command.as_ref()
    }

    /// Directory holding the per-user journal databases, mirroring the layout
    /// used by the journal service registry (`<data dir>/journals`).
    pub fn journals_dir(&self) -> PathBuf {
        let database_path = format!("{}/{}", self.data_dir, self.database_file);
        let data_dir = std::path::Path::new(&database_path)
            .parent()
            .unwrap_or_else(|| std::path::Path::new("data"));
        data_dir.join("journals")
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

        let mcp_server = McpServerConfig {
            enabled: self.mcp_enabled.unwrap_or(false),
            bind: self.mcp_bind,
        };

        let openai_api_key = self.openai_api_key.clone();

        let daily_review = DailyReviewRuntimeConfig {
            openai_api_key: openai_api_key.clone(),
            review: ReviewConfig::from_values(self.review_model.clone()),
            prompt: DailyReviewPromptConfig::from_values(self.review_prompt_path.clone()),
            // Replaced in `serve()` with the shared handles loaded from the
            // persisted `/model` overrides.
            model_override: ModelOverride::default(),
        };

        let weekly_review = WeeklyReviewRuntimeConfig {
            openai_api_key: openai_api_key.clone(),
            review: WeeklyReviewConfig::from_values(self.week_review_model.clone()),
            prompt: WeeklyReviewPromptConfig::from_values(self.week_review_prompt_path.clone()),
            min_daily_reviews: self
                .week_review_min_daily_reviews
                .unwrap_or(DEFAULT_MIN_DAILY_REVIEWS),
            // Replaced in `serve()` with the shared handles loaded from the
            // persisted `/model` overrides.
            model_override: ModelOverride::default(),
        };

        let entry_extraction = JournalEntryExtractionRuntimeConfig {
            openai_api_key: openai_api_key.clone(),
            extraction: JournalEntryExtractionConfig::from_values(
                self.entry_extraction_model.clone(),
            ),
            prompt: JournalEntryExtractionPromptConfig::from_values(
                self.entry_extraction_prompt_path.clone(),
            ),
        };

        let signal_runtime = DailyReviewSignalRuntimeConfig {
            openai_api_key: openai_api_key.clone(),
            signal: DailyReviewSignalConfig::from_values(self.signal_extraction_model.clone()),
            prompt: DailyReviewSignalPromptConfig::from_values(
                self.signal_extraction_prompt_path.clone(),
            ),
        };

        Ok(ServeConfig {
            telegram_bot_token: telegram_bot_token.clone(),
            telegram_allowed_user_ids: self.telegram_allowed_user_ids.clone(),
            database_url: format!("sqlite:{database_path}"),
            database_path,
            embedding_worker,
            daily_review_embedding_worker,
            extraction_worker,
            daily_review_delivery,
            weekly_review_delivery,
            signal_worker,
            mcp_server,
            openai_api_key,
            embedding: EmbeddingConfig::from_values(self.embedding_model.clone()),
            daily_review,
            weekly_review,
            entry_extraction,
            signal_runtime,
        })
    }
}

#[cfg(test)]
mod tests {
    use clap::{CommandFactory, Parser};

    use super::*;

    fn default_cli() -> Cli {
        Cli {
            command: None,
            telegram_bot_token: None,
            telegram_allowed_user_ids: None,
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
            mcp_enabled: None,
            mcp_bind: "127.0.0.1:8080".parse().unwrap(),
            openai_api_key: None,
            embedding_model: None,
            review_model: None,
            review_prompt_path: None,
            week_review_model: None,
            week_review_prompt_path: None,
            entry_extraction_model: None,
            entry_extraction_prompt_path: None,
            signal_extraction_model: None,
            signal_extraction_prompt_path: None,
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
        ]);

        let config = cli.serve_config().unwrap();

        assert_eq!(config.telegram_bot_token, "token");
        assert_eq!(config.telegram_allowed_user_ids, None);
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
    fn parses_optional_telegram_allowed_user_ids() {
        let cli = Cli::parse_from([
            "froid",
            "--telegram-bot-token",
            "token",
            "--telegram-allowed-user-ids",
            "42,43,44",
        ]);

        let config = cli.serve_config().unwrap();

        assert_eq!(config.telegram_allowed_user_ids, Some(vec![42, 43, 44]));
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
    fn parses_serve_subcommand() {
        let cli = Cli::parse_from(["froid", "serve", "--telegram-bot-token", "token"]);

        assert!(matches!(cli.subcommand(), Some(super::Command::Serve)));
        assert!(cli.serve_config().is_ok());
    }

    #[test]
    fn defaults_to_serve_when_no_subcommand_given() {
        let cli = Cli::parse_from(["froid", "--telegram-bot-token", "token"]);

        assert!(cli.subcommand().is_none());
        assert!(cli.serve_config().is_ok());
    }

    #[test]
    fn parses_users_list_subcommand() {
        let cli = Cli::parse_from(["froid", "users", "list"]);

        assert!(matches!(
            cli.subcommand(),
            Some(super::Command::Users {
                command: UsersCommand::List
            })
        ));
    }

    #[test]
    fn parses_users_delete_subcommand() {
        let cli = Cli::parse_from(["froid", "users", "delete", "111", "--yes"]);

        let Some(super::Command::Users {
            command: UsersCommand::Delete { chat_id, yes },
        }) = cli.subcommand()
        else {
            panic!("expected users delete command");
        };
        assert_eq!(chat_id, "111");
        assert!(yes);
    }

    #[test]
    fn users_delete_defaults_to_unconfirmed() {
        let cli = Cli::parse_from(["froid", "users", "delete", "111"]);

        let Some(super::Command::Users {
            command: UsersCommand::Delete { yes, .. },
        }) = cli.subcommand()
        else {
            panic!("expected users delete command");
        };
        assert!(!yes);
    }

    #[test]
    fn journals_dir_derives_from_data_dir() {
        let cli = Cli::parse_from(["froid", "--data-dir", "custom"]);

        assert_eq!(cli.journals_dir(), PathBuf::from("custom/journals"));
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
    fn serve_config_builds_llm_configs_from_flags() {
        let cli = Cli::parse_from([
            "froid",
            "--telegram-bot-token",
            "token",
            "--openai-api-key",
            "sk-test",
            "--embedding-model",
            "embed-x",
            "--review-model",
            "review-x",
            "--review-prompt-path",
            "prompts/custom_daily.md",
            "--week-review-min-daily-reviews",
            "5",
        ]);

        let config = cli.serve_config().unwrap();

        assert_eq!(config.openai_api_key(), Some("sk-test"));
        assert_eq!(config.embedding.model, "embed-x");
        assert_eq!(
            config.daily_review.openai_api_key.as_deref(),
            Some("sk-test")
        );
        assert_eq!(config.daily_review.review.model, "review-x");
        assert_eq!(
            config.daily_review.prompt.path,
            PathBuf::from("prompts/custom_daily.md")
        );
        assert_eq!(config.weekly_review.min_daily_reviews, 5);
    }

    #[test]
    fn serve_config_openai_api_key_treats_blank_as_unset() {
        let cli = Cli::parse_from([
            "froid",
            "--telegram-bot-token",
            "token",
            "--openai-api-key",
            "  ",
        ]);

        let config = cli.serve_config().unwrap();

        assert_eq!(config.openai_api_key(), None);
    }

    #[test]
    fn serve_config_mcp_disabled_by_default() {
        let config = cli_with_token("token").serve_config().unwrap();

        assert!(!config.mcp_server.enabled);
        assert_eq!(config.mcp_server.bind.to_string(), "127.0.0.1:8080");
    }

    #[test]
    fn serve_config_mcp_enabled_with_custom_bind() {
        let cli = Cli::parse_from([
            "froid",
            "--telegram-bot-token",
            "token",
            "--mcp-enabled",
            "true",
            "--mcp-bind",
            "0.0.0.0:9000",
        ]);

        let config = cli.serve_config().unwrap();

        assert!(config.mcp_server.enabled);
        assert_eq!(config.mcp_server.bind.to_string(), "0.0.0.0:9000");
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
