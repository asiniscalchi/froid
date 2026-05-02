use std::{env, error::Error, fmt, sync::Arc};

use async_trait::async_trait;
use rig::{
    client::CompletionClient,
    providers::openai::{Client as OpenAiClient, completion::GPT_5_MINI},
};

use crate::journal::review::{JournalEntryWithExtraction, signals::types::DailyReviewSignalsOutput};

use super::prompt::DailyReviewSignalPrompt;

pub const DEFAULT_SIGNAL_EXTRACTION_MODEL: &str = GPT_5_MINI;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DailyReviewSignalConfig {
    pub model: String,
}

impl Default for DailyReviewSignalConfig {
    fn default() -> Self {
        Self {
            model: DEFAULT_SIGNAL_EXTRACTION_MODEL.to_string(),
        }
    }
}

impl DailyReviewSignalConfig {
    pub fn from_env() -> Self {
        Self::from_values(env::var("FROID_SIGNAL_EXTRACTION_MODEL").ok())
    }

    pub(crate) fn from_values(model: Option<String>) -> Self {
        let defaults = Self::default();
        Self {
            model: model
                .filter(|v| !v.trim().is_empty())
                .unwrap_or(defaults.model),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DailyReviewSignalGenerationError {
    message: String,
}

impl DailyReviewSignalGenerationError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for DailyReviewSignalGenerationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl Error for DailyReviewSignalGenerationError {}

#[async_trait]
pub trait DailyReviewSignalGenerator: Send + Sync {
    fn model(&self) -> &str;
    fn prompt_version(&self) -> &str;

    async fn generate_signals(
        &self,
        review_text: &str,
        entries: &[JournalEntryWithExtraction],
    ) -> Result<DailyReviewSignalsOutput, DailyReviewSignalGenerationError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RigOpenAiDailyReviewSignalGeneratorError {
    MissingOpenAiApiKey,
    Client(String),
}

impl fmt::Display for RigOpenAiDailyReviewSignalGeneratorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingOpenAiApiKey => write!(f, "OPENAI_API_KEY is required"),
            Self::Client(message) => write!(
                f,
                "failed to construct OpenAI signal extraction generator: {message}"
            ),
        }
    }
}

impl Error for RigOpenAiDailyReviewSignalGeneratorError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignalProviderError {
    Request(String),
}

impl fmt::Display for SignalProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Request(message) => write!(f, "{message}"),
        }
    }
}

impl Error for SignalProviderError {}

#[async_trait]
pub(crate) trait SignalProvider: Send + Sync {
    async fn complete_signal_extraction(
        &self,
        model: &str,
        instructions: &str,
        prompt: &str,
    ) -> Result<DailyReviewSignalsOutput, SignalProviderError>;
}

#[derive(Clone)]
struct RigOpenAiSignalProvider {
    client: OpenAiClient,
}

impl RigOpenAiSignalProvider {
    fn new(api_key: &str) -> Result<Self, RigOpenAiDailyReviewSignalGeneratorError> {
        let client = OpenAiClient::new(api_key).map_err(|error| {
            RigOpenAiDailyReviewSignalGeneratorError::Client(error.to_string())
        })?;
        Ok(Self { client })
    }
}

#[async_trait]
impl SignalProvider for RigOpenAiSignalProvider {
    async fn complete_signal_extraction(
        &self,
        model: &str,
        instructions: &str,
        prompt: &str,
    ) -> Result<DailyReviewSignalsOutput, SignalProviderError> {
        use rig::completion::TypedPrompt;

        let agent = self.client.agent(model).preamble(instructions).build();

        agent
            .prompt_typed::<DailyReviewSignalsOutput>(prompt)
            .await
            .map_err(|error| SignalProviderError::Request(error.to_string()))
    }
}

#[derive(Clone)]
pub struct RigOpenAiDailyReviewSignalGenerator {
    config: DailyReviewSignalConfig,
    prompt: DailyReviewSignalPrompt,
    provider: Arc<dyn SignalProvider>,
}

impl RigOpenAiDailyReviewSignalGenerator {
    pub fn from_optional_api_key(
        config: DailyReviewSignalConfig,
        prompt: DailyReviewSignalPrompt,
        api_key: Option<String>,
    ) -> Result<Self, RigOpenAiDailyReviewSignalGeneratorError> {
        let api_key = api_key
            .filter(|v| !v.trim().is_empty())
            .ok_or(RigOpenAiDailyReviewSignalGeneratorError::MissingOpenAiApiKey)?;
        let provider = RigOpenAiSignalProvider::new(&api_key)?;

        Ok(Self {
            config,
            prompt,
            provider: Arc::new(provider),
        })
    }

    #[cfg(test)]
    pub(crate) fn new<P>(
        config: DailyReviewSignalConfig,
        prompt: DailyReviewSignalPrompt,
        provider: P,
    ) -> Self
    where
        P: SignalProvider + 'static,
    {
        Self {
            config,
            prompt,
            provider: Arc::new(provider),
        }
    }
}

