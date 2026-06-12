//! Read-only services exposed to MCP clients.
//!
//! Each service is bound to one tenant's isolated database — user identity
//! comes from the bearer token at the HTTP layer, never from LLM-supplied
//! input — and each service caps requested limits to a maximum.

pub mod journal;
pub mod review;
pub mod semantic;
pub mod signal;
pub mod tools;
pub mod types;
mod validation;
pub mod wiring;

pub use semantic::{DefaultSemanticJournalSearcher, SemanticJournalSearcher};
pub use wiring::{AnalyzerMcpComponents, build_analyzer_mcp_components};
