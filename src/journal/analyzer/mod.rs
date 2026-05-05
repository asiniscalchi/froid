//! Read-only services exposed to MCP clients.
//!
//! Every method takes a [`UserContext`] populated from authenticated session
//! state — `user_id` is never an LLM-supplied argument. Each service enforces
//! user scoping internally and caps requested limits to a maximum.

pub mod journal;
pub mod review;
pub mod semantic;
pub mod signal;
pub mod tools;
pub mod types;
mod validation;
pub mod wiring;

pub use journal::{DefaultJournalReadService, JournalReadService};
pub use review::{DefaultReviewReadService, ReviewReadService};
pub use semantic::{DefaultSemanticJournalSearcher, SemanticJournalSearcher};
pub use signal::{DefaultSignalReadService, SignalReadService};
pub use types::{
    AnalyzerError, DailyReviewView, GetRecentRequest, GetReviewsRequest, JournalEntryView,
    MAX_RECENT_LIMIT, MAX_SEMANTIC_LIMIT, MAX_SIGNAL_LIMIT, MAX_TEXT_SEARCH_LIMIT,
    SearchSemanticRequest, SearchSignalsRequest, SearchTextRequest, SemanticHit, SignalView,
    UserContext, WeeklyReviewView,
};
pub use wiring::build_analyzer_tool_registry;
