//! Shared Telegram delivery for the review workers.
//!
//! The daily and weekly delivery workers send their reviews the same way:
//! parse the conversation id into a chat id, skip chats outside the
//! configured user scope, and send the text. This module holds that logic
//! once; each worker keeps its own sender trait for test doubles.

use async_trait::async_trait;
use teloxide::{prelude::*, types::ChatId};
use tracing::info;

use crate::workers::{daily_review::DailyReviewSender, weekly_review::WeeklyReviewSender};

/// Outcome of attempting to deliver a review to a conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewSendOutcome {
    Sent,
    Skipped,
}

#[derive(Clone)]
pub struct TelegramReviewSender {
    bot: Bot,
    allowed_user_ids: Option<Vec<u64>>,
}

impl TelegramReviewSender {
    pub fn new(bot_token: String, allowed_user_ids: Option<Vec<u64>>) -> Self {
        Self {
            bot: Bot::new(bot_token),
            allowed_user_ids,
        }
    }

    async fn send_review(
        &self,
        review_kind: &'static str,
        source_conversation_id: &str,
        text: &str,
    ) -> Result<ReviewSendOutcome, String> {
        let chat_id = source_conversation_id
            .parse::<i64>()
            .map_err(|_| format!("invalid Telegram chat id: {source_conversation_id}"))?;

        if let Some(ref allowed_user_ids) = self.allowed_user_ids
            && (chat_id < 0 || !allowed_user_ids.contains(&(chat_id as u64)))
        {
            info!(
                review_kind,
                source_conversation_id,
                ?allowed_user_ids,
                "skipping review delivery outside configured Telegram user scope"
            );
            return Ok(ReviewSendOutcome::Skipped);
        }

        self.bot
            .send_message(ChatId(chat_id), text.to_string())
            .await
            .map(|_| ReviewSendOutcome::Sent)
            .map_err(|error| error.to_string())
    }
}

#[async_trait]
impl DailyReviewSender for TelegramReviewSender {
    async fn send_daily_review(
        &self,
        source_conversation_id: &str,
        text: &str,
    ) -> Result<ReviewSendOutcome, String> {
        self.send_review("daily review", source_conversation_id, text)
            .await
    }
}

#[async_trait]
impl WeeklyReviewSender for TelegramReviewSender {
    async fn send_weekly_review(
        &self,
        source_conversation_id: &str,
        text: &str,
    ) -> Result<ReviewSendOutcome, String> {
        self.send_review("weekly review", source_conversation_id, text)
            .await
    }
}
