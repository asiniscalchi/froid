use chrono::{DateTime, Utc};
use teloxide::{prelude::*, types::Message};
use tracing::{error, info};

use crate::{
    journal::service::JournalService,
    messages::{IncomingMessage, MessageSource},
};

use super::Adapter;

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
    let response_text = match incoming_from_text_message(&message, Utc::now()) {
        Some(incoming) => {
            info!(
                source_conversation_id = %incoming.source_conversation_id,
                source_message_id = %incoming.source_message_id,
                user_id = %incoming.user_id,
                "received Telegram text message"
            );

            match journal_service.process(&incoming).await {
                Ok(outgoing) => outgoing.text,
                Err(err) => {
                    error!(%err, "failed to store journal entry");
                    "Something went wrong. Please try again.".to_string()
                }
            }
        }
        None => UNSUPPORTED_MESSAGE_RESPONSE.to_string(),
    };

    bot.send_message(message.chat.id, response_text).await?;

    Ok(())
}

fn incoming_from_text_message(
    message: &Message,
    received_at: DateTime<Utc>,
) -> Option<IncomingMessage> {
    let text = message.text()?;
    let user_id = message
        .from
        .as_ref()
        .map(|user| user.id.to_string())
        .unwrap_or_else(|| message.chat.id.to_string());

    Some(IncomingMessage {
        source: MessageSource::Telegram,
        source_conversation_id: message.chat.id.to_string(),
        source_message_id: message.id.to_string(),
        user_id,
        text: text.to_string(),
        received_at,
    })
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use serde_json::json;

    use super::*;
    use crate::messages::MessageSource;

    #[test]
    fn maps_telegram_text_message_to_internal_message() {
        let telegram_message = telegram_message(json!({
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
        let received_at = Utc.with_ymd_and_hms(2026, 4, 28, 12, 0, 0).unwrap();

        let incoming = incoming_from_text_message(&telegram_message, received_at).unwrap();

        assert_eq!(incoming.source, MessageSource::Telegram);
        assert_eq!(incoming.source_conversation_id, "42");
        assert_eq!(incoming.source_message_id, "100");
        assert_eq!(incoming.user_id, "7");
        assert_eq!(incoming.text, "hello froid");
        assert_eq!(incoming.received_at, received_at);
    }

    #[test]
    fn does_not_map_non_text_message_to_internal_message() {
        let telegram_message = telegram_message(json!({
            "message_id": 101,
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
            "photo": [{
                "file_id": "file-id",
                "file_unique_id": "file-unique-id",
                "width": 320,
                "height": 240
            }]
        }));

        assert!(incoming_from_text_message(&telegram_message, Utc::now()).is_none());
    }

    fn telegram_message(value: serde_json::Value) -> Message {
        serde_json::from_value(value).unwrap()
    }
}
