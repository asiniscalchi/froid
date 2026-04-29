use std::{error::Error, fmt};

use tracing::warn;

use super::{Embedder, EmbeddingIndex, EmbeddingRepositoryError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackfillResult {
    pub attempted: u32,
    pub created: u32,
    pub failed: u32,
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
pub struct EmbeddingBackfillService<I, E> {
    index: I,
    embedder: E,
}

impl<I, E> EmbeddingBackfillService<I, E>
where
    I: EmbeddingIndex,
    E: Embedder,
{
    pub fn new(index: I, embedder: E) -> Self {
        Self { index, embedder }
    }

    pub fn model(&self) -> &str {
        self.embedder.model()
    }

    pub fn dimensions(&self) -> usize {
        self.embedder.dimensions()
    }

    pub async fn backfill_missing_embeddings(
        &self,
        limit: u32,
    ) -> Result<BackfillResult, EmbeddingBackfillError> {
        let embedding_model = self.embedder.model();
        let embedding_dim = self.embedder.dimensions();

        let candidates = self
            .index
            .find_entries_missing_embedding(embedding_model, limit)
            .await
            .map_err(EmbeddingBackfillError::Repository)?;

        let mut result = BackfillResult {
            attempted: candidates.len() as u32,
            created: 0,
            failed: 0,
        };

        for candidate in candidates {
            let embedding = match self.embedder.embed(&candidate.raw_text).await {
                Ok(embedding) => embedding,
                Err(error) => {
                    result.failed += 1;
                    warn!(
                        journal_entry_id = candidate.journal_entry_id,
                        embedding_model,
                        embedding_dim,
                        error = %error,
                        "failed to generate journal entry embedding"
                    );
                    continue;
                }
            };

            match self
                .index
                .store_embedding(
                    candidate.journal_entry_id,
                    embedding_model,
                    embedding_dim,
                    &embedding,
                )
                .await
            {
                Ok(true) => result.created += 1,
                Ok(false) => {}
                Err(error) => {
                    result.failed += 1;
                    warn!(
                        journal_entry_id = candidate.journal_entry_id,
                        embedding_model,
                        embedding_dim,
                        error = %error,
                        "failed to store journal entry embedding"
                    );
                }
            }
        }

        Ok(result)
    }
}
