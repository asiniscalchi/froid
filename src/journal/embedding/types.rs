use std::{error::Error, fmt};

use async_trait::async_trait;

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmbedderError {
    InvalidDimension { expected: usize, actual: usize },
    Provider(String),
}

impl fmt::Display for EmbedderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDimension { expected, actual } => {
                write!(
                    f,
                    "embedding dimension mismatch: expected {expected}, got {actual}"
                )
            }
            Self::Provider(message) => write!(f, "embedding provider failed: {message}"),
        }
    }
}

impl Error for EmbedderError {}

#[async_trait]
pub trait Embedder: Send + Sync {
    fn model(&self) -> &str;

    fn dimensions(&self) -> usize;

    async fn embed(&self, text: &str) -> Result<Embedding, EmbedderError>;
}
