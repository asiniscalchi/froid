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
    pub id: String,
    pub entry: JournalEntry,
}

impl AsRef<JournalEntry> for StoredJournalEntry {
    fn as_ref(&self) -> &JournalEntry {
        &self.entry
    }
}
