use teloxide::{
    net::Download,
    payloads::SetMessageReactionSetters,
    prelude::*,
    sugar::bot::BotMessagesExt,
    types::{Message, ReactionType},
    utils::command::BotCommands,
};
use tracing::{error, info, warn};

use crate::{
    handler::MessageHandler,
    journal::command::{DEFAULT_RECENT_LIMIT, JournalCommand, JournalCommandRequest},
    journal::transfer::{TransferError, TransferService},
    messages::{IncomingMessage, MessageSource},
    tokens::TokenIssuer,
};

const UNSUPPORTED_MESSAGE_RESPONSE: &str = "Unsupported message type";

/// Import files larger than this are rejected before download. The Bot API
/// caps bot downloads at ~20 MB anyway; a froid export of that size would be
/// decades of text entries.
const MAX_IMPORT_BYTES: u32 = 10 * 1024 * 1024;

pub struct TelegramAdapter<H: MessageHandler> {
    bot_token: String,
    allowed_user_ids: Option<Vec<u64>>,
    handler: H,
    token_issuer: Option<TokenIssuer>,
    transfer: Option<TransferService>,
}

impl<H: MessageHandler> TelegramAdapter<H> {
    pub fn new(bot_token: String, allowed_user_ids: Option<Vec<u64>>, handler: H) -> Self {
        Self {
            bot_token,
            allowed_user_ids,
            handler,
            token_issuer: None,
            transfer: None,
        }
    }

    /// Attach the issuer backing the `/token` command (the central token store).
    pub fn with_token_issuer(mut self, token_issuer: TokenIssuer) -> Self {
        self.token_issuer = Some(token_issuer);
        self
    }

    /// Attach the service backing the `/export` and `/import` commands.
    pub fn with_transfer(mut self, transfer: TransferService) -> Self {
        self.transfer = Some(transfer);
        self
    }

    pub async fn run(self) {
        let bot = Bot::new(self.bot_token);
        let allowed_user_ids = self.allowed_user_ids;
        let handler = self.handler;
        let token_issuer = self.token_issuer;
        let transfer = self.transfer;

        match &allowed_user_ids {
            Some(ids) => {
                info!(
                    allowed_user_ids = ?ids,
                    chat_scope = "private",
                    "starting Telegram adapter with whitelist"
                );
            }
            None => {
                info!(
                    allowed_user_ids = "all",
                    chat_scope = "private",
                    "starting Telegram adapter without whitelist"
                );
            }
        }

        // The bot username is needed to parse commands like /cmd@botname.
        let bot_username = match bot.get_me().await {
            Ok(me) => me.username().to_string(),
            Err(err) => {
                warn!(%err, "failed to fetch bot identity; /cmd@botname forms will not parse");
                String::new()
            }
        };

        // Register the command list with Telegram so clients show the
        // autocomplete menu. Derived from the same enum as parsing and /help.
        if let Err(err) = bot.set_my_commands(Command::bot_commands()).await {
            warn!(%err, "failed to register the bot command menu with Telegram");
        }

        teloxide::repl(bot, move |bot: Bot, message: Message| {
            let handler = handler.clone();
            let allowed_user_ids = allowed_user_ids.clone();
            let token_issuer = token_issuer.clone();
            let transfer = transfer.clone();
            let bot_username = bot_username.clone();

            async move {
                handle_message(
                    bot,
                    message,
                    allowed_user_ids,
                    handler,
                    token_issuer,
                    transfer,
                    &bot_username,
                )
                .await
            }
        })
        .await;
    }
}

/// All bot commands: the single source of truth for parsing, the `/help`
/// text, and the command menu registered with Telegram at startup.
#[derive(BotCommands, Clone, Debug, PartialEq)]
#[command(rename_rule = "snake_case")]
enum Command {
    #[command(description = "start journaling")]
    Start,
    #[command(description = "show commands")]
    Help,
    #[command(description = "show latest entry")]
    Last,
    #[command(description = "delete latest entry")]
    Undo,
    #[command(description = "show recent entries (optionally how many)")]
    Recent(String),
    #[command(description = "show today's entries")]
    Today,
    #[command(description = "show daily review")]
    DayReview,
    #[command(description = "show last week's review")]
    WeekReview,
    #[command(description = "show journal stats")]
    Stats,
    #[command(description = "show bot status")]
    Status,
    #[command(description = "search entries by meaning")]
    Search(String),
    #[command(description = "create or rotate your MCP access token (/token revoke to disable)")]
    Token(String),
    #[command(description = "download your journal as a JSON file")]
    Export,
    #[command(description = "send an export file with /import as the caption to load it")]
    Import,
}

