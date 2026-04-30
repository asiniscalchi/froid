use teloxide::{prelude::*, types::Message};
use tracing::{error, info};

use chrono::{DateTime, NaiveDate, Utc};

use crate::{
    handler::MessageHandler,
    journal::command::{
        DEFAULT_RECENT_LIMIT, JournalCommand, JournalCommandRequest, MAX_REVIEW_OFFSET,
    },
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

    if let Some(command) = parse_command(text, message.date) {
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

fn parse_command(text: &str, received_at: DateTime<Utc>) -> Option<JournalCommand> {
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
        "/status" => Some(JournalCommand::Status),
        "/review" => Some(parse_review_argument(argument, received_at)),
        "/search" => Some(parse_search_argument(argument)),
        _ => None,
    }
}

fn parse_review_argument(argument: Option<&str>, received_at: DateTime<Utc>) -> JournalCommand {
    let today = received_at.date_naive();
    match argument {
        None | Some("today") => JournalCommand::ReviewToday,
        Some(arg) if arg.starts_with('-') => parse_review_relative_offset(&arg[1..], today),
        Some(arg) => parse_review_explicit_date(arg, today),
    }
}

fn parse_review_relative_offset(digits: &str, today: NaiveDate) -> JournalCommand {
    match digits.parse::<u32>() {
        Ok(0) | Err(_) => JournalCommand::ReviewUsage,
        Ok(offset) if offset > MAX_REVIEW_OFFSET => JournalCommand::ReviewError {
            message: format!(
                "Offset -{offset} exceeds the maximum allowed offset of {MAX_REVIEW_OFFSET} days."
            ),
        },
        Ok(offset) => JournalCommand::ReviewDate {
            date: today - chrono::Duration::days(i64::from(offset)),
        },
    }
}

fn parse_review_explicit_date(arg: &str, today: NaiveDate) -> JournalCommand {
    match NaiveDate::parse_from_str(arg, "%Y-%m-%d") {
        Ok(date) if date > today => JournalCommand::ReviewError {
            message: format!(
                "Date {date} is in the future. Only past and present dates are supported."
            ),
        },
        Ok(date) => JournalCommand::ReviewDate { date },
        Err(_) => JournalCommand::ReviewUsage,
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
        assert_eq!(cmd("/start"), Some(JournalCommand::Start));
    }

    #[test]
    fn parse_help_command() {
        assert_eq!(cmd("/help"), Some(JournalCommand::Help));
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
    fn parse_review_no_argument_returns_today() {
        assert_eq!(cmd("/review"), Some(JournalCommand::ReviewToday));
        assert_eq!(cmd("/review "), Some(JournalCommand::ReviewToday));
    }

    #[test]
    fn parse_review_today_command() {
        assert_eq!(cmd("/review today"), Some(JournalCommand::ReviewToday));
    }

    #[test]
    fn parse_review_today_command_strips_bot_name_suffix() {
        assert_eq!(
            cmd("/review@mybot today"),
            Some(JournalCommand::ReviewToday)
        );
        assert_eq!(cmd("/review@mybot"), Some(JournalCommand::ReviewToday));
    }

    #[test]
    fn parse_review_explicit_date_returns_review_date() {
        assert_eq!(
            cmd("/review 2026-04-29"),
            Some(JournalCommand::ReviewDate {
                date: NaiveDate::from_ymd_opt(2026, 4, 29).unwrap()
            })
        );
        assert_eq!(
            cmd("/review 2026-04-28"),
            Some(JournalCommand::ReviewDate {
                date: NaiveDate::from_ymd_opt(2026, 4, 28).unwrap()
            })
        );
    }

    #[test]
    fn parse_review_explicit_date_strips_bot_name_suffix() {
        assert_eq!(
            cmd("/review@mybot 2026-04-28"),
            Some(JournalCommand::ReviewDate {
                date: NaiveDate::from_ymd_opt(2026, 4, 28).unwrap()
            })
        );
    }

    #[test]
    fn parse_review_future_date_returns_error() {
        assert_eq!(
            cmd("/review 2026-04-30"),
            Some(JournalCommand::ReviewError {
                message:
                    "Date 2026-04-30 is in the future. Only past and present dates are supported."
                        .to_string()
            })
        );
    }

    #[test]
    fn parse_review_relative_offset_returns_review_date() {
        assert_eq!(
            cmd("/review -1"),
            Some(JournalCommand::ReviewDate {
                date: NaiveDate::from_ymd_opt(2026, 4, 28).unwrap()
            })
        );
        assert_eq!(
            cmd("/review -7"),
            Some(JournalCommand::ReviewDate {
                date: NaiveDate::from_ymd_opt(2026, 4, 22).unwrap()
            })
        );
    }

    #[test]
    fn parse_review_relative_offset_strips_bot_name_suffix() {
        assert_eq!(
            cmd("/review@mybot -1"),
            Some(JournalCommand::ReviewDate {
                date: NaiveDate::from_ymd_opt(2026, 4, 28).unwrap()
            })
        );
    }

    #[test]
    fn parse_review_zero_offset_returns_usage() {
        assert_eq!(cmd("/review -0"), Some(JournalCommand::ReviewUsage));
    }

    #[test]
    fn parse_review_positive_offset_returns_usage() {
        assert_eq!(cmd("/review +1"), Some(JournalCommand::ReviewUsage));
        assert_eq!(cmd("/review 1"), Some(JournalCommand::ReviewUsage));
    }

    #[test]
    fn parse_review_offset_exceeding_max_returns_error() {
        assert_eq!(
            cmd("/review -366"),
            Some(JournalCommand::ReviewError {
                message: "Offset -366 exceeds the maximum allowed offset of 365 days.".to_string()
            })
        );
    }

    #[test]
    fn parse_review_invalid_date_format_returns_usage() {
        assert_eq!(cmd("/review 04-29"), Some(JournalCommand::ReviewUsage));
        assert_eq!(cmd("/review 2026-13-01"), Some(JournalCommand::ReviewUsage));
        assert_eq!(cmd("/review not-a-date"), Some(JournalCommand::ReviewUsage));
    }

    #[test]
    fn parse_review_natural_language_returns_usage() {
        assert_eq!(cmd("/review yesterday"), Some(JournalCommand::ReviewUsage));
        assert_eq!(cmd("/review monday"), Some(JournalCommand::ReviewUsage));
        assert_eq!(cmd("/review last week"), Some(JournalCommand::ReviewUsage));
    }

    #[test]
    fn parse_review_extra_words_after_today_returns_usage() {
        assert_eq!(
            cmd("/review today extra"),
            Some(JournalCommand::ReviewUsage)
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
        assert_eq!(cmd("/other"), None);
    }

    fn telegram_message(value: serde_json::Value) -> Message {
        serde_json::from_value(value).unwrap()
    }
}
