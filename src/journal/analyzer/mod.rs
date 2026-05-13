//! Read-only services exposed to MCP clients.
//!
//! Froid is a single-user journal. Every method takes a fixed [`UserContext`]
//! from server startup, never an LLM-supplied user id. The MCP adapter is
//! restricted to loopback binds by configuration validation, and each service
//! caps requested limits to a maximum.

pub mod journal;
pub mod review;
pub mod semantic;
pub mod signal;
pub mod tools;
pub mod types;
mod validation;
pub mod wiring;

pub use semantic::{DefaultSemanticJournalSearcher, SemanticJournalSearcher};
pub use types::UserContext;
pub use wiring::{AnalyzerMcpComponents, build_analyzer_mcp_components};
