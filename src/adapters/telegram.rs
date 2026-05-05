use teloxide::{
    payloads::SetMessageReactionSetters,
    prelude::*,
    sugar::bot::BotMessagesExt,
    types::{Message, ReactionType},
};
use tracing::{error, info};

use chrono::{DateTime, Utc};

use crate::{
    handler::MessageHandler,
    journal::command::{DEFAULT_RECENT_LIMIT, JournalCommand, JournalCommandRequest},
    messages::{IncomingMessage, MessageSource, SINGLE_USER_ID},
};

const UNSUPPORTED_MESSAGE_RESPONSE: &str = "Unsupported message type";

pub struct TelegramAdapter<H: MessageHandler> {
    bot_token: String,
    allowed_user_id: Option<u64>,
    handler: H,
}

impl<H: MessageHandler> TelegramAdapter<H> {
    pub fn new(bot_token: String, allowed_user_id: Option<u64>, handler: H) -> Self {
        Self {
            bot_token,
            allowed_user_id,
            handler,
        }
    }

    pub async fn run(self) {
        let bot = Bot::new(self.bot_token);
        let allowed_user_id = self.allowed_user_id;
        let handler = self.handler;

        match allowed_user_id {
            Some(id) => {
                info!(
                    allowed_user_id = id,
                    chat_scope = "private",
                    "starting Telegram adapter"
                );
            }
            None => {
                info!(
                    allowed_user_id = "all",
                    chat_scope = "private",
                    "starting Telegram adapter"
                );
            }
        }

        teloxide::repl(bot, move |bot: Bot, message: Message| {
            let handler = handler.clone();
            let allowed_user_id = allowed_user_id;

            async move { handle_message(bot, message, allowed_user_id, handler).await }
        })
        .await;
    }
}

