use chrono::{DateTime, Utc};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalEntry {
    pub text: String,
    pub received_at: DateTime<Utc>,
}

impl AsRef<JournalEntry> for JournalEntry {
    fn as_ref(&self) -> &JournalEntry {
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredJournalEntry {
    pub id: i64,
    pub entry: JournalEntry,
}

impl AsRef<JournalEntry> for StoredJournalEntry {
    fn as_ref(&self) -> &JournalEntry {
        &self.entry
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalStats {
    pub total_entries: i64,
    pub entries_today: i64,
    pub latest_received_at: Option<DateTime<Utc>>,
}
