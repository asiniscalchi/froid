use std::{env, error::Error, fmt, sync::Arc};

use async_trait::async_trait;
use rig::{
    client::CompletionClient,
    providers::openai::{Client as OpenAiClient, completion::GPT_5_MINI},
};

use crate::journal::extraction::{JournalEntryExtractionPrompt, JournalEntryExtractionResult};

pub const DEFAULT_JOURNAL_ENTRY_EXTRACTION_MODEL: &str = GPT_5_MINI;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalEntryExtractionConfig {
    pub model: String,
}

impl Default for JournalEntryExtractionConfig {
    fn default() -> Self {
        Self {
            model: DEFAULT_JOURNAL_ENTRY_EXTRACTION_MODEL.to_string(),
        }
    }
}

impl JournalEntryExtractionConfig {
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

#[derive(Debug, Clone, PartialEq)]
pub struct JournalEntryExtractionGenerationError {
    message: String,
}

impl JournalEntryExtractionGenerationError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for JournalEntryExtractionGenerationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl Error for JournalEntryExtractionGenerationError {}

#[async_trait]
pub trait JournalEntryExtractionGenerator: Send + Sync {
    fn model(&self) -> &str;
    fn prompt_version(&self) -> &str;

    async fn generate_entry_extraction(
        &self,
        note: &str,
    ) -> Result<JournalEntryExtractionResult, JournalEntryExtractionGenerationError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RigOpenAiJournalEntryExtractionGeneratorError {
    MissingOpenAiApiKey,
    Client(String),
}

impl fmt::Display for RigOpenAiJournalEntryExtractionGeneratorError {
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

impl Error for RigOpenAiJournalEntryExtractionGeneratorError {}

#[derive(Debug, Clone, PartialEq)]
pub enum JournalEntryExtractionProviderError {
    Request(String),
}

impl fmt::Display for JournalEntryExtractionProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Request(message) => write!(f, "{message}"),
        }
    }
}

impl Error for JournalEntryExtractionProviderError {}

#[async_trait]
pub(crate) trait JournalEntryExtractionProvider: Send + Sync {
    async fn complete_entry_extraction(
        &self,
        model: &str,
        instructions: &str,
        prompt: &str,
    ) -> Result<JournalEntryExtractionResult, JournalEntryExtractionProviderError>;
}

#[derive(Clone)]
struct RigOpenAiJournalEntryExtractionProvider {
    client: OpenAiClient,
}

impl RigOpenAiJournalEntryExtractionProvider {
    fn new(api_key: &str) -> Result<Self, RigOpenAiJournalEntryExtractionGeneratorError> {
        let client = OpenAiClient::new(api_key).map_err(|error| {
            RigOpenAiJournalEntryExtractionGeneratorError::Client(error.to_string())
        })?;
        Ok(Self { client })
    }
}

#[async_trait]
impl JournalEntryExtractionProvider for RigOpenAiJournalEntryExtractionProvider {
    async fn complete_entry_extraction(
        &self,
        model: &str,
        instructions: &str,
        prompt: &str,
    ) -> Result<JournalEntryExtractionResult, JournalEntryExtractionProviderError> {
        use rig::completion::TypedPrompt;

        let agent = self.client.agent(model).preamble(instructions).build();

        agent
            .prompt_typed::<JournalEntryExtractionResult>(prompt)
            .await
            .map_err(|error| JournalEntryExtractionProviderError::Request(error.to_string()))
    }
}

#[derive(Clone)]
pub struct RigOpenAiJournalEntryExtractionGenerator {
    config: JournalEntryExtractionConfig,
    prompt: JournalEntryExtractionPrompt,
    provider: Arc<dyn JournalEntryExtractionProvider>,
}

impl RigOpenAiJournalEntryExtractionGenerator {
    pub fn from_optional_api_key(
        config: JournalEntryExtractionConfig,
        prompt: JournalEntryExtractionPrompt,
        api_key: Option<String>,
    ) -> Result<Self, RigOpenAiJournalEntryExtractionGeneratorError> {
        let api_key = api_key
            .filter(|value| !value.trim().is_empty())
            .ok_or(RigOpenAiJournalEntryExtractionGeneratorError::MissingOpenAiApiKey)?;
        let provider = RigOpenAiJournalEntryExtractionProvider::new(&api_key)?;

        Ok(Self {
            config,
            prompt,
            provider: Arc::new(provider),
        })
    }

