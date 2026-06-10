use std::net::SocketAddr;

use clap::Parser;

use crate::{
    auth::UserToken,
    journal::{
        review::DailyReviewDeliveryWorkerConfig, week_review::WeeklyReviewDeliveryWorkerConfig,
    },
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

    #[arg(
        long,
        env = "FROID_MCP_BIND",
        global = true,
        default_value = "127.0.0.1:8080"
    )]
    mcp_bind: SocketAddr,

    #[arg(long, env = "FROID_DASHBOARD_ENABLED", global = true)]
    dashboard_enabled: Option<bool>,

    #[arg(long, env = "FROID_AUTH_TOKEN", global = true, hide_env_values = true)]
    auth_token: Option<String>,

    #[arg(
        long,
        env = "FROID_AUTH_TOKENS",
        global = true,
        hide_env_values = true,
        value_delimiter = ','
    )]
    auth_tokens: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServerConfig {
    pub enabled: bool,
    pub bind: SocketAddr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DashboardConfig {
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpAuthConfig {
    /// Bearer token required on the shared HTTP listener (MCP and dashboard).
    /// When `None`, the HTTP endpoints are unauthenticated.
    pub token: Option<String>,
    /// Per-user bearer tokens (`chat_id:token` pairs). When non-empty, each
    /// request is served from the database of the user owning the token.
    /// Mutually exclusive with `token`.
    pub user_tokens: Vec<UserToken>,
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
    pub dashboard: DashboardConfig,
    pub http_auth: HttpAuthConfig,
}

/// Parse `chat_id:token` pairs from `FROID_AUTH_TOKENS`. The chat id is
/// everything before the first colon; the token is the (possibly
/// colon-containing) remainder.
fn parse_user_tokens(pairs: &[String]) -> Result<Vec<UserToken>, clap::Error> {
    let invalid =
        |message: String| clap::Error::raw(clap::error::ErrorKind::ValueValidation, message);

    let mut tokens: Vec<UserToken> = Vec::with_capacity(pairs.len());
    for pair in pairs {
        let Some((chat_id, token)) = pair.split_once(':') else {
            return Err(invalid(format!(
                "FROID_AUTH_TOKENS entries must look like <chat_id>:<token>; got {pair:?}"
            )));
        };
        let (chat_id, token) = (chat_id.trim(), token.trim());
        if chat_id.is_empty() || token.is_empty() {
            return Err(invalid(format!(
                "FROID_AUTH_TOKENS entries must have a non-empty chat id and token; got {pair:?}"
            )));
        }
        if tokens.iter().any(|existing| existing.chat_id == chat_id) {
            return Err(invalid(format!(
                "FROID_AUTH_TOKENS lists chat id {chat_id} more than once"
            )));
        }
        if tokens.iter().any(|existing| existing.token == token) {
            return Err(invalid(
                "FROID_AUTH_TOKENS lists the same token for multiple chat ids".to_string(),
            ));
        }
        tokens.push(UserToken {
            chat_id: chat_id.to_string(),
            token: token.to_string(),
        });
    }

    Ok(tokens)
}

impl Cli {
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

        let dashboard = DashboardConfig {
            enabled: self.dashboard_enabled.unwrap_or(false),
        };

        let token = match self.auth_token.as_deref() {
            Some(token) if token.trim().is_empty() => {
                return Err(clap::Error::raw(
                    clap::error::ErrorKind::ValueValidation,
                    "FROID_AUTH_TOKEN environment variable or --auth-token must not be empty when set",
                ));
            }
            Some(token) => Some(token.to_string()),
            None => None,
        };

        let user_tokens = parse_user_tokens(self.auth_tokens.as_deref().unwrap_or_default())?;

        if token.is_some() && !user_tokens.is_empty() {
            return Err(clap::Error::raw(
                clap::error::ErrorKind::ValueValidation,
                "FROID_AUTH_TOKEN and FROID_AUTH_TOKENS are mutually exclusive; configure one of them",
            ));
        }

        let http_auth = HttpAuthConfig { token, user_tokens };

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
            dashboard,
            http_auth,
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
            dashboard_enabled: None,
            auth_token: None,
            auth_tokens: None,
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
    fn serve_config_mcp_disabled_by_default() {
        let config = cli_with_token("token").serve_config().unwrap();

        assert!(!config.mcp_server.enabled);
        assert_eq!(config.mcp_server.bind.to_string(), "127.0.0.1:8080");
    }

    #[test]
    fn serve_config_dashboard_disabled_by_default() {
        let config = cli_with_token("token").serve_config().unwrap();

        assert!(!config.dashboard.enabled);
    }

    #[test]
    fn serve_config_http_auth_token_none_by_default() {
        let config = cli_with_token("token").serve_config().unwrap();

        assert_eq!(config.http_auth.token, None);
    }

    #[test]
    fn serve_config_http_auth_token_set_from_flag() {
        let cli = Cli::parse_from([
            "froid",
            "--telegram-bot-token",
            "token",
            "--auth-token",
            "secret-bearer",
        ]);

        let config = cli.serve_config().unwrap();

        assert_eq!(config.http_auth.token.as_deref(), Some("secret-bearer"));
    }

    #[test]
    fn serve_config_rejects_empty_auth_token() {
        let error = Cli {
            auth_token: Some("  ".to_string()),
            ..cli_with_token("token")
        }
        .serve_config()
        .unwrap_err();

        assert_eq!(error.kind(), clap::error::ErrorKind::ValueValidation);
        assert!(error.to_string().contains("FROID_AUTH_TOKEN"));
    }

    #[test]
    fn serve_config_parses_per_user_tokens() {
        let cli = Cli::parse_from([
            "froid",
            "--telegram-bot-token",
            "token",
            "--auth-tokens",
            "111:alice-secret,222:bob-secret",
        ]);

        let config = cli.serve_config().unwrap();

        assert_eq!(config.http_auth.token, None);
        assert_eq!(
            config.http_auth.user_tokens,
            vec![
                UserToken {
                    chat_id: "111".to_string(),
                    token: "alice-secret".to_string(),
                },
                UserToken {
                    chat_id: "222".to_string(),
                    token: "bob-secret".to_string(),
                },
            ]
        );
    }

    #[test]
    fn serve_config_user_token_keeps_colons_in_token() {
        let cli = Cli {
            auth_tokens: Some(vec!["111:secret:with:colons".to_string()]),
            ..cli_with_token("token")
        };

        let config = cli.serve_config().unwrap();

        assert_eq!(config.http_auth.user_tokens[0].token, "secret:with:colons");
    }

    #[test]
    fn serve_config_rejects_user_token_without_separator() {
        let error = Cli {
            auth_tokens: Some(vec!["just-a-token".to_string()]),
            ..cli_with_token("token")
        }
        .serve_config()
        .unwrap_err();

        assert_eq!(error.kind(), clap::error::ErrorKind::ValueValidation);
        assert!(error.to_string().contains("<chat_id>:<token>"));
    }

    #[test]
    fn serve_config_rejects_empty_user_token_parts() {
        for pair in [":secret", "111:", " : "] {
            let error = Cli {
                auth_tokens: Some(vec![pair.to_string()]),
                ..cli_with_token("token")
            }
            .serve_config()
            .unwrap_err();

            assert_eq!(error.kind(), clap::error::ErrorKind::ValueValidation);
        }
    }

    #[test]
    fn serve_config_rejects_duplicate_chat_id_in_user_tokens() {
        let error = Cli {
            auth_tokens: Some(vec!["111:a".to_string(), "111:b".to_string()]),
            ..cli_with_token("token")
        }
        .serve_config()
        .unwrap_err();

        assert!(error.to_string().contains("more than once"));
    }

    #[test]
    fn serve_config_rejects_duplicate_token_in_user_tokens() {
        let error = Cli {
            auth_tokens: Some(vec!["111:same".to_string(), "222:same".to_string()]),
            ..cli_with_token("token")
        }
        .serve_config()
        .unwrap_err();

        assert!(error.to_string().contains("same token"));
    }

    #[test]
    fn serve_config_rejects_single_and_per_user_tokens_together() {
        let error = Cli {
            auth_token: Some("single".to_string()),
            auth_tokens: Some(vec!["111:per-user".to_string()]),
            ..cli_with_token("token")
        }
        .serve_config()
        .unwrap_err();

        assert!(error.to_string().contains("mutually exclusive"));
    }

    #[test]
    fn serve_config_dashboard_enabled_when_flag_set() {
        let cli = Cli::parse_from([
            "froid",
            "--telegram-bot-token",
            "token",
            "--dashboard-enabled",
            "true",
        ]);

        let config = cli.serve_config().unwrap();

        assert!(config.dashboard.enabled);
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