#[async_trait]
impl DailyReviewSignalGenerator for RigOpenAiDailyReviewSignalGenerator {
    fn model(&self) -> &str {
        &self.config.model
    }

    fn prompt_version(&self) -> &str {
        &self.prompt.version
    }

    async fn generate_signals(
        &self,
        review_text: &str,
        entries: &[JournalEntryWithExtraction],
    ) -> Result<DailyReviewSignalsOutput, DailyReviewSignalGenerationError> {
        let prompt = build_signal_extraction_prompt(review_text, entries);
        self.provider
            .complete_signal_extraction(&self.config.model, &self.prompt.text, &prompt)
            .await
            .map_err(|error| DailyReviewSignalGenerationError::new(error.to_string()))
    }
}

fn build_signal_extraction_prompt(
    review_text: &str,
    entries: &[JournalEntryWithExtraction],
) -> String {
    let formatted_entries = entries
        .iter()
        .map(|entry_with_ext| {
            let entry = &entry_with_ext.entry;
            let mut formatted = format!(
                "Entry #{}. [{} UTC] {}",
                entry_with_ext.id,
                entry.received_at.format("%Y-%m-%d %H:%M"),
                entry.text
            );

            if let Some(extraction) = &entry_with_ext.extraction
                && let Ok(json) = serde_json::to_string(extraction)
            {
                formatted.push_str("\n   Structured extraction: ");
                formatted.push_str(&json);
            }

            formatted
        })
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        r#"Daily review:
{review_text}

Journal entries:
{formatted_entries}"#
    )
}

/// A fake generator for use in tests.
#[cfg(test)]
pub mod fake {
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    use async_trait::async_trait;

    use super::{DailyReviewSignalGenerationError, DailyReviewSignalGenerator};
    use crate::journal::review::{
        JournalEntryWithExtraction, signals::types::DailyReviewSignalsOutput,
    };

    #[derive(Debug, Clone)]
    pub struct FakeSignalGenerator {
        model: String,
        prompt_version: String,
        result: Arc<Mutex<Result<DailyReviewSignalsOutput, DailyReviewSignalGenerationError>>>,
        calls: Arc<AtomicUsize>,
    }

    impl FakeSignalGenerator {
        pub fn succeeding(output: DailyReviewSignalsOutput) -> Self {
            Self {
                model: "fake-signal-model".to_string(),
                prompt_version: "fake-signal-prompt-v1".to_string(),
                result: Arc::new(Mutex::new(Ok(output))),
                calls: Arc::new(AtomicUsize::new(0)),
            }
        }

        pub fn failing(message: impl Into<String>) -> Self {
            Self {
                model: "fake-signal-model".to_string(),
                prompt_version: "fake-signal-prompt-v1".to_string(),
                result: Arc::new(Mutex::new(Err(DailyReviewSignalGenerationError::new(
                    message,
                )))),
                calls: Arc::new(AtomicUsize::new(0)),
            }
        }

