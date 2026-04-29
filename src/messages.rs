use chrono::{DateTime, Utc};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageSource {
    Telegram,
}

impl std::fmt::Display for MessageSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MessageSource::Telegram => write!(f, "telegram"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncomingMessage {
    pub source: MessageSource,
    pub source_conversation_id: String,
    pub source_message_id: String,
    pub user_id: String,
    pub text: String,
    pub received_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutgoingMessage {
    pub text: String,
}

pub const DEFAULT_RECENT_LIMIT: u32 = 10;
pub const MAX_RECENT_LIMIT: u32 = 50;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalCommandRequest {
    pub user_id: String,
    pub received_at: DateTime<Utc>,
    pub command: JournalCommand,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JournalCommand {
    Start,
    Help,
    Recent { requested_limit: u32 },
    RecentUsage,
    Today,
    Stats,
}
