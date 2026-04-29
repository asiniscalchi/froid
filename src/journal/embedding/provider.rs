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
    #[allow(dead_code)]
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

#[cfg(test)]
mod tests {
    use async_trait::async_trait;

    use super::*;
    use crate::journal::embedding::{DEFAULT_EMBEDDING_MODEL, SUPPORTED_EMBEDDING_DIMENSIONS};

    #[derive(Debug, Clone)]
    struct FakeProvider {
        result: Result<Vec<f32>, ProviderError>,
    }

    #[async_trait]
    impl EmbeddingProvider for FakeProvider {
        async fn embed(&self, _model: &str, _text: &str) -> Result<Vec<f32>, ProviderError> {
            self.result.clone()
        }
    }

    #[test]
    fn real_openai_embedder_requires_api_key() {
        let result = RigOpenAiEmbedder::from_optional_api_key(EmbeddingConfig::default(), None);

        assert!(matches!(
            result,
            Err(RigOpenAiEmbedderError::MissingOpenAiApiKey)
        ));
    }

    #[tokio::test]
    async fn accepts_provider_vector_with_configured_dimensions() {
        let embedder = RigOpenAiEmbedder::new(
            EmbeddingConfig::default(),
            FakeProvider {
                result: Ok(vec![1.0; SUPPORTED_EMBEDDING_DIMENSIONS]),
            },
        );

        let embedding = embedder.embed("hello").await.unwrap();

        assert_eq!(embedder.model(), DEFAULT_EMBEDDING_MODEL);
        assert_eq!(embedder.dimensions(), SUPPORTED_EMBEDDING_DIMENSIONS);
        assert_eq!(embedding.values().len(), SUPPORTED_EMBEDDING_DIMENSIONS);
    }

    #[tokio::test]
    async fn rejects_provider_vector_with_wrong_dimensions() {
        let embedder = RigOpenAiEmbedder::new(
            EmbeddingConfig::default(),
            FakeProvider {
                result: Ok(vec![1.0; SUPPORTED_EMBEDDING_DIMENSIONS - 1]),
            },
        );

        let error = embedder.embed("hello").await.unwrap_err();

        assert_eq!(
            error,
            EmbedderError::InvalidDimension {
                expected: SUPPORTED_EMBEDDING_DIMENSIONS,
                actual: SUPPORTED_EMBEDDING_DIMENSIONS - 1,
            }
        );
    }

    #[tokio::test]
    async fn maps_provider_errors() {
        let embedder = RigOpenAiEmbedder::new(
            EmbeddingConfig::default(),
            FakeProvider {
                result: Err(ProviderError::Request("provider down".to_string())),
            },
        );

        let error = embedder.embed("hello").await.unwrap_err();

        assert_eq!(error, EmbedderError::Provider("provider down".to_string()));
    }
}
