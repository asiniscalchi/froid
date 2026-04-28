use chrono::{DateTime, Utc};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalEntry {
    pub text: String,
    pub received_at: DateTime<Utc>,
}
