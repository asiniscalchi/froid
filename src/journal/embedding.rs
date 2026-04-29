mod backfill;
mod config;
mod provider;
mod repository;
mod types;
mod worker_config;

pub use backfill::{BackfillResult, EmbeddingBackfillError, EmbeddingBackfillService};
pub use config::{EmbeddingConfig, EmbeddingConfigError};
pub(crate) use provider::EmbeddingProvider;
pub use provider::{ProviderError, RigOpenAiEmbedder, RigOpenAiEmbedderError};
pub use repository::{
    EmbeddingIndex, EmbeddingRepositoryError, EmbeddingSearchResult,
    JournalEntryEmbeddingCandidate, SqliteEmbeddingRepository,
};
pub use types::{Embedder, EmbedderError, Embedding};
pub use worker_config::{EmbeddingWorkerConfig, EmbeddingWorkerConfigError};

pub const DEFAULT_EMBEDDING_MODEL: &str = "text-embedding-3-small";
pub const SUPPORTED_EMBEDDING_DIMENSIONS: usize = 1536;

#[cfg(test)]
mod tests;
