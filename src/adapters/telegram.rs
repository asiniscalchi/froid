use teloxide::{prelude::*, types::Message};
use tracing::{error, info};

use crate::{
    handler::MessageHandler,
    journal::command::{DEFAULT_RECENT_LIMIT, JournalCommand, JournalCommandRequest},
    messages::{IncomingMessage, MessageSource},
};

use super::Adapter;

const UNSUPPORTED_MESSAGE_RESPONSE: &str = "Unsupported message type";

pub struct TelegramAdapter<H: MessageHandler> {
    bot_token: String,
    handler: H,
}

impl<H: MessageHandler> TelegramAdapter<H> {
    pub fn new(bot_token: String, handler: H) -> Self {
        Self { bot_token, handler }
    }
}

impl<H: MessageHandler> Adapter for TelegramAdapter<H> {
    async fn run(self) {
        let bot = Bot::new(self.bot_token);
        let handler = self.handler;

        teloxide::repl(bot, move |bot: Bot, message: Message| {
            let handler = handler.clone();

            async move { handle_message(bot, message, handler).await }
        })
        .await;
    }
}

async fn handle_message<H: MessageHandler>(
    bot: Bot,
    message: Message,
    handler: H,
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

    if let Some(command) = parse_command(text) {
        let request = JournalCommandRequest {
            user_id: user_id.clone(),
            received_at: message.date,
            command,
        };

        info!(user_id = %user_id, "received Telegram command");

        match handler.command(&request).await {
            Ok(outgoing) => {
                bot.send_message(message.chat.id, outgoing.text).await?;
            }
            Err(err) => {
                error!(%err, "failed to process journal command");
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

    let response_text = match handler.process(&incoming).await {
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
        received_at: message.date,
    }
}

fn parse_command(text: &str) -> Option<JournalCommand> {
    let mut parts = text.trim().splitn(2, char::is_whitespace);
    let command = parts.next()?;
    // strip optional @botname suffix
    let command = command.split('@').next()?;
    let argument = parts.next().map(str::trim).filter(|s| !s.is_empty());

    match command {
        "/start" => Some(JournalCommand::Start),
        "/help" => Some(JournalCommand::Help),
        "/recent" => parse_recent_argument(argument),
        "/today" => Some(JournalCommand::Today),
        "/stats" => Some(JournalCommand::Stats),
        "/review" => Some(parse_review_argument(argument)),
        "/search" => Some(parse_search_argument(argument)),
        _ => None,
    }
}

fn parse_review_argument(argument: Option<&str>) -> JournalCommand {
    match argument {
        Some("today") => JournalCommand::ReviewToday,
        _ => JournalCommand::ReviewUsage,
    }
}

fn parse_search_argument(argument: Option<&str>) -> JournalCommand {
    match argument {
        Some(query) => JournalCommand::Search {
            query: query.to_string(),
        },
        None => JournalCommand::SearchUsage,
    }
}

fn parse_recent_argument(argument: Option<&str>) -> Option<JournalCommand> {
    let Some(argument) = argument else {
        return Some(JournalCommand::Recent {
            requested_limit: DEFAULT_RECENT_LIMIT,
        });
    };

    match argument.parse::<u32>() {
        Ok(limit) if limit > 0 => Some(JournalCommand::Recent {
            requested_limit: limit,
        }),
        _ => Some(JournalCommand::RecentUsage),
    }
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
        assert_eq!(
            incoming.received_at,
            chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap()
        );
    }

    #[test]
    fn parse_start_command() {
        assert_eq!(parse_command("/start"), Some(JournalCommand::Start));
    }

    #[test]
    fn parse_help_command() {
        assert_eq!(parse_command("/help"), Some(JournalCommand::Help));
    }

    #[test]
    fn parse_recent_command_with_no_argument_uses_default_limit() {
        assert_eq!(
            parse_command("/recent"),
            Some(JournalCommand::Recent {
                requested_limit: DEFAULT_RECENT_LIMIT
            })
        );
    }

    #[test]
    fn parse_recent_command_with_explicit_limit() {
        assert_eq!(
            parse_command("/recent 5"),
            Some(JournalCommand::Recent { requested_limit: 5 })
        );
    }

    #[test]
    fn parse_recent_command_strips_bot_name_suffix() {
        assert_eq!(
            parse_command("/recent@mybot"),
            Some(JournalCommand::Recent {
                requested_limit: DEFAULT_RECENT_LIMIT
            })
        );
        assert_eq!(
            parse_command("/recent@mybot 3"),
            Some(JournalCommand::Recent { requested_limit: 3 })
        );
    }

    #[test]
    fn parse_recent_command_returns_usage_for_invalid_argument() {
        assert_eq!(
            parse_command("/recent abc"),
            Some(JournalCommand::RecentUsage)
        );
        assert_eq!(
            parse_command("/recent 0"),
            Some(JournalCommand::RecentUsage)
        );
        assert_eq!(
            parse_command("/recent -3"),
            Some(JournalCommand::RecentUsage)
        );
    }

    #[test]
    fn parse_today_command() {
        assert_eq!(parse_command("/today"), Some(JournalCommand::Today));
    }

    #[test]
    fn parse_stats_command() {
        assert_eq!(parse_command("/stats"), Some(JournalCommand::Stats));
    }

    #[test]
    fn parse_review_today_command() {
        assert_eq!(
            parse_command("/review today"),
            Some(JournalCommand::ReviewToday)
        );
    }

    #[test]
    fn parse_review_today_command_strips_bot_name_suffix() {
        assert_eq!(
            parse_command("/review@mybot today"),
            Some(JournalCommand::ReviewToday)
        );
    }

    #[test]
    fn parse_review_command_without_today_returns_usage() {
        assert_eq!(parse_command("/review"), Some(JournalCommand::ReviewUsage));
        assert_eq!(parse_command("/review "), Some(JournalCommand::ReviewUsage));
    }

    #[test]
    fn parse_review_command_with_unsupported_argument_returns_usage() {
        assert_eq!(
            parse_command("/review yesterday"),
            Some(JournalCommand::ReviewUsage)
        );
        assert_eq!(
            parse_command("/review today extra"),
            Some(JournalCommand::ReviewUsage)
        );
    }

    #[test]
    fn parse_search_command_with_query() {
        assert_eq!(
            parse_command("/search anxiety before meetings"),
            Some(JournalCommand::Search {
                query: "anxiety before meetings".to_string()
            })
        );
    }

    #[test]
    fn parse_search_command_strips_bot_name_suffix() {
        assert_eq!(
            parse_command("/search@mybot something"),
            Some(JournalCommand::Search {
                query: "something".to_string()
            })
        );
    }

    #[test]
    fn parse_search_command_without_query_returns_usage() {
        assert_eq!(parse_command("/search"), Some(JournalCommand::SearchUsage));
    }

    #[test]
    fn parse_search_command_treats_all_words_after_command_as_query() {
        assert_eq!(
            parse_command("/search word1 word2 word3"),
            Some(JournalCommand::Search {
                query: "word1 word2 word3".to_string()
            })
        );
    }

    #[test]
    fn parse_recent_command_returns_none_for_non_command() {
        assert_eq!(parse_command("hello"), None);
        assert_eq!(parse_command("/other"), None);
    }

    fn telegram_message(value: serde_json::Value) -> Message {
        serde_json::from_value(value).unwrap()
    }
}
