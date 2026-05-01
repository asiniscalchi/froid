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
    pub reaction: Option<OutgoingReaction>,
}

impl OutgoingMessage {
    pub fn text(text: String) -> Self {
        Self {
            text,
            reaction: None,
        }
    }

    pub fn with_reaction(text: String, reaction: OutgoingReaction) -> Self {
        Self {
            text,
            reaction: Some(reaction),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutgoingReaction {
    MessageSaved,
    JournalEntryExtracted,
}
