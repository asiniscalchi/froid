use std::{env, error::Error, fmt};

use async_trait::async_trait;
use rig::{
    client::EmbeddingsClient,
    embeddings::EmbeddingModel,
    providers::openai::{self, Client as OpenAiClient},
};

use super::{Embedder, EmbedderError, Embedding, EmbeddingConfig};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RigOpenAiEmbedderError {
    MissingOpenAiApiKey,
    Client(String),
}

impl fmt::Display for RigOpenAiEmbedderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingOpenAiApiKey => write!(f, "OPENAI_API_KEY is required"),
            Self::Client(message) => write!(f, "failed to construct OpenAI embedder: {message}"),
        }
    }
}

impl Error for RigOpenAiEmbedderError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderError {
    Request(String),
}

impl fmt::Display for ProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Request(message) => write!(f, "{message}"),
        }
    }
}

impl Error for ProviderError {}

#[async_trait]
pub(crate) trait EmbeddingProvider: Send + Sync {
    async fn embed(&self, model: &str, text: &str) -> Result<Vec<f32>, ProviderError>;
}

#[derive(Clone)]
pub(crate) struct RigOpenAiProvider {
    embedding_model: openai::EmbeddingModel,
}

impl RigOpenAiProvider {
    fn new(config: &EmbeddingConfig, api_key: &str) -> Result<Self, RigOpenAiEmbedderError> {
        let client = OpenAiClient::new(api_key)
            .map_err(|error| RigOpenAiEmbedderError::Client(error.to_string()))?;
        let embedding_model = client.embedding_model_with_ndims(&config.model, config.dimensions);

        Ok(Self { embedding_model })
    }
}

#[async_trait]
impl EmbeddingProvider for RigOpenAiProvider {
    async fn embed(&self, _model: &str, text: &str) -> Result<Vec<f32>, ProviderError> {
        let embedding = self
            .embedding_model
            .embed_text(text)
            .await
            .map_err(|error| ProviderError::Request(error.to_string()))?;

        Ok(embedding
            .vec
            .into_iter()
            .map(|value| value as f32)
            .collect())
    }
}

#[derive(Clone)]
pub struct RigOpenAiEmbedder<P = RigOpenAiProvider> {
    config: EmbeddingConfig,
    provider: P,
}

impl RigOpenAiEmbedder<RigOpenAiProvider> {
    pub fn from_env(config: EmbeddingConfig) -> Result<Self, RigOpenAiEmbedderError> {
        Self::from_optional_api_key(config, env::var("OPENAI_API_KEY").ok())
    }

    pub(crate) fn from_optional_api_key(
        config: EmbeddingConfig,
        api_key: Option<String>,
    ) -> Result<Self, RigOpenAiEmbedderError> {
        let api_key = api_key
            .filter(|value| !value.trim().is_empty())
            .ok_or(RigOpenAiEmbedderError::MissingOpenAiApiKey)?;
        let provider = RigOpenAiProvider::new(&config, &api_key)?;

        Ok(Self { config, provider })
    }
}

impl<P> RigOpenAiEmbedder<P>
where
    P: EmbeddingProvider,
{
    pub(crate) fn new(config: EmbeddingConfig, provider: P) -> Self {
        Self { config, provider }
    }
}

#[async_trait]
impl<P> Embedder for RigOpenAiEmbedder<P>
where
    P: EmbeddingProvider,
{
    fn model(&self) -> &str {
        &self.config.model
    }

    fn dimensions(&self) -> usize {
        self.config.dimensions
    }

    async fn embed(&self, text: &str) -> Result<Embedding, EmbedderError> {
        let values = self
            .provider
            .embed(&self.config.model, text)
            .await
            .map_err(|error| EmbedderError::Provider(error.to_string()))?;

        Embedding::new(values, self.config.dimensions)
    }
}
