use std::{error::Error, fmt};

use chrono::{DateTime, NaiveDate, Utc};

use crate::journal::entry::StoredJournalEntry;

pub const MAX_RECENT_LIMIT: u32 = 50;
pub const MAX_TEXT_SEARCH_LIMIT: u32 = 50;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserContext {
    pub user_id: String,
}

impl UserContext {
    pub fn new(user_id: impl Into<String>) -> Self {
        Self {
            user_id: user_id.into(),
        }
    }
}

#[derive(Debug)]
pub enum AnalyzerError {
    InvalidArgument(String),
    LimitTooLarge { max: u32 },
    Internal(Box<dyn Error + Send + Sync>),
}

impl fmt::Display for AnalyzerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidArgument(message) => write!(f, "invalid argument: {message}"),
            Self::LimitTooLarge { max } => write!(f, "limit exceeds maximum (max {max})"),
            Self::Internal(source) => write!(f, "internal error: {source}"),
        }
    }
}

impl Error for AnalyzerError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Internal(source) => Some(source.as_ref()),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalEntryView {
    pub id: i64,
    pub received_at: DateTime<Utc>,
    pub text: String,
}

impl From<StoredJournalEntry> for JournalEntryView {
    fn from(stored: StoredJournalEntry) -> Self {
        Self {
            id: stored.id,
            received_at: stored.entry.received_at,
            text: stored.entry.text,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GetRecentRequest {
    pub limit: u32,
    pub from_date: Option<NaiveDate>,
    pub to_date_exclusive: Option<NaiveDate>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchTextRequest {
    pub query: String,
    pub limit: u32,
    pub from_date: Option<NaiveDate>,
    pub to_date_exclusive: Option<NaiveDate>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn analyzer_error_displays_each_variant() {
        let invalid = AnalyzerError::InvalidArgument("query is empty".to_string());
        assert_eq!(invalid.to_string(), "invalid argument: query is empty");

        let too_large = AnalyzerError::LimitTooLarge { max: 50 };
        assert_eq!(too_large.to_string(), "limit exceeds maximum (max 50)");

        let internal = AnalyzerError::Internal(Box::<dyn Error + Send + Sync>::from("boom"));
        assert_eq!(internal.to_string(), "internal error: boom");
    }

    #[test]
    fn analyzer_error_internal_exposes_source() {
        let internal = AnalyzerError::Internal(Box::<dyn Error + Send + Sync>::from("inner"));
        assert!(internal.source().is_some());

        let invalid = AnalyzerError::InvalidArgument("x".to_string());
        assert!(invalid.source().is_none());
    }

    #[test]
    fn journal_entry_view_is_built_from_stored_entry() {
        let stored = StoredJournalEntry {
            id: 42,
            entry: crate::journal::entry::JournalEntry {
                text: "hello".to_string(),
                received_at: Utc::now(),
            },
        };
        let view: JournalEntryView = stored.clone().into();
        assert_eq!(view.id, 42);
        assert_eq!(view.text, "hello");
        assert_eq!(view.received_at, stored.entry.received_at);
    }
}