    #[cfg(test)]
    pub(crate) fn new<P>(
        config: JournalEntryExtractionConfig,
        prompt: JournalEntryExtractionPrompt,
        provider: P,
    ) -> Self
    where
        P: JournalEntryExtractionProvider + 'static,
    {
        Self {
            config,
            prompt,
            provider: Arc::new(provider),
        }
    }
}

#[async_trait]
impl JournalEntryExtractionGenerator for RigOpenAiJournalEntryExtractionGenerator {
    fn model(&self) -> &str {
        &self.config.model
    }

    fn prompt_version(&self) -> &str {
        &self.prompt.version
    }

    async fn generate_entry_extraction(
        &self,
        note: &str,
    ) -> Result<JournalEntryExtractionResult, JournalEntryExtractionGenerationError> {
        let prompt = build_entry_extraction_prompt(note);
        self.provider
            .complete_entry_extraction(&self.config.model, &self.prompt.text, &prompt)
            .await
            .map_err(|error| JournalEntryExtractionGenerationError::new(error.to_string()))
    }
}

fn build_entry_extraction_prompt(note: &str) -> String {
    format!(
        r#"Journal note:
{note}"#
    )
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;

    use super::*;

    #[derive(Clone)]
    struct FakeProvider {
        result: Result<JournalEntryExtractionResult, JournalEntryExtractionProviderError>,
        calls: Arc<Mutex<Vec<(String, String, String)>>>,
    }

    impl FakeProvider {
        fn succeeding(response: JournalEntryExtractionResult) -> Self {
            Self {
                result: Ok(response),
                calls: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn calls(&self) -> Vec<(String, String, String)> {
            self.calls.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl JournalEntryExtractionProvider for FakeProvider {
        async fn complete_entry_extraction(
            &self,
            model: &str,
            instructions: &str,
            prompt: &str,
        ) -> Result<JournalEntryExtractionResult, JournalEntryExtractionProviderError> {
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
        let expected_result = JournalEntryExtractionResult {
            summary: "Saved".to_string(),
            domains: vec![],
            emotions: vec![],
            behaviors: vec![],
            needs: vec![],
            possible_patterns: vec![],
        };
        let provider = FakeProvider::succeeding(expected_result.clone());
        let generator = RigOpenAiJournalEntryExtractionGenerator::new(
            JournalEntryExtractionConfig {
                model: "custom-model".to_string(),
            },
            JournalEntryExtractionPrompt {
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

        assert_eq!(output, expected_result);
        assert_eq!(generator.model(), "custom-model");
        assert_eq!(generator.prompt_version(), "entry_extraction_test");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "custom-model");
        assert_eq!(calls[0].1, "System instructions");
        assert!(calls[0].2.contains("I felt uncertain after work."));
    }

    #[tokio::test]
    async fn generator_maps_provider_failure() {
        let generator = RigOpenAiJournalEntryExtractionGenerator::new(
            JournalEntryExtractionConfig::default(),
            JournalEntryExtractionPrompt {
                version: "entry_extraction_test".to_string(),
                text: "System instructions".to_string(),
            },
            FakeProvider {
                result: Err(JournalEntryExtractionProviderError::Request(
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
        let config = JournalEntryExtractionConfig::from_values(None);

        assert_eq!(config.model, DEFAULT_JOURNAL_ENTRY_EXTRACTION_MODEL);
    }

    #[test]
    fn extraction_config_accepts_model_override() {
        let config = JournalEntryExtractionConfig::from_values(Some("custom-model".to_string()));

        assert_eq!(config.model, "custom-model");
    }

    #[test]
    fn real_openai_generator_requires_api_key() {
        let result = RigOpenAiJournalEntryExtractionGenerator::from_optional_api_key(
            JournalEntryExtractionConfig::default(),
            JournalEntryExtractionPrompt {
                version: "entry_extraction_test".to_string(),
                text: "System instructions".to_string(),
            },
            None,
        );

        assert_eq!(
            result.err(),
            Some(RigOpenAiJournalEntryExtractionGeneratorError::MissingOpenAiApiKey)
        );
    }
}