/// Where a parsed command is handled.
#[derive(Debug, PartialEq)]
enum Dispatch {
    /// Forwarded to the per-tenant journal service.
    Journal(JournalCommand),
    Help,
    Token(TokenAction),
    Export,
    /// `/import` sent as plain text — the file must come as a document.
    ImportUsage,
}

fn dispatch_for(command: Command) -> Dispatch {
    match command {
        Command::Start => Dispatch::Journal(JournalCommand::Start),
        Command::Help => Dispatch::Help,
        Command::Last => Dispatch::Journal(JournalCommand::Last),
        Command::Undo => Dispatch::Journal(JournalCommand::Undo),
        Command::Recent(argument) => {
            let argument = argument.trim();
            let command = if argument.is_empty() {
                JournalCommand::Recent {
                    requested_limit: DEFAULT_RECENT_LIMIT,
                }
            } else {
                match argument.parse::<u32>() {
                    Ok(limit) if limit > 0 => JournalCommand::Recent {
                        requested_limit: limit,
                    },
                    _ => JournalCommand::RecentUsage,
                }
            };
            Dispatch::Journal(command)
        }
        Command::Today => Dispatch::Journal(JournalCommand::Today),
        Command::DayReview => Dispatch::Journal(JournalCommand::DayReviewLast),
        Command::WeekReview => Dispatch::Journal(JournalCommand::WeekReviewLast),
        Command::Stats => Dispatch::Journal(JournalCommand::Stats),
        Command::Status => Dispatch::Journal(JournalCommand::Status),
        Command::Search(query) => {
            let query = query.trim();
            let command = if query.is_empty() {
                JournalCommand::SearchUsage
            } else {
                JournalCommand::Search {
                    query: query.to_string(),
                }
            };
            Dispatch::Journal(command)
        }
        Command::Token(argument) => Dispatch::Token(match argument.trim() {
            "" => TokenAction::Issue,
            "revoke" => TokenAction::Revoke,
            _ => TokenAction::Usage,
        }),
        Command::Export => Dispatch::Export,
        Command::Import => Dispatch::ImportUsage,
    }
}

fn help_text() -> String {
    format!("Commands:\n{}", Command::descriptions())
}

fn unknown_command_reply(text: &str) -> String {
    let command = text.split_whitespace().next().unwrap_or(text);
    format!("Unknown command: {command}\n\n{}", help_text())
}

