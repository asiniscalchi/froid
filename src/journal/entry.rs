use chrono::{DateTime, Utc};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalEntry {
    pub text: String,
    pub received_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalStats {
    pub total_entries: i64,
    pub entries_today: i64,
    pub latest_received_at: Option<DateTime<Utc>>,
}
