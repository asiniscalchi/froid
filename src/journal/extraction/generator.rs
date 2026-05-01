use std::{env, error::Error, fmt, sync::Arc};

use async_trait::async_trait;
use rig::{
    client::CompletionClient,
    completion::Prompt,
    providers::openai::{Client as OpenAiClient, completion::GPT_5_MINI},
};

use crate::journal::extraction::EntryExtractionPrompt;

pub const DEFAULT_ENTRY_EXTRACTION_MODEL: &str = GPT_5_MINI;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryExtractionConfig {
    pub model: String,
}

impl Default for EntryExtractionConfig {
    fn default() -> Self {
        Self {
            model: DEFAULT_ENTRY_EXTRACTION_MODEL.to_string(),
        }
    }
}

impl EntryExtractionConfig {
    pub fn from_env() -> Self {
        Self::from_values(env::var("FROID_ENTRY_EXTRACTION_MODEL").ok())
    }

    pub(crate) fn from_values(model: Option<String>) -> Self {
        let defaults = Self::default();
        Self {
            model: model
                .filter(|value| !value.trim().is_empty())
                .unwrap_or(defaults.model),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryExtractionGenerationError {
    message: String,
}

impl EntryExtractionGenerationError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for EntryExtractionGenerationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl Error for EntryExtractionGenerationError {}

#[async_trait]
pub trait EntryExtractionGenerator: Send + Sync {
    fn model(&self) -> &str;
    fn prompt_version(&self) -> &str;

    async fn generate_entry_extraction(
        &self,
        note: &str,
    ) -> Result<String, EntryExtractionGenerationError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RigOpenAiEntryExtractionGeneratorError {
    MissingOpenAiApiKey,
    Client(String),
}

impl fmt::Display for RigOpenAiEntryExtractionGeneratorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingOpenAiApiKey => write!(f, "OPENAI_API_KEY is required"),
            Self::Client(message) => write!(
                f,
                "failed to construct OpenAI journal entry extraction generator: {message}"
            ),
        }
    }
}

impl Error for RigOpenAiEntryExtractionGeneratorError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryExtractionProviderError {
    Request(String),
}

impl fmt::Display for EntryExtractionProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Request(message) => write!(f, "{message}"),
        }
    }
}

impl Error for EntryExtractionProviderError {}

#[async_trait]
pub(crate) trait EntryExtractionProvider: Send + Sync {
    async fn complete_entry_extraction(
        &self,
        model: &str,
        instructions: &str,
        prompt: &str,
    ) -> Result<String, EntryExtractionProviderError>;
}

#[derive(Clone)]
struct RigOpenAiEntryExtractionProvider {
    client: OpenAiClient,
}

impl RigOpenAiEntryExtractionProvider {
    fn new(api_key: &str) -> Result<Self, RigOpenAiEntryExtractionGeneratorError> {
        let client = OpenAiClient::new(api_key)
            .map_err(|error| RigOpenAiEntryExtractionGeneratorError::Client(error.to_string()))?;
        Ok(Self { client })
    }
}

#[async_trait]
impl EntryExtractionProvider for RigOpenAiEntryExtractionProvider {
    async fn complete_entry_extraction(
        &self,
        model: &str,
        instructions: &str,
        prompt: &str,
    ) -> Result<String, EntryExtractionProviderError> {
        let agent = self.client.agent(model).preamble(instructions).build();

        agent
            .prompt(prompt)
            .await
            .map_err(|error| EntryExtractionProviderError::Request(error.to_string()))
    }
}

#[derive(Clone)]
pub struct RigOpenAiEntryExtractionGenerator {
    config: EntryExtractionConfig,
    prompt: EntryExtractionPrompt,
    provider: Arc<dyn EntryExtractionProvider>,
}

impl RigOpenAiEntryExtractionGenerator {
    pub fn from_optional_api_key(
        config: EntryExtractionConfig,
        prompt: EntryExtractionPrompt,
        api_key: Option<String>,
    ) -> Result<Self, RigOpenAiEntryExtractionGeneratorError> {
        let api_key = api_key
            .filter(|value| !value.trim().is_empty())
            .ok_or(RigOpenAiEntryExtractionGeneratorError::MissingOpenAiApiKey)?;
        let provider = RigOpenAiEntryExtractionProvider::new(&api_key)?;

        Ok(Self {
            config,
            prompt,
            provider: Arc::new(provider),
        })
    }

