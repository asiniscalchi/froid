use std::sync::Arc;

use teloxide::{prelude::*, types::Message};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::journal::analyzer::{AnalyzerAgent, UserContext};

const UNSUPPORTED_MESSAGE_RESPONSE: &str = "Send a question as plain text.";
const ANALYZER_FAILURE_RESPONSE: &str = "Sorry, I couldn't answer that. Please try again.";

pub struct AnalyzerTelegramAdapter {
    bot_token: String,
    agent: Arc<dyn AnalyzerAgent>,
}

impl AnalyzerTelegramAdapter {
    pub fn new(bot_token: String, agent: Arc<dyn AnalyzerAgent>) -> Self {
        Self { bot_token, agent }
    }

    /// Run the analyzer bot until `shutdown` is cancelled or the dispatcher
    /// exits. Designed to be spawned alongside other workers from `serve()`.
    pub async fn run_until_cancelled(self, shutdown: CancellationToken) {
        let bot = Bot::new(self.bot_token);
        let agent = self.agent;

        let dispatcher_future = teloxide::repl(bot, move |bot: Bot, message: Message| {
            let agent = Arc::clone(&agent);
            async move { handle_message(bot, message, agent).await }
        });

        tokio::select! {
            () = dispatcher_future => {
                warn!("analyzer telegram dispatcher exited");
            }
            () = shutdown.cancelled() => {
                info!("analyzer telegram adapter shutting down");
            }
        }
    }
}

async fn handle_message(
    bot: Bot,
    message: Message,
    agent: Arc<dyn AnalyzerAgent>,
) -> ResponseResult<()> {
    let Some(text) = message.text() else {
        bot.send_message(message.chat.id, UNSUPPORTED_MESSAGE_RESPONSE)
            .await?;
        return Ok(());
    };

    let user_id = derive_user_id(&message);
    let ctx = UserContext::new(user_id.clone());
    let question = text.to_string();

    info!(user_id = %user_id, "analyzer received question");

    match agent.ask(ctx, question).await {
        Ok(reply) => {
            let reply = if reply.trim().is_empty() {
                ANALYZER_FAILURE_RESPONSE.to_string()
            } else {
                reply
            };
            bot.send_message(message.chat.id, reply).await?;
        }
        Err(err) => {
            error!(%err, user_id = %user_id, "analyzer agent failed");
            bot.send_message(message.chat.id, ANALYZER_FAILURE_RESPONSE)
                .await?;
        }
    }

    Ok(())
}

fn derive_user_id(message: &Message) -> String {
    message
        .from
        .as_ref()
        .map(|user| user.id.to_string())
        .unwrap_or_else(|| message.chat.id.to_string())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn telegram_message(value: serde_json::Value) -> Message {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn derive_user_id_prefers_sender_id() {
        let message = telegram_message(json!({
            "message_id": 1,
            "from": {"id": 7, "is_bot": false, "first_name": "Ada"},
            "date": 1_700_000_000,
            "chat": {"id": 42, "type": "private", "first_name": "Ada"},
            "text": "what patterns do you see?"
        }));

        assert_eq!(derive_user_id(&message), "7");
    }

    #[test]
    fn derive_user_id_falls_back_to_chat_id_when_sender_missing() {
        let message = telegram_message(json!({
            "message_id": 1,
            "date": 1_700_000_000,
            "chat": {"id": 42, "type": "private", "first_name": "Ada"},
            "text": "anonymous"
        }));

        assert_eq!(derive_user_id(&message), "42");
    }
}
