use std::{env, error::Error, fmt};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppConfig {
    pub telegram_bot_token: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    MissingTelegramBotToken,
    EmptyTelegramBotToken,
}

impl AppConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(env::var)
    }

    fn from_lookup(
        lookup: impl FnOnce(&'static str) -> Result<String, env::VarError>,
    ) -> Result<Self, ConfigError> {
        let telegram_bot_token =
            lookup("TELEGRAM_BOT_TOKEN").map_err(|_| ConfigError::MissingTelegramBotToken)?;

        if telegram_bot_token.trim().is_empty() {
            return Err(ConfigError::EmptyTelegramBotToken);
        }

        Ok(Self { telegram_bot_token })
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingTelegramBotToken => {
                write!(
                    formatter,
                    "TELEGRAM_BOT_TOKEN environment variable is required"
                )
            }
            Self::EmptyTelegramBotToken => {
                write!(
                    formatter,
                    "TELEGRAM_BOT_TOKEN environment variable must not be empty"
                )
            }
        }
    }
}

impl Error for ConfigError {}

#[cfg(test)]
mod tests {
    use std::env::VarError;

    use super::*;

    #[test]
    fn loads_telegram_bot_token() {
        let config = AppConfig::from_lookup(|name| {
            assert_eq!(name, "TELEGRAM_BOT_TOKEN");
            Ok("token".to_string())
        })
        .unwrap();

        assert_eq!(config.telegram_bot_token, "token");
    }

    #[test]
    fn rejects_missing_telegram_bot_token() {
        let error = AppConfig::from_lookup(|_| Err(VarError::NotPresent)).unwrap_err();

        assert_eq!(error, ConfigError::MissingTelegramBotToken);
    }

    #[test]
    fn rejects_empty_telegram_bot_token() {
        let error = AppConfig::from_lookup(|_| Ok("  ".to_string())).unwrap_err();

        assert_eq!(error, ConfigError::EmptyTelegramBotToken);
    }
}