async fn handle_message<H: MessageHandler>(
    bot: Bot,
    message: Message,
    allowed_user_ids: Option<Vec<u64>>,
    handler: H,
    token_issuer: Option<TokenIssuer>,
    transfer: Option<TransferService>,
    bot_username: &str,
) -> ResponseResult<()> {
    if !should_handle_message(&message, allowed_user_ids.as_deref()) {
        info!(
            chat_id = %message.chat.id,
            sender_user_id = message.from.as_ref().map(|user| user.id.0),
            ?allowed_user_ids,
            "ignored Telegram message outside configured private user scope"
        );
        return Ok(());
    }

    // A document with "/import" as its caption is an import request; any
    // other non-text message stays unsupported.
    if let Some(document) = message.document() {
        if is_import_caption(message.caption().unwrap_or("")) {
            info!(chat_id = %message.chat.id, "received Telegram /import document");
            return handle_import_document(&bot, &message, document, transfer.as_ref()).await;
        }
        bot.send_message(message.chat.id, UNSUPPORTED_MESSAGE_RESPONSE)
            .await?;
        return Ok(());
    }

    let Some(text) = message.text() else {
        bot.send_message(message.chat.id, UNSUPPORTED_MESSAGE_RESPONSE)
            .await?;
        return Ok(());
    };

    let trimmed = text.trim_start();
    if trimmed.starts_with('/') {
        info!(chat_id = %message.chat.id, "received Telegram command");
        let Ok(command) = Command::parse(trimmed, bot_username) else {
            bot.send_message(message.chat.id, unknown_command_reply(trimmed))
                .await?;
            return Ok(());
        };

        match dispatch_for(command) {
            Dispatch::Journal(command) => {
                let request = JournalCommandRequest {
                    source: MessageSource::Telegram,
                    source_conversation_id: message.chat.id.to_string(),
                    received_at: message.date,
                    command,
                };

                match handler.command(&request).await {
                    Ok(outgoing) => {
                        bot.send_message(message.chat.id, outgoing.text).await?;
                    }
                    Err(err) => {
                        error!(%err, "failed to process journal command");
                    }
                }
            }
            Dispatch::Help => {
                bot.send_message(message.chat.id, help_text()).await?;
            }
            Dispatch::Token(action) => {
                let reply = handle_token_command(
                    token_issuer.as_ref(),
                    action,
                    &message.chat.id.to_string(),
                )
                .await;
                bot.send_message(message.chat.id, reply).await?;
            }
            Dispatch::Export => {
                return handle_export_command(&bot, &message, transfer.as_ref()).await;
            }
            Dispatch::ImportUsage => {
                bot.send_message(
                    message.chat.id,
                    "To import, send your froid export JSON file as a document with /import as the caption.",
                )
                .await?;
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

fn should_handle_message(message: &Message, allowed_user_ids: Option<&[u64]>) -> bool {
    if !message.chat.is_private() {
        return false;
    }

    let Some(sender) = message.from.as_ref() else {
        return false;
    };

    allowed_user_ids.is_none_or(|ids| ids.contains(&sender.id.0))
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
        text: message.text().unwrap_or_default().to_string(),
        received_at: message.date,
    }
}

/// Whether a document caption requests an import.
fn is_import_caption(caption: &str) -> bool {
    caption
        .split_whitespace()
        .next()
        .and_then(|command| command.split('@').next())
        == Some("/import")
}

async fn handle_export_command(
    bot: &Bot,
    message: &Message,
    transfer: Option<&TransferService>,
) -> ResponseResult<()> {
    let chat_id = message.chat.id.to_string();
    let Some(transfer) = transfer else {
        error!(
            chat_id,
            "received /export but no transfer service is attached"
        );
        bot.send_message(
            message.chat.id,
            "Export is not available right now. Please try again later.",
        )
        .await?;
        return Ok(());
    };

    match transfer.export(&chat_id).await {
        Ok(export) => {
            let document =
                teloxide::types::InputFile::memory(export.bytes).file_name(export.filename.clone());
            bot.send_document(message.chat.id, document)
                .caption(format!(
                    "Your journal export — {} messages.",
                    export.message_count
                ))
                .await?;
        }
        Err(err) => {
            error!(%err, chat_id, "failed to export journal");
            bot.send_message(
                message.chat.id,
                "Something went wrong exporting your journal. Please try again.",
            )
            .await?;
        }
    }
    Ok(())
}

async fn handle_import_document(
    bot: &Bot,
    message: &Message,
    document: &teloxide::types::Document,
    transfer: Option<&TransferService>,
) -> ResponseResult<()> {
    let chat_id = message.chat.id.to_string();
    let Some(transfer) = transfer else {
        error!(
            chat_id,
            "received /import but no transfer service is attached"
        );
        bot.send_message(
            message.chat.id,
            "Import is not available right now. Please try again later.",
        )
        .await?;
        return Ok(());
    };

    if document.file.size > MAX_IMPORT_BYTES {
        bot.send_message(
            message.chat.id,
            format!(
                "That file is too large to import over Telegram (max {} MB).",
                MAX_IMPORT_BYTES / (1024 * 1024)
            ),
        )
        .await?;
        return Ok(());
    }

    let file = bot.get_file(document.file.id.clone()).await?;
    let mut payload: Vec<u8> = Vec::with_capacity(document.file.size as usize);
    if let Err(err) = bot.download_file(&file.path, &mut payload).await {
        error!(%err, chat_id, "failed to download import document");
        bot.send_message(
            message.chat.id,
            "Something went wrong downloading the file. Please try again.",
        )
        .await?;
        return Ok(());
    }

    let reply = import_reply(transfer, &chat_id, &payload).await;
    bot.send_message(message.chat.id, reply).await?;
    Ok(())
}

/// Run the import and render the outcome as a chat reply.
async fn import_reply(transfer: &TransferService, chat_id: &str, payload: &[u8]) -> String {
    match transfer.import(chat_id, payload).await {
        Ok(0) => "The file contained no messages; nothing was imported.".to_string(),
        Ok(1) => "Imported 1 message.".to_string(),
        Ok(count) => format!("Imported {count} messages."),
        Err(
            err @ (TransferError::InvalidPayload(_)
            | TransferError::UnsupportedVersion { .. }
            | TransferError::Conflict { .. }),
        ) => err.to_string(),
        Err(err) => {
            error!(%err, chat_id, "failed to import journal");
            "Something went wrong importing your journal. Please try again.".to_string()
        }
    }
}

/// Action requested via the `/token` command.
#[derive(Debug, Clone, PartialEq, Eq)]
enum TokenAction {
    Issue,
    Revoke,
    Usage,
}

async fn handle_token_command(
    issuer: Option<&TokenIssuer>,
    action: TokenAction,
    chat_id: &str,
) -> String {
    let Some(issuer) = issuer else {
        // Defensive: serve() always attaches an issuer; this only triggers if
        // the adapter was built without one.
        error!(chat_id, "received /token but no token issuer is attached");
        return "Access tokens are not available right now. Please try again later.".to_string();
    };

    match action {
        TokenAction::Issue => match issuer.issue(chat_id).await {
            Ok(token) => format!(
                "Your new access token:\n\n{token}\n\nUse it as a bearer token for MCP \
                 clients (Authorization: Bearer …). It replaces any previous token and \
                 is shown only once — treat it like a password. Send /token again to \
                 rotate it, or /token revoke to disable access."
            ),
            Err(err) => {
                error!(%err, chat_id, "failed to issue access token");
                "Something went wrong issuing your token. Please try again.".to_string()
            }
        },
        TokenAction::Revoke => match issuer.revoke(chat_id).await {
            Ok(true) => {
                "Your access token has been revoked. Send /token to create a new one.".to_string()
            }
            Ok(false) => "You have no active access token. Send /token to create one.".to_string(),
            Err(err) => {
                error!(%err, chat_id, "failed to revoke access token");
                "Something went wrong revoking your token. Please try again.".to_string()
            }
        },
        TokenAction::Usage => {
            "Usage: /token to create or rotate your access token, /token revoke to disable it."
                .to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::messages::MessageSource;

    /// Parse a command the way `handle_message` does and return its dispatch.
    fn cmd(text: &str) -> Option<Dispatch> {
        Command::parse(text.trim_start(), "mybot")
            .ok()
            .map(dispatch_for)
    }

    fn journal(text: &str) -> Option<JournalCommand> {
        match cmd(text)? {
            Dispatch::Journal(command) => Some(command),
            other => panic!("expected journal dispatch, got {other:?}"),
        }
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

        assert!(should_handle_message(&message, Some(&[7])));
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

        assert!(!should_handle_message(&message, Some(&[7])));
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

        assert!(!should_handle_message(&message, Some(&[7])));
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

        assert!(!should_handle_message(&message, Some(&[7])));
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
    fn parse_journal_commands() {
        assert_eq!(journal("/start"), Some(JournalCommand::Start));
        assert_eq!(journal("/last"), Some(JournalCommand::Last));
        assert_eq!(journal("/undo"), Some(JournalCommand::Undo));
        assert_eq!(journal("/today"), Some(JournalCommand::Today));
        assert_eq!(journal("/stats"), Some(JournalCommand::Stats));
        assert_eq!(journal("/status"), Some(JournalCommand::Status));
        assert_eq!(journal("/day_review"), Some(JournalCommand::DayReviewLast));
        assert_eq!(
            journal("/week_review"),
            Some(JournalCommand::WeekReviewLast)
        );
    }

    #[test]
    fn parse_strips_bot_name_suffix() {
        assert_eq!(journal("/last@mybot"), Some(JournalCommand::Last));
        assert_eq!(journal("/status@mybot"), Some(JournalCommand::Status));
        assert_eq!(
            journal("/recent@mybot 3"),
            Some(JournalCommand::Recent { requested_limit: 3 })
        );
        assert_eq!(
            journal("/search@mybot something"),
            Some(JournalCommand::Search {
                query: "something".to_string()
            })
        );
    }

    #[test]
    fn parse_recent_command_arguments() {
        assert_eq!(
            journal("/recent"),
            Some(JournalCommand::Recent {
                requested_limit: DEFAULT_RECENT_LIMIT
            })
        );
        assert_eq!(
            journal("/recent 5"),
            Some(JournalCommand::Recent { requested_limit: 5 })
        );
        assert_eq!(journal("/recent abc"), Some(JournalCommand::RecentUsage));
        assert_eq!(journal("/recent 0"), Some(JournalCommand::RecentUsage));
        assert_eq!(journal("/recent -3"), Some(JournalCommand::RecentUsage));
    }

    #[test]
    fn parse_search_command_arguments() {
        assert_eq!(
            journal("/search anxiety before meetings"),
            Some(JournalCommand::Search {
                query: "anxiety before meetings".to_string()
            })
        );
        assert_eq!(journal("/search"), Some(JournalCommand::SearchUsage));
    }

    #[test]
    fn parse_adapter_handled_commands() {
        assert_eq!(cmd("/help"), Some(Dispatch::Help));
        assert_eq!(cmd("/export"), Some(Dispatch::Export));
        assert_eq!(cmd("/export@mybot"), Some(Dispatch::Export));
        assert_eq!(cmd("/import"), Some(Dispatch::ImportUsage));
        assert_eq!(cmd("/token"), Some(Dispatch::Token(TokenAction::Issue)));
        assert_eq!(
            cmd("/token revoke"),
            Some(Dispatch::Token(TokenAction::Revoke))
        );
        assert_eq!(
            cmd("/token nonsense"),
            Some(Dispatch::Token(TokenAction::Usage))
        );
    }

    #[test]
    fn parse_rejects_non_commands_and_unknown_commands() {
        assert_eq!(cmd("hello"), None);
        assert_eq!(cmd("/other"), None);
        assert_eq!(cmd("   /other with text"), None);
    }

    #[test]
    fn unknown_command_reply_names_the_command_and_shows_help() {
        let reply = unknown_command_reply("/other with text");

        assert!(reply.starts_with("Unknown command: /other"));
        assert!(reply.contains("/help"));
    }

    #[test]
    fn help_text_covers_every_registered_command() {
        let help = help_text();

        for registered in Command::bot_commands() {
            assert!(
                help.contains(&registered.command),
                "help text is missing {}",
                registered.command
            );
        }
        assert!(help.contains("/recent"));
        assert!(help.contains("/token"));
        assert!(help.contains("/export"));
    }

    #[test]
    fn import_caption_detection() {
        assert!(is_import_caption("/import"));
        assert!(is_import_caption("  /import  "));
        assert!(is_import_caption("/import@mybot"));
        assert!(is_import_caption("/import my backup"));
        assert!(!is_import_caption(""));
        assert!(!is_import_caption("holiday photo"));
        assert!(!is_import_caption("import"));
    }

    mod transfer_commands {
        use super::super::import_reply;
        use crate::journal::transfer::TransferService;
        use crate::journal::{
            extraction::JournalEntryExtractionRuntimeConfig,
            registry::{JournalServiceRegistry, JournalServiceRegistryConfig},
            review::DailyReviewRuntimeConfig,
            review::signals::wiring::DailyReviewSignalRuntimeConfig,
            week_review::WeeklyReviewRuntimeConfig,
        };
        use clap::Parser;
        use tokio_util::sync::CancellationToken;

        async fn transfer_service() -> TransferService {
            let test_id = ulid::Ulid::new().to_string();
            let temp_base_dir = std::env::temp_dir().join(format!("froid_test_transfer_{test_id}"));
            tokio::fs::create_dir_all(&temp_base_dir).await.unwrap();

            let cli = crate::cli::Cli::try_parse_from([
                "froid",
                "--telegram-bot-token",
                "mock_telegram_token_123",
                "--data-dir",
                temp_base_dir.to_str().unwrap(),
            ])
            .unwrap();

            let registry = JournalServiceRegistry::new(JournalServiceRegistryConfig {
                config: cli.serve_config().unwrap(),
                embedding_config: None,
                entry_extraction_config: JournalEntryExtractionRuntimeConfig::from_env(),
                daily_review_config: DailyReviewRuntimeConfig::from_env(),
                weekly_review_config: WeeklyReviewRuntimeConfig::from_env(),
                signal_runtime_config: DailyReviewSignalRuntimeConfig::from_env(),
                delivery_configured: false,
                shutdown: CancellationToken::new(),
            })
            .with_base_dir(temp_base_dir);

            TransferService::new(registry)
        }

        #[tokio::test]
        async fn export_then_import_roundtrips_between_users() {
            let transfer = transfer_service().await;

            // Alice exports her journal entry...
            transfer
                .import(
                    "111",
                    serde_json::json!({
                        "version": 2,
                        "messages": [{
                            "source": "telegram",
                            "source_conversation_id": "111",
                            "source_message_id": "m1",
                            "text": "alice note",
                            "received_at": "2026-06-01T10:00:00Z"
                        }]
                    })
                    .to_string()
                    .as_bytes(),
                )
                .await
                .unwrap();
            let export = transfer.export("111").await.unwrap();
            assert_eq!(export.message_count, 1);

            // ...and it can be imported into Bob's isolated journal.
            let reply = import_reply(&transfer, "222", &export.bytes).await;
            assert_eq!(reply, "Imported 1 message.");

            let bobs = transfer.export("222").await.unwrap();
            assert_eq!(bobs.message_count, 1);
        }

        #[tokio::test]
        async fn import_reply_reports_conflicts() {
            let transfer = transfer_service().await;
            let payload = serde_json::json!({
                "version": 2,
                "messages": [{
                    "source": "telegram",
                    "source_conversation_id": "111",
                    "source_message_id": "m1",
                    "text": "duplicate",
                    "received_at": "2026-06-01T10:00:00Z"
                }]
            })
            .to_string();

            assert_eq!(
                import_reply(&transfer, "111", payload.as_bytes()).await,
                "Imported 1 message."
            );
            let reply = import_reply(&transfer, "111", payload.as_bytes()).await;
            assert!(reply.contains("collides"), "got: {reply}");
        }

        #[tokio::test]
        async fn import_reply_reports_invalid_files() {
            let transfer = transfer_service().await;

            let reply = import_reply(&transfer, "111", b"not json").await;

            assert!(reply.contains("invalid export file"), "got: {reply}");
        }

        #[tokio::test]
        async fn import_reply_reports_empty_envelopes() {
            let transfer = transfer_service().await;
            let payload = serde_json::json!({"version": 2, "messages": []}).to_string();

            let reply = import_reply(&transfer, "111", payload.as_bytes()).await;

            assert!(reply.contains("nothing was imported"));
        }
    }

    mod token_command {
        use super::super::{TokenAction, handle_token_command};

        use crate::tokens::{TokenIssuer, UserTokenStore, hash_token};

        async fn issuer() -> (TokenIssuer, UserTokenStore) {
            let pool = crate::database::test_pool().await;
            let store = UserTokenStore::new(pool);
            (TokenIssuer::new(store.clone()), store)
        }

        #[tokio::test]
        async fn replies_gracefully_when_no_issuer_is_attached() {
            let reply = handle_token_command(None, TokenAction::Issue, "42").await;

            assert!(reply.contains("not available"));
        }

        #[tokio::test]
        async fn issue_returns_working_token() {
            let (issuer, store) = issuer().await;

            let reply = handle_token_command(Some(&issuer), TokenAction::Issue, "42").await;

            let token = reply
                .split_whitespace()
                .find(|word| word.starts_with("froid_"))
                .expect("reply contains the token");
            assert_eq!(
                store
                    .find_chat_id_by_hash(&hash_token(token))
                    .await
                    .unwrap(),
                Some("42".to_string())
            );
        }

        #[tokio::test]
        async fn revoke_disables_the_token() {
            let (issuer, store) = issuer().await;
            let token = issuer.issue("42").await.unwrap();

            let reply = handle_token_command(Some(&issuer), TokenAction::Revoke, "42").await;

            assert!(reply.contains("revoked"));
            assert_eq!(
                store
                    .find_chat_id_by_hash(&hash_token(&token))
                    .await
                    .unwrap(),
                None
            );
        }

        #[tokio::test]
        async fn revoke_without_token_says_so() {
            let (issuer, _) = issuer().await;

            let reply = handle_token_command(Some(&issuer), TokenAction::Revoke, "42").await;

            assert!(reply.contains("no active access token"));
        }

        #[tokio::test]
        async fn usage_reply_mentions_both_forms() {
            let (issuer, _) = issuer().await;

            let reply = handle_token_command(Some(&issuer), TokenAction::Usage, "42").await;

            assert!(reply.contains("/token revoke"));
        }
    }

    fn telegram_message(value: serde_json::Value) -> Message {
        serde_json::from_value(value).unwrap()
    }
}
