use super::entry::JournalStats;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusReport {
    pub journal: JournalStats,
    pub embeddings: EmbeddingStatus,
    pub daily_review: DailyReviewStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingStatus {
    pub semantic_search: SemanticSearchStatus,
    pub config: Option<EmbeddingStatusConfig>,
    pub pending_embeddings: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingStatusConfig {
    pub model: String,
    pub dimensions: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SemanticSearchStatus {
    Enabled,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DailyReviewStatus {
    pub generation: DailyReviewGenerationStatus,
    pub prompt_version: Option<String>,
    pub delivery: DailyReviewDeliveryStatus,
    pub date_mode: DailyReviewDateMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DailyReviewGenerationStatus {
    Configured,
    NotConfigured,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DailyReviewDeliveryStatus {
    NotImplemented,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DailyReviewDateMode {
    Utc,
}