    #[cfg(test)]
    pub(crate) fn new<P>(
        config: EntryExtractionConfig,
        prompt: EntryExtractionPrompt,
        provider: P,
    ) -> Self
    where
        P: EntryExtractionProvider + 'static,
    {
        Self {
            config,
            prompt,
            provider: Arc::new(provider),
        }
    }
}

#[async_trait]
impl EntryExtractionGenerator for RigOpenAiEntryExtractionGenerator {
    fn model(&self) -> &str {
        &self.config.model
    }

    fn prompt_version(&self) -> &str {
        &self.prompt.version
    }

    async fn generate_entry_extraction(
        &self,
        note: &str,
    ) -> Result<String, EntryExtractionGenerationError> {
        let prompt = build_entry_extraction_prompt(note);
        self.provider
            .complete_entry_extraction(&self.config.model, &self.prompt.text, &prompt)
            .await
            .map_err(|error| EntryExtractionGenerationError::new(error.to_string()))
    }
}

fn build_entry_extraction_prompt(note: &str) -> String {
    format!(
        r#"Journal note:
{note}

Return the structured extraction as JSON only."#
    )
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;

    use super::*;

    #[derive(Clone)]
    struct FakeProvider {
        result: Result<String, EntryExtractionProviderError>,
        calls: Arc<Mutex<Vec<(String, String, String)>>>,
    }

    impl FakeProvider {
        fn succeeding(response: &str) -> Self {
            Self {
                result: Ok(response.to_string()),
                calls: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn calls(&self) -> Vec<(String, String, String)> {
            self.calls.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl EntryExtractionProvider for FakeProvider {
        async fn complete_entry_extraction(
            &self,
            model: &str,
            instructions: &str,
            prompt: &str,
        ) -> Result<String, EntryExtractionProviderError> {
            self.calls.lock().unwrap().push((
                model.to_string(),
                instructions.to_string(),
                prompt.to_string(),
            ));
            self.result.clone()
        }
    }

    #[tokio::test]
    async fn generator_uses_configured_model_prompt_version_and_note() {
        let provider = FakeProvider::succeeding(
            r#"{"summary":"Saved","domains":[],"emotions":[],"behaviors":[],"needs":[],"possible_patterns":[]}"#,
        );
        let generator = RigOpenAiEntryExtractionGenerator::new(
            EntryExtractionConfig {
                model: "custom-model".to_string(),
            },
            EntryExtractionPrompt {
                version: "entry_extraction_test".to_string(),
                text: "System instructions".to_string(),
            },
            provider.clone(),
        );

        let output = generator
            .generate_entry_extraction("I felt uncertain after work.")
            .await
            .unwrap();
        let calls = provider.calls();

        assert!(output.contains("\"summary\""));
        assert_eq!(generator.model(), "custom-model");
        assert_eq!(generator.prompt_version(), "entry_extraction_test");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "custom-model");
        assert_eq!(calls[0].1, "System instructions");
        assert!(calls[0].2.contains("I felt uncertain after work."));
    }

    #[tokio::test]
    async fn generator_maps_provider_failure() {
        let generator = RigOpenAiEntryExtractionGenerator::new(
            EntryExtractionConfig::default(),
            EntryExtractionPrompt {
                version: "entry_extraction_test".to_string(),
                text: "System instructions".to_string(),
            },
            FakeProvider {
                result: Err(EntryExtractionProviderError::Request(
                    "provider down".to_string(),
                )),
                calls: Arc::new(Mutex::new(Vec::new())),
            },
        );

        let error = generator
            .generate_entry_extraction("A note")
            .await
            .unwrap_err();

        assert_eq!(error.to_string(), "provider down");
    }

    #[test]
    fn extraction_config_uses_default_model() {
        let config = EntryExtractionConfig::from_values(None);

        assert_eq!(config.model, DEFAULT_ENTRY_EXTRACTION_MODEL);
    }

    #[test]
    fn extraction_config_accepts_model_override() {
        let config = EntryExtractionConfig::from_values(Some("custom-model".to_string()));

        assert_eq!(config.model, "custom-model");
    }

    #[test]
    fn real_openai_generator_requires_api_key() {
        let result = RigOpenAiEntryExtractionGenerator::from_optional_api_key(
            EntryExtractionConfig::default(),
            EntryExtractionPrompt {
                version: "entry_extraction_test".to_string(),
                text: "System instructions".to_string(),
            },
            None,
        );

        assert_eq!(
            result.err(),
            Some(RigOpenAiEntryExtractionGeneratorError::MissingOpenAiApiKey)
        );
    }
}
