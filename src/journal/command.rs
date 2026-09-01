use chrono::{DateTime, Utc};

use crate::messages::MessageSource;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalCommandRequest {
    pub source: MessageSource,
    pub source_conversation_id: String,
    pub received_at: DateTime<Utc>,
    pub command: JournalCommand,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JournalCommand {
    Start,
    Undo,
    DayReviewLast,
    WeekReviewLast,
    Search { query: String },
    SearchUsage,
    ReviewsStatus,
    ReviewsSet(bool),
    ReviewsUsage,
}