        pub fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl DailyReviewSignalGenerator for FakeSignalGenerator {
        fn model(&self) -> &str {
            &self.model
        }

        fn prompt_version(&self) -> &str {
            &self.prompt_version
        }

        async fn generate_signals(
            &self,
            _review_text: &str,
            _entries: &[JournalEntryWithExtraction],
        ) -> Result<DailyReviewSignalsOutput, DailyReviewSignalGenerationError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.result.lock().unwrap().clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use chrono::{TimeZone, Utc};

    use super::*;
    use crate::journal::{
        entry::JournalEntry,
        review::{
            JournalEntryWithExtraction,
            signals::types::{DailyReviewSignalCandidate, DailyReviewSignalsOutput, SignalType},
        },
    };

    #[derive(Clone)]
    struct FakeProvider {
        result: Result<DailyReviewSignalsOutput, SignalProviderError>,
        calls: Arc<Mutex<Vec<(String, String, String)>>>,
    }

    impl FakeProvider {
        fn succeeding(output: DailyReviewSignalsOutput) -> Self {
            Self {
                result: Ok(output),
                calls: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn failing(message: &str) -> Self {
            Self {
                result: Err(SignalProviderError::Request(message.to_string())),
                calls: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn calls(&self) -> Vec<(String, String, String)> {
            self.calls.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl SignalProvider for FakeProvider {
        async fn complete_signal_extraction(
            &self,
            model: &str,
            instructions: &str,
            prompt: &str,
        ) -> Result<DailyReviewSignalsOutput, SignalProviderError> {
            self.calls.lock().unwrap().push((
                model.to_string(),
                instructions.to_string(),
                prompt.to_string(),
            ));
            self.result.clone()
        }
    }

    fn entry(text: &str) -> JournalEntryWithExtraction {
        JournalEntryWithExtraction {
            id: 1,
            entry: JournalEntry {
                text: text.to_string(),
                received_at: Utc.with_ymd_and_hms(2026, 4, 28, 10, 0, 0).unwrap(),
            },
            extraction: None,
        }
    }

    fn empty_output() -> DailyReviewSignalsOutput {
        DailyReviewSignalsOutput { signals: vec![] }
    }

    fn theme_signal() -> DailyReviewSignalCandidate {
        DailyReviewSignalCandidate {
            signal_type: SignalType::Theme,
            label: "physical appearance".to_string(),
            status: None,
            valence: None,
            strength: 0.8,
            confidence: 0.9,
            evidence: "Review mentions concern around training and diet.".to_string(),
        }
    }

    #[test]
    fn config_uses_default_model() {
        let config = DailyReviewSignalConfig::from_values(None);
        assert_eq!(config.model, DEFAULT_SIGNAL_EXTRACTION_MODEL);
    }

    #[test]
    fn config_accepts_model_override() {
        let config = DailyReviewSignalConfig::from_values(Some("custom-model".to_string()));
        assert_eq!(config.model, "custom-model");
    }

    #[test]
    fn generator_requires_api_key() {
        let result = RigOpenAiDailyReviewSignalGenerator::from_optional_api_key(
            DailyReviewSignalConfig::default(),
            DailyReviewSignalPrompt {
                version: "v1".to_string(),
                text: "instructions".to_string(),
            },
            None,
        );

        assert_eq!(
            result.err(),
            Some(RigOpenAiDailyReviewSignalGeneratorError::MissingOpenAiApiKey)
        );
    }

    #[tokio::test]
    async fn generator_passes_model_instructions_and_prompt_to_provider() {
        let expected = DailyReviewSignalsOutput {
            signals: vec![theme_signal()],
        };
        let provider = FakeProvider::succeeding(expected.clone());
        let generator = RigOpenAiDailyReviewSignalGenerator::new(
            DailyReviewSignalConfig {
                model: "test-model".to_string(),
            },
            DailyReviewSignalPrompt {
                version: "test-v1".to_string(),
                text: "System instructions".to_string(),
            },
            provider.clone(),
        );

        let output = generator
            .generate_signals("review text", &[entry("entry text")])
            .await
            .unwrap();
        let calls = provider.calls();

        assert_eq!(output, expected);
        assert_eq!(generator.model(), "test-model");
        assert_eq!(generator.prompt_version(), "test-v1");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "test-model");
        assert_eq!(calls[0].1, "System instructions");
        assert!(calls[0].2.contains("review text"));
        assert!(calls[0].2.contains("entry text"));
    }

    #[tokio::test]
    async fn generator_maps_provider_failure_to_generation_error() {
        let provider = FakeProvider::failing("provider down");
        let generator = RigOpenAiDailyReviewSignalGenerator::new(
            DailyReviewSignalConfig::default(),
            DailyReviewSignalPrompt {
                version: "v1".to_string(),
                text: "instructions".to_string(),
            },
            provider,
        );

        let error = generator
            .generate_signals("review text", &[])
            .await
            .unwrap_err();

        assert_eq!(error.to_string(), "provider down");
    }

    #[tokio::test]
    async fn generator_returns_empty_signals_when_provider_returns_empty() {
        let provider = FakeProvider::succeeding(empty_output());
        let generator = RigOpenAiDailyReviewSignalGenerator::new(
            DailyReviewSignalConfig::default(),
            DailyReviewSignalPrompt {
                version: "v1".to_string(),
                text: "instructions".to_string(),
            },
            provider,
        );

        let output = generator.generate_signals("review text", &[]).await.unwrap();

        assert!(output.signals.is_empty());
    }

    #[test]
    fn build_prompt_includes_review_and_entries() {
        let prompt = build_signal_extraction_prompt(
            "Today was hard.",
            &[entry("Felt anxious at work.")],
        );

        assert!(prompt.contains("Today was hard."));
        assert!(prompt.contains("Felt anxious at work."));
        assert!(prompt.contains("Daily review:"));
        assert!(prompt.contains("Journal entries:"));
    }

    #[test]
    fn build_prompt_includes_extraction_when_available() {
        use crate::journal::extraction::JournalEntryExtractionResult;

        let extraction = JournalEntryExtractionResult {
            summary: "Work stress".to_string(),
            domains: vec![],
            emotions: vec![],
            behaviors: vec![],
            needs: vec![],
            possible_patterns: vec![],
        };
        let entry_with_extraction = JournalEntryWithExtraction {
            id: 1,
            entry: JournalEntry {
                text: "Felt anxious at work.".to_string(),
                received_at: Utc.with_ymd_and_hms(2026, 4, 28, 10, 0, 0).unwrap(),
            },
            extraction: Some(extraction),
        };

        let prompt = build_signal_extraction_prompt("review text", &[entry_with_extraction]);

        assert!(prompt.contains("Structured extraction:"));
        assert!(prompt.contains("Work stress"));
    }
}
