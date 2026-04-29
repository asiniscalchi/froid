mod backfill;
mod config;
mod provider;
mod repository;
mod types;
mod worker_config;

pub use backfill::{BackfillResult, EmbeddingBackfillError, EmbeddingBackfillService};
#[allow(unused_imports)]
pub use config::{EmbeddingConfig, EmbeddingConfigError};
#[allow(unused_imports)]
pub(crate) use provider::EmbeddingProvider;
#[allow(unused_imports)]
pub use provider::{ProviderError, RigOpenAiEmbedder, RigOpenAiEmbedderError};
#[allow(unused_imports)]
pub use repository::{
    EmbeddingIndex, EmbeddingRepositoryError, EmbeddingSearchResult,
    JournalEntryEmbeddingCandidate, SqliteEmbeddingRepository,
};
pub use types::{Embedder, EmbedderError, Embedding};
#[allow(unused_imports)]
pub use worker_config::{EmbeddingWorkerConfig, EmbeddingWorkerConfigError};

pub const DEFAULT_EMBEDDING_MODEL: &str = "text-embedding-3-small";
pub const SUPPORTED_EMBEDDING_DIMENSIONS: usize = 1536;

#[cfg(test)]
mod tests;
