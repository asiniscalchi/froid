use std::{error::Error, fmt};

use tracing::warn;

use super::{Embedder, EmbeddingIndex, EmbeddingRepositoryError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackfillResult {
    pub attempted: u32,
    pub created: u32,
    pub failed: u32,
    pub remaining: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmbeddingBackfillError {
    Repository(EmbeddingRepositoryError),
}

impl fmt::Display for EmbeddingBackfillError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Repository(error) => write!(f, "{error}"),
        }
    }
}

impl Error for EmbeddingBackfillError {}

#[derive(Debug, Clone)]
pub struct EmbeddingBackfillService<ID, I, E> {
    index: I,
    embedder: E,
    _phantom: std::marker::PhantomData<ID>,
}

impl<ID, I, E> EmbeddingBackfillService<ID, I, E>
where
    I: EmbeddingIndex<ID>,
    E: Embedder,
    ID: Send + Sync + Copy,
{
    pub fn new(index: I, embedder: E) -> Self {
        Self {
            index,
            embedder,
            _phantom: std::marker::PhantomData,
        }
    }

    pub fn model(&self) -> &str {
        self.embedder.model()
    }

    pub fn dimensions(&self) -> usize {
        self.embedder.dimensions()
    }

    pub async fn backfill_missing_or_failed_embeddings(
        &self,
        limit: u32,
    ) -> Result<BackfillResult, EmbeddingBackfillError> {
        let embedding_model = self.embedder.model();
        let embedding_dim = self.embedder.dimensions();

        let candidates = self
            .index
            .find_entries_missing_or_failed_embedding(embedding_model, limit)
            .await
            .map_err(EmbeddingBackfillError::Repository)?;

        let mut result = BackfillResult {
            attempted: candidates.len() as u32,
            created: 0,
            failed: 0,
            remaining: 0,
        };

        for candidate in candidates {
            if let Err(error) = self
                .index
                .delete_failed_embedding(candidate.id, embedding_model)
                .await
            {
                result.failed += 1;
                warn!(
                    // journal_entry_id = candidate.id, // ID might not be printable directly without traits, but i64/etc usually are.
                    error = %error,
                    "failed to delete previous failed embedding record"
                );
                continue;
            }

            let embedding = match self.embedder.embed(&candidate.raw_text).await {
                Ok(embedding) => embedding,
                Err(error) => {
                    result.failed += 1;
                    warn!(
                        // journal_entry_id = candidate.id,
                        embedding_model,
                        embedding_dim,
                        error = %error,
                        "failed to generate journal entry embedding"
                    );
                    self.record_failure(candidate.id, embedding_model, &error.to_string())
                        .await;
                    continue;
                }
            };

            match self
                .index
                .store_embedding(candidate.id, embedding_model, embedding_dim, &embedding)
                .await
            {
                Ok(true) => result.created += 1,
                Ok(false) => {}
                Err(error) => {
                    result.failed += 1;
                    warn!(
                        // journal_entry_id = candidate.id,
                        embedding_model,
                        embedding_dim,
                        error = %error,
                        "failed to store journal entry embedding"
                    );
                    self.record_failure(candidate.id, embedding_model, &error.to_string())
                        .await;
                }
            }
        }

        result.remaining = self
            .index
            .count_entries_missing_or_failed_embedding(embedding_model)
            .await
            .map_err(EmbeddingBackfillError::Repository)?;

        Ok(result)
    }

    async fn record_failure(&self, id: ID, embedding_model: &str, error_message: &str) {
        if let Err(db_error) = self
            .index
            .record_embedding_failure(id, embedding_model, error_message)
            .await
        {
            warn!(
                // id,
                error = %db_error,
                "failed to record embedding failure"
            );
        }
    }
}
