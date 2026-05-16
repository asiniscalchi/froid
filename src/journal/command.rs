use chrono::{DateTime, Utc};

use crate::journal::delivery_switch::ReviewKind;
use crate::messages::MessageSource;

pub const DEFAULT_RECENT_LIMIT: u32 = 10;
pub const MAX_RECENT_LIMIT: u32 = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewToggleAction {
    On,
    Off,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewsSubcommand {
    Toggle {
        kind: ReviewKind,
        action: ReviewToggleAction,
    },
    Status,
    Usage,
}

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
    DayReviewLast,
    WeekReviewLast,
    Search { query: String },
    SearchUsage,
    Reviews { subcommand: ReviewsSubcommand },
    Unknown { command: String },
}
