use chrono::{DateTime, NaiveDate, Utc};

use crate::messages::MessageSource;

pub const DEFAULT_RECENT_LIMIT: u32 = 10;
pub const MAX_RECENT_LIMIT: u32 = 50;
pub const MAX_REVIEW_OFFSET: u32 = 365;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalCommandRequest {
    pub source: MessageSource,
    pub source_conversation_id: String,
    pub user_id: String,
    pub received_at: DateTime<Utc>,
    pub command: JournalCommand,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JournalCommand {
    Start,
    Help,
    Last,
    Undo,
    Recent { requested_limit: u32 },
    RecentUsage,
    Today,
    Stats,
    Status,
    ReviewToday,
    ReviewDate { date: NaiveDate },
    ReviewUsage,
    ReviewError { message: String },
    Search { query: String },
    SearchUsage,
    Unknown { command: String },
}
