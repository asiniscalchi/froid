use chrono::Utc;
use teloxide::{prelude::*, types::Message};
use tracing::{error, info};

use crate::{
    journal::service::JournalService,
    messages::{IncomingMessage, MessageSource},
};

use super::Adapter;

const DEFAULT_RECENT_LIMIT: u32 = 10;
const UNSUPPORTED_MESSAGE_RESPONSE: &str = "Unsupported message type";

pub struct TelegramAdapter {
    bot_token: String,
    journal_service: JournalService,
}

impl TelegramAdapter {
    pub fn new(bot_token: String, journal_service: JournalService) -> Self {
        Self {
            bot_token,
            journal_service,
        }
    }
}

impl Adapter for TelegramAdapter {
    async fn run(self) {
        let bot = Bot::new(self.bot_token);
        let journal_service = self.journal_service;

        teloxide::repl(bot, move |bot: Bot, message: Message| {
            let journal_service = journal_service.clone();

            async move { handle_message(bot, message, journal_service).await }
        })
        .await;
    }
}

async fn handle_message(
    bot: Bot,
    message: Message,
    journal_service: JournalService,
) -> ResponseResult<()> {
    let Some(text) = message.text() else {
        bot.send_message(message.chat.id, UNSUPPORTED_MESSAGE_RESPONSE)
            .await?;
        return Ok(());
    };

    let user_id = message
        .from
        .as_ref()
        .map(|u| u.id.to_string())
        .unwrap_or_else(|| message.chat.id.to_string());

    if let Some(limit) = parse_recent_command(text) {
        info!(user_id = %user_id, limit, "received /recent command");

        match journal_service.recent(&user_id, limit).await {
            Ok(Some(outgoing)) => {
                bot.send_message(message.chat.id, outgoing.text).await?;
            }
            Ok(None) => {}
            Err(err) => {
                error!(%err, "failed to fetch recent entries");
            }
        }

        return Ok(());
    }

    let incoming = incoming_from_text_message(&message, user_id);

    info!(
        source_conversation_id = %incoming.source_conversation_id,
        source_message_id = %incoming.source_message_id,
        user_id = %incoming.user_id,
        "received Telegram text message"
    );

    let response_text = match journal_service.process(&incoming).await {
        Ok(outgoing) => outgoing.text,
        Err(err) => {
            error!(%err, "failed to store journal entry");
            "Something went wrong. Please try again.".to_string()
        }
    };

    bot.send_message(message.chat.id, response_text).await?;

    Ok(())
}

fn incoming_from_text_message(message: &Message, user_id: String) -> IncomingMessage {
    IncomingMessage {
        source: MessageSource::Telegram,
        source_conversation_id: message.chat.id.to_string(),
        source_message_id: message.id.to_string(),
        user_id,
        text: message.text().unwrap_or_default().to_string(),
        received_at: Utc::now(),
    }
}

fn parse_recent_command(text: &str) -> Option<u32> {
    let mut parts = text.trim().splitn(2, char::is_whitespace);
    let command = parts.next()?;
    // strip optional @botname suffix
    let command = command.split('@').next()?;
    if command != "/recent" {
        return None;
    }
    let limit = parts
        .next()
        .and_then(|s| s.trim().parse::<u32>().ok())
        .unwrap_or(DEFAULT_RECENT_LIMIT);
    Some(limit)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::messages::MessageSource;

    #[test]
    fn maps_telegram_text_message_to_internal_message() {
        let message = telegram_message(json!({
            "message_id": 100,
            "from": {
                "id": 7,
                "is_bot": false,
                "first_name": "Ada"
            },
            "date": 1_700_000_000,
            "chat": {
                "id": 42,
                "type": "private",
                "first_name": "Ada"
            },
            "text": "hello froid"
        }));
        let user_id = "7".to_string();

        let incoming = incoming_from_text_message(&message, user_id);

        assert_eq!(incoming.source, MessageSource::Telegram);
        assert_eq!(incoming.source_conversation_id, "42");
        assert_eq!(incoming.source_message_id, "100");
        assert_eq!(incoming.user_id, "7");
        assert_eq!(incoming.text, "hello froid");
    }

    #[test]
    fn parse_recent_command_with_no_argument_uses_default_limit() {
        assert_eq!(parse_recent_command("/recent"), Some(DEFAULT_RECENT_LIMIT));
    }

    #[test]
    fn parse_recent_command_with_explicit_limit() {
        assert_eq!(parse_recent_command("/recent 5"), Some(5));
    }

    #[test]
    fn parse_recent_command_strips_bot_name_suffix() {
        assert_eq!(
            parse_recent_command("/recent@mybot"),
            Some(DEFAULT_RECENT_LIMIT)
        );
        assert_eq!(parse_recent_command("/recent@mybot 3"), Some(3));
    }

    #[test]
    fn parse_recent_command_returns_none_for_non_command() {
        assert_eq!(parse_recent_command("hello"), None);
        assert_eq!(parse_recent_command("/other"), None);
    }

    fn telegram_message(value: serde_json::Value) -> Message {
        serde_json::from_value(value).unwrap()
    }
}
