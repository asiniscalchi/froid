use std::sync::Arc;

use async_trait::async_trait;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq)]
pub struct Embedding(Vec<f32>);

impl Embedding {
    pub fn new(values: Vec<f32>, expected_dimensions: usize) -> Result<Self, EmbedderError> {
        if values.len() != expected_dimensions {
            return Err(EmbedderError::InvalidDimension {
                expected: expected_dimensions,
                actual: values.len(),
            });
        }
        Ok(Self(values))
    }

    pub fn values(&self) -> &[f32] {
        &self.0
    }

    pub fn to_blob(&self) -> Vec<u8> {
        let mut blob = Vec::with_capacity(self.0.len() * 4);
        for value in &self.0 {
            blob.extend_from_slice(&value.to_le_bytes());
        }
        blob
    }

    pub fn from_blob(blob: &[u8]) -> Self {
        let values = blob
            .as_chunks::<4>()
            .0
            .iter()
            .map(|chunk| f32::from_le_bytes(*chunk))
            .collect();
        Self(values)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingCandidate<ID> {
    pub id: ID,
    pub raw_text: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EmbeddingSearchResult<ID> {
    pub id: ID,
    pub distance: f32,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum EmbedderError {
    #[error("embedding dimension mismatch: expected {expected}, got {actual}")]
    InvalidDimension { expected: usize, actual: usize },
    #[error("embedding provider failed: {0}")]
    Provider(String),
}

#[async_trait]
pub trait Embedder: Send + Sync {
    fn model(&self) -> &str;

    fn dimensions(&self) -> usize;

    async fn embed(&self, text: &str) -> Result<Embedding, EmbedderError>;
}

#[async_trait]
impl<E> Embedder for Arc<E>
where
    E: Embedder + ?Sized,
{
    fn model(&self) -> &str {
        (**self).model()
    }

    fn dimensions(&self) -> usize {
        (**self).dimensions()
    }

    async fn embed(&self, text: &str) -> Result<Embedding, EmbedderError> {
        (**self).embed(text).await
    }
}
