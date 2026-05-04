//! Read-only services exposed to the analyzer bot.
//!
//! Every method takes a [`UserContext`] populated from authenticated session
//! state — `user_id` is never an LLM-supplied argument. Each service enforces
//! user scoping internally and caps requested limits to a maximum.

pub mod journal;
pub mod review;
pub mod types;
mod validation;

pub use journal::{DefaultJournalReadService, JournalReadService};
pub use review::{DefaultReviewReadService, ReviewReadService};
pub use types::{
    AnalyzerError, DailyReviewView, GetRecentRequest, GetReviewsRequest, JournalEntryView,
    MAX_RECENT_LIMIT, MAX_TEXT_SEARCH_LIMIT, SearchTextRequest, UserContext, WeeklyReviewView,
};