async fn handle_message<H: MessageHandler>(
    bot: Bot,
    message: Message,
    allowed_user_id: Option<u64>,
    handler: H,
) -> ResponseResult<()> {
    if !should_handle_message(&message, allowed_user_id) {
        info!(
            chat_id = %message.chat.id,
            sender_user_id = message.from.as_ref().map(|user| user.id.0),
            allowed_user_id,
            "ignored Telegram message outside configured private user scope"
        );
        return Ok(());
    }

    let Some(text) = message.text() else {
        bot.send_message(message.chat.id, UNSUPPORTED_MESSAGE_RESPONSE)
            .await?;
        return Ok(());
    };

    if let Some(command) = parse_command(text, message.date) {
        let request = JournalCommandRequest {
            source: MessageSource::Telegram,
            source_conversation_id: message.chat.id.to_string(),
            user_id: SINGLE_USER_ID.to_string(),
            received_at: message.date,
            command,
        };

        info!("received Telegram command");

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

    let incoming = incoming_from_text_message(&message);

    info!(
        source_conversation_id = %incoming.source_conversation_id,
        source_message_id = %incoming.source_message_id,
        "received Telegram text message"
    );

    match handler.process(&incoming).await {
        Ok(_) => {
            bot.set_reaction(&message)
                .reaction([saved_reaction()])
                .await?;
        }
        Err(err) => {
            error!(%err, "failed to store journal entry");
            bot.send_message(message.chat.id, "Something went wrong. Please try again.")
                .await?;
        }
    };

    Ok(())
}

fn should_handle_message(message: &Message, allowed_user_id: Option<u64>) -> bool {
    if !message.chat.is_private() {
        return false;
    }

    let Some(sender) = message.from.as_ref() else {
        return false;
    };

    allowed_user_id.is_none_or(|id| sender.id.0 == id)
}

fn saved_reaction() -> ReactionType {
    ReactionType::Emoji {
        emoji: "✍".to_string(),
    }
}

fn incoming_from_text_message(message: &Message) -> IncomingMessage {
    IncomingMessage {
        source: MessageSource::Telegram,
        source_conversation_id: message.chat.id.to_string(),
        source_message_id: message.id.to_string(),
        user_id: SINGLE_USER_ID.to_string(),
        text: message.text().unwrap_or_default().to_string(),
        received_at: message.date,
    }
}

fn parse_command(text: &str, _received_at: DateTime<Utc>) -> Option<JournalCommand> {
    let mut parts = text.trim().splitn(2, char::is_whitespace);
    let command = parts.next()?;
    // strip optional @botname suffix
    let command = command.split('@').next()?;
    let argument = parts.next().map(str::trim).filter(|s| !s.is_empty());

    match command {
        "/start" => Some(JournalCommand::Start),
        "/help" => Some(JournalCommand::Help),
        "/last" => Some(JournalCommand::Last),
        "/undo" => Some(JournalCommand::Undo),
        "/recent" => parse_recent_argument(argument),
        "/today" => Some(JournalCommand::Today),
        "/stats" => Some(JournalCommand::Stats),
        "/status" => Some(JournalCommand::Status),
        "/day_review" => Some(JournalCommand::DayReviewLast),
        "/week_review" => Some(JournalCommand::WeekReviewLast),
        "/search" => Some(parse_search_argument(argument)),
        _ if command.starts_with('/') => Some(JournalCommand::Unknown {
            command: command.to_string(),
        }),
        _ => None,
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
    use chrono::{TimeZone, Utc};
    use serde_json::json;

    use super::*;
    use crate::messages::MessageSource;

    fn received_at() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 4, 29, 12, 0, 0).unwrap()
    }

    fn cmd(text: &str) -> Option<JournalCommand> {
        parse_command(text, received_at())
    }

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
        let incoming = incoming_from_text_message(&message);

        assert_eq!(incoming.source, MessageSource::Telegram);
        assert_eq!(incoming.source_conversation_id, "42");
        assert_eq!(incoming.source_message_id, "100");
        assert_eq!(incoming.user_id, SINGLE_USER_ID);
        assert_eq!(incoming.text, "hello froid");
        assert_eq!(
            incoming.received_at,
            chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap()
        );
    }

    #[test]
    fn handles_private_message_from_allowed_user() {
        let message = telegram_message(json!({
            "message_id": 100,
            "from": {
                "id": 7,
                "is_bot": false,
                "first_name": "Ada"
            },
            "date": 1_700_000_000,
            "chat": {
                "id": 7,
                "type": "private",
                "first_name": "Ada"
            },
            "text": "hello froid"
        }));

        assert!(should_handle_message(&message, Some(7)));
    }

    #[test]
    fn handles_any_private_sender_when_no_allowed_user_is_configured() {
        let message = telegram_message(json!({
            "message_id": 100,
            "from": {
                "id": 99,
                "is_bot": false,
                "first_name": "Grace"
            },
            "date": 1_700_000_000,
            "chat": {
                "id": 99,
                "type": "private",
                "first_name": "Grace"
            },
            "text": "hello froid"
        }));

        assert!(should_handle_message(&message, None));
    }

    #[test]
    fn ignores_private_message_from_other_user() {
        let message = telegram_message(json!({
            "message_id": 100,
            "from": {
                "id": 8,
                "is_bot": false,
                "first_name": "Edsger"
            },
            "date": 1_700_000_000,
            "chat": {
                "id": 8,
                "type": "private",
                "first_name": "Edsger"
            },
            "text": "hello froid"
        }));

        assert!(!should_handle_message(&message, Some(7)));
    }

    #[test]
    fn ignores_group_message_even_from_allowed_user() {
        let message = telegram_message(json!({
            "message_id": 100,
            "from": {
                "id": 7,
                "is_bot": false,
                "first_name": "Ada"
            },
            "date": 1_700_000_000,
            "chat": {
                "id": -42,
                "type": "group",
                "title": "Journal"
            },
            "text": "hello froid"
        }));

        assert!(!should_handle_message(&message, Some(7)));
        assert!(!should_handle_message(&message, None));
    }

    #[test]
    fn ignores_message_without_sender() {
        let message = telegram_message(json!({
            "message_id": 100,
            "date": 1_700_000_000,
            "chat": {
                "id": 7,
                "type": "private",
                "first_name": "Ada"
            },
            "text": "hello froid"
        }));

        assert!(!should_handle_message(&message, Some(7)));
        assert!(!should_handle_message(&message, None));
    }

    #[test]
    fn saved_reaction_uses_writing_hand() {
        assert_eq!(
            saved_reaction(),
            ReactionType::Emoji {
                emoji: "✍".to_string()
            }
        );
    }

    #[test]
    fn parse_start_command() {
        assert_eq!(cmd("/start"), Some(JournalCommand::Start));
    }

    #[test]
    fn parse_help_command() {
        assert_eq!(cmd("/help"), Some(JournalCommand::Help));
    }

    #[test]
    fn parse_last_command() {
        assert_eq!(cmd("/last"), Some(JournalCommand::Last));
        assert_eq!(cmd("/last@mybot"), Some(JournalCommand::Last));
    }

    #[test]
    fn parse_undo_command() {
        assert_eq!(cmd("/undo"), Some(JournalCommand::Undo));
        assert_eq!(cmd("/undo@mybot"), Some(JournalCommand::Undo));
    }

    #[test]
    fn parse_recent_command_with_no_argument_uses_default_limit() {
        assert_eq!(
            cmd("/recent"),
            Some(JournalCommand::Recent {
                requested_limit: DEFAULT_RECENT_LIMIT
            })
        );
    }

    #[test]
    fn parse_recent_command_with_explicit_limit() {
        assert_eq!(
            cmd("/recent 5"),
            Some(JournalCommand::Recent { requested_limit: 5 })
        );
    }

    #[test]
    fn parse_recent_command_strips_bot_name_suffix() {
        assert_eq!(
            cmd("/recent@mybot"),
            Some(JournalCommand::Recent {
                requested_limit: DEFAULT_RECENT_LIMIT
            })
        );
        assert_eq!(
            cmd("/recent@mybot 3"),
            Some(JournalCommand::Recent { requested_limit: 3 })
        );
    }

    #[test]
    fn parse_recent_command_returns_usage_for_invalid_argument() {
        assert_eq!(cmd("/recent abc"), Some(JournalCommand::RecentUsage));
        assert_eq!(cmd("/recent 0"), Some(JournalCommand::RecentUsage));
        assert_eq!(cmd("/recent -3"), Some(JournalCommand::RecentUsage));
    }

    #[test]
    fn parse_today_command() {
        assert_eq!(cmd("/today"), Some(JournalCommand::Today));
    }

    #[test]
    fn parse_stats_command() {
        assert_eq!(cmd("/stats"), Some(JournalCommand::Stats));
    }

    #[test]
    fn parse_status_command() {
        assert_eq!(cmd("/status"), Some(JournalCommand::Status));
    }

    #[test]
    fn parse_status_command_strips_bot_name_suffix() {
        assert_eq!(cmd("/status@mybot"), Some(JournalCommand::Status));
    }

    #[test]
    fn parse_day_review_command() {
        assert_eq!(cmd("/day_review"), Some(JournalCommand::DayReviewLast));
        assert_eq!(cmd("/day_review "), Some(JournalCommand::DayReviewLast));
        assert_eq!(
            cmd("/day_review@mybot"),
            Some(JournalCommand::DayReviewLast)
        );
    }

    #[test]
    fn parse_week_review_command() {
        assert_eq!(cmd("/week_review"), Some(JournalCommand::WeekReviewLast));
        assert_eq!(cmd("/week_review "), Some(JournalCommand::WeekReviewLast));
        assert_eq!(
            cmd("/week_review@mybot"),
            Some(JournalCommand::WeekReviewLast)
        );
    }

    #[test]
    fn parse_search_command_with_query() {
        assert_eq!(
            cmd("/search anxiety before meetings"),
            Some(JournalCommand::Search {
                query: "anxiety before meetings".to_string()
            })
        );
    }

    #[test]
    fn parse_search_command_strips_bot_name_suffix() {
        assert_eq!(
            cmd("/search@mybot something"),
            Some(JournalCommand::Search {
                query: "something".to_string()
            })
        );
    }

    #[test]
    fn parse_search_command_without_query_returns_usage() {
        assert_eq!(cmd("/search"), Some(JournalCommand::SearchUsage));
    }

    #[test]
    fn parse_search_command_treats_all_words_after_command_as_query() {
        assert_eq!(
            cmd("/search word1 word2 word3"),
            Some(JournalCommand::Search {
                query: "word1 word2 word3".to_string()
            })
        );
    }

    #[test]
    fn parse_returns_none_for_non_command() {
        assert_eq!(cmd("hello"), None);
    }

    #[test]
    fn parse_unknown_slash_prefixed_message_as_command() {
        assert_eq!(
            cmd("/other"),
            Some(JournalCommand::Unknown {
                command: "/other".to_string()
            })
        );
        assert_eq!(
            cmd("/other@mybot"),
            Some(JournalCommand::Unknown {
                command: "/other".to_string()
            })
        );
        assert_eq!(
            cmd("   /other with text"),
            Some(JournalCommand::Unknown {
                command: "/other".to_string()
            })
        );
    }

    fn telegram_message(value: serde_json::Value) -> Message {
        serde_json::from_value(value).unwrap()
    }
}
