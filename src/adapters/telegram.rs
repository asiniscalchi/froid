use teloxide::{
    net::Download,
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
    journal::transfer::{TransferError, TransferService},
    messages::{IncomingMessage, MessageSource, SINGLE_USER_ID},
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

        teloxide::repl(bot, move |bot: Bot, message: Message| {
            let handler = handler.clone();
            let allowed_user_ids = allowed_user_ids.clone();
            let token_issuer = token_issuer.clone();
            let transfer = transfer.clone();

            async move {
                handle_message(
                    bot,
                    message,
                    allowed_user_ids,
                    handler,
                    token_issuer,
                    transfer,
                )
                .await
            }
        })
        .await;
    }
}

async fn handle_message<H: MessageHandler>(
    bot: Bot,
    message: Message,
    allowed_user_ids: Option<Vec<u64>>,
    handler: H,
    token_issuer: Option<TokenIssuer>,
    transfer: Option<TransferService>,
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

    match parse_transfer_command(text) {
        Some(TransferCommand::Export) => {
            info!(chat_id = %message.chat.id, "received Telegram /export command");
            return handle_export_command(&bot, &message, transfer.as_ref()).await;
        }
        Some(TransferCommand::ImportUsage) => {
            bot.send_message(
                message.chat.id,
                "To import, send your froid export JSON file as a document with /import as the caption.",
            )
            .await?;
            return Ok(());
        }
        None => {}
    }

    if let Some(action) = parse_token_command(text) {
        info!(chat_id = %message.chat.id, "received Telegram /token command");
        let reply =
            handle_token_command(token_issuer.as_ref(), action, &message.chat.id.to_string()).await;
        bot.send_message(message.chat.id, reply).await?;
        return Ok(());
    }

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
        user_id: SINGLE_USER_ID.to_string(),
        text: message.text().unwrap_or_default().to_string(),
        received_at: message.date,
    }
}

/// Data-portability command parsed from a text message.
#[derive(Debug, Clone, PartialEq, Eq)]
enum TransferCommand {
    Export,
    /// `/import` sent as plain text — the file must come as a document.
    ImportUsage,
}

fn parse_transfer_command(text: &str) -> Option<TransferCommand> {
    let command = text.split_whitespace().next()?.split('@').next()?;
    match command {
        "/export" => Some(TransferCommand::Export),
        "/import" => Some(TransferCommand::ImportUsage),
        _ => None,
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

fn parse_token_command(text: &str) -> Option<TokenAction> {
    let mut parts = text.trim().splitn(2, char::is_whitespace);
    let command = parts.next()?.split('@').next()?;
    if command != "/token" {
        return None;
    }

    match parts.next().map(str::trim).filter(|s| !s.is_empty()) {
        None => Some(TokenAction::Issue),
        Some("revoke") => Some(TokenAction::Revoke),
        Some(_) => Some(TokenAction::Usage),
    }
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
                "Your new access token:\n\n{token}\n\nUse it as a bearer token for the \
                 dashboard and MCP (Authorization: Bearer …). It replaces any previous \
                 token and is shown only once — treat it like a password. Send /token \
                 again to rotate it, or /token revoke to disable access."
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
    fn parse_transfer_command_variants() {
        assert_eq!(
            parse_transfer_command("/export"),
            Some(TransferCommand::Export)
        );
        assert_eq!(
            parse_transfer_command("/export@mybot"),
            Some(TransferCommand::Export)
        );
        assert_eq!(
            parse_transfer_command("/import"),
            Some(TransferCommand::ImportUsage)
        );
        assert_eq!(parse_transfer_command("/help"), None);
        assert_eq!(parse_transfer_command("export"), None);
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

    #[test]
    fn parse_token_command_variants() {
        assert_eq!(parse_token_command("/token"), Some(TokenAction::Issue));
        assert_eq!(
            parse_token_command("/token@mybot"),
            Some(TokenAction::Issue)
        );
        assert_eq!(
            parse_token_command("/token revoke"),
            Some(TokenAction::Revoke)
        );
        assert_eq!(
            parse_token_command("/token nonsense"),
            Some(TokenAction::Usage)
        );
        assert_eq!(parse_token_command("/help"), None);
        assert_eq!(parse_token_command("token"), None);
    }

    mod token_command {
        use super::super::{TokenAction, handle_token_command};
        use crate::database;
        use crate::tokens::{TokenIssuer, UserTokenStore, hash_token};

        async fn issuer() -> (TokenIssuer, UserTokenStore) {
            database::register_sqlite_vec_extension();
            let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
            sqlx::migrate!().run(&pool).await.unwrap();
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
