use clap::{Parser, Subcommand};

use crate::version;

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
        default_value = "froid.sqlite3"
    )]
    database_path: String,

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
    pub database_url: String,
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

        Ok(ServeConfig {
            telegram_bot_token: telegram_bot_token.clone(),
            database_url: format!("sqlite:{}", self.database_path),
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
        assert_eq!(config.database_url, "sqlite:custom.db");
    }

    #[test]
    fn uses_default_database_path() {
        let cli = Cli::parse_from(["froid", "--telegram-bot-token", "token"]);

        let config = cli.serve_config().unwrap();

        assert_eq!(config.database_url, "sqlite:froid.sqlite3");
    }

    #[test]
    fn defaults_to_serve_command() {
        let cli = Cli {
            telegram_bot_token: None,
            database_path: "froid.sqlite3".to_string(),
            command: None,
        };

        assert!(matches!(cli.selected_command(), Command::Serve));
    }

    #[test]
    fn rejects_missing_telegram_bot_token() {
        let cli = Cli {
            telegram_bot_token: None,
            database_path: "froid.sqlite3".to_string(),
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
}
