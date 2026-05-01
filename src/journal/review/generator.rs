use std::{env, error::Error, fmt, sync::Arc};

use async_trait::async_trait;
use rig::{
    client::CompletionClient,
    completion::Prompt,
    providers::openai::{Client as OpenAiClient, completion::GPT_5_MINI},
};

use crate::journal::{
    entry::JournalEntry,
    review::{DailyReviewPrompt, DailyReviewPromptError, JournalEntryWithExtraction},
};

pub const DEFAULT_REVIEW_MODEL: &str = GPT_5_MINI;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewConfig {
    pub model: String,
}

impl Default for ReviewConfig {
    fn default() -> Self {
        Self {
            model: DEFAULT_REVIEW_MODEL.to_string(),
        }
    }
}

impl ReviewConfig {
    pub fn from_env() -> Self {
        Self::from_values(env::var("FROID_REVIEW_MODEL").ok())
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
pub struct ReviewGenerationError {
    message: String,
}

impl ReviewGenerationError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for ReviewGenerationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl Error for ReviewGenerationError {}

#[async_trait]
pub trait ReviewGenerator: Send + Sync {
    fn model(&self) -> &str;
    fn prompt_version(&self) -> &str;

    async fn generate_daily_review(
        &self,
        entries: &[JournalEntryWithExtraction],
    ) -> Result<String, ReviewGenerationError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RigOpenAiReviewGeneratorError {
    MissingOpenAiApiKey,
    Prompt(DailyReviewPromptError),
    Client(String),
}

impl fmt::Display for RigOpenAiReviewGeneratorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingOpenAiApiKey => write!(f, "OPENAI_API_KEY is required"),
            Self::Prompt(error) => write!(f, "{error}"),
            Self::Client(message) => {
                write!(f, "failed to construct OpenAI review generator: {message}")
            }
        }
    }
}

impl Error for RigOpenAiReviewGeneratorError {}

impl From<DailyReviewPromptError> for RigOpenAiReviewGeneratorError {
    fn from(error: DailyReviewPromptError) -> Self {
        Self::Prompt(error)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewProviderError {
    Request(String),
}

impl fmt::Display for ReviewProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Request(message) => write!(f, "{message}"),
        }
    }
}

impl Error for ReviewProviderError {}

#[async_trait]
pub(crate) trait ReviewProvider: Send + Sync {
    async fn complete_daily_review(
        &self,
        model: &str,
        instructions: &str,
        prompt: &str,
    ) -> Result<String, ReviewProviderError>;
}

#[derive(Clone)]
struct RigOpenAiReviewProvider {
    client: OpenAiClient,
}

impl RigOpenAiReviewProvider {
    fn new(api_key: &str) -> Result<Self, RigOpenAiReviewGeneratorError> {
        let client = OpenAiClient::new(api_key)
            .map_err(|error| RigOpenAiReviewGeneratorError::Client(error.to_string()))?;
        Ok(Self { client })
    }
}

#[async_trait]
impl ReviewProvider for RigOpenAiReviewProvider {
    async fn complete_daily_review(
        &self,
        model: &str,
        instructions: &str,
        prompt: &str,
    ) -> Result<String, ReviewProviderError> {
        let agent = self.client.agent(model).preamble(instructions).build();

        agent
            .prompt(prompt)
            .await
            .map_err(|error| ReviewProviderError::Request(error.to_string()))
    }
}

#[derive(Clone)]
pub struct RigOpenAiReviewGenerator {
    config: ReviewConfig,
    prompt: DailyReviewPrompt,
    provider: Arc<dyn ReviewProvider>,
}

impl RigOpenAiReviewGenerator {
    pub fn from_optional_api_key(
        config: ReviewConfig,
        prompt: DailyReviewPrompt,
        api_key: Option<String>,
    ) -> Result<Self, RigOpenAiReviewGeneratorError> {
        let api_key = api_key
            .filter(|value| !value.trim().is_empty())
            .ok_or(RigOpenAiReviewGeneratorError::MissingOpenAiApiKey)?;
        let provider = RigOpenAiReviewProvider::new(&api_key)?;

        Ok(Self {
            config,
            prompt,
            provider: Arc::new(provider),
        })
    }

    #[cfg(test)]
    pub(crate) fn new<P>(config: ReviewConfig, prompt: DailyReviewPrompt, provider: P) -> Self
    where
        P: ReviewProvider + 'static,
    {
        Self {
            config,
            prompt,
            provider: Arc::new(provider),
        }
    }
}

#[async_trait]
impl ReviewGenerator for RigOpenAiReviewGenerator {
    fn model(&self) -> &str {
        &self.config.model
    }

    fn prompt_version(&self) -> &str {
        &self.prompt.version
    }

    async fn generate_daily_review(
        &self,
        entries: &[JournalEntryWithExtraction],
    ) -> Result<String, ReviewGenerationError> {
        let prompt = build_daily_review_prompt(entries);
        self.provider
            .complete_daily_review(&self.config.model, &self.prompt.text, &prompt)
            .await
            .map_err(|error| ReviewGenerationError::new(error.to_string()))
    }
}

fn build_daily_review_prompt(entries: &[JournalEntryWithExtraction]) -> String {
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

            if let Some(extraction) = &entry_with_ext.extraction {
                if let Ok(json) = serde_json::to_string(extraction) {
                    formatted.push_str("\n   Structured extraction: ");
                    formatted.push_str(&json);
                }
            }

            formatted
        })
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        r#"Write a daily review using only these journal entries.

Return this format:
Summary:
...

Themes:
- ...
- ...

Pay attention tomorrow:
- ...

Journal entries:
{formatted_entries}"#
    )
}

#[cfg(test)]
pub mod fake {
    use std::{
        collections::VecDeque,
        sync::{
            Arc, Mutex,
            atomic::{AtomicUsize, Ordering},
        },
    };

    use async_trait::async_trait;

    use super::{ReviewGenerationError, ReviewGenerator};
    use crate::journal::review::JournalEntryWithExtraction;

    #[derive(Debug, Clone)]
    pub struct FakeReviewGenerator {
        model: String,
        prompt_version: String,
        results: Arc<Mutex<VecDeque<Result<String, ReviewGenerationError>>>>,
        calls: Arc<AtomicUsize>,
        entries_seen: Arc<Mutex<Vec<Vec<JournalEntryWithExtraction>>>>,
    }

    impl FakeReviewGenerator {
        pub fn succeeding(review_text: impl Into<String>) -> Self {
            Self::new(vec![Ok(review_text.into())])
        }

        pub fn failing(error_message: impl Into<String>) -> Self {
            Self::new(vec![Err(ReviewGenerationError::new(error_message))])
        }

        pub fn new(results: Vec<Result<String, ReviewGenerationError>>) -> Self {
            Self {
                model: "fake-review-model".to_string(),
                prompt_version: "fake-prompt-v1".to_string(),
                results: Arc::new(Mutex::new(VecDeque::from(results))),
                calls: Arc::new(AtomicUsize::new(0)),
                entries_seen: Arc::new(Mutex::new(Vec::new())),
            }
        }

        pub fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }

        pub fn entries_seen(&self) -> Vec<Vec<JournalEntryWithExtraction>> {
            self.entries_seen.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl ReviewGenerator for FakeReviewGenerator {
        fn model(&self) -> &str {
            &self.model
        }

        fn prompt_version(&self) -> &str {
            &self.prompt_version
        }

        async fn generate_daily_review(
            &self,
            entries: &[JournalEntryWithExtraction],
        ) -> Result<String, ReviewGenerationError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.entries_seen.lock().unwrap().push(entries.to_vec());

            self.results
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Ok("fake daily review".to_string()))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use chrono::{TimeZone, Utc};

    use super::*;
    use crate::journal::extraction::JournalEntryExtractionResult;

    #[derive(Debug, Clone)]
    struct FakeReviewProvider {
        result: Result<String, ReviewProviderError>,
        instructions: Arc<Mutex<Vec<String>>>,
        prompts: Arc<Mutex<Vec<String>>>,
        models: Arc<Mutex<Vec<String>>>,
    }

    impl FakeReviewProvider {
        fn succeeding(review_text: &str) -> Self {
            Self {
                result: Ok(review_text.to_string()),
                instructions: Arc::new(Mutex::new(Vec::new())),
                prompts: Arc::new(Mutex::new(Vec::new())),
                models: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn failing(error_message: &str) -> Self {
            Self {
                result: Err(ReviewProviderError::Request(error_message.to_string())),
                instructions: Arc::new(Mutex::new(Vec::new())),
                prompts: Arc::new(Mutex::new(Vec::new())),
                models: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn instructions(&self) -> Vec<String> {
            self.instructions.lock().unwrap().clone()
        }

        fn prompts(&self) -> Vec<String> {
            self.prompts.lock().unwrap().clone()
        }

        fn models(&self) -> Vec<String> {
            self.models.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl ReviewProvider for FakeReviewProvider {
        async fn complete_daily_review(
            &self,
            model: &str,
            instructions: &str,
            prompt: &str,
        ) -> Result<String, ReviewProviderError> {
            self.models.lock().unwrap().push(model.to_string());
            self.instructions
                .lock()
                .unwrap()
                .push(instructions.to_string());
            self.prompts.lock().unwrap().push(prompt.to_string());
            self.result.clone()
        }
    }

    fn entry(day: u32, text: &str) -> JournalEntry {
        JournalEntry {
            text: text.to_string(),
            received_at: Utc.with_ymd_and_hms(2026, 4, day, 10, 0, 0).unwrap(),
        }
    }

    fn prompt(version: &str, text: &str) -> DailyReviewPrompt {
        DailyReviewPrompt {
            version: version.to_string(),
            text: text.to_string(),
        }
    }

    #[test]
    fn review_config_uses_defaults() {
        let config = ReviewConfig::from_values(None);

        assert_eq!(config.model, DEFAULT_REVIEW_MODEL);
    }

    #[test]
    fn review_config_accepts_overrides() {
        let config = ReviewConfig::from_values(Some("custom-model".to_string()));

        assert_eq!(config.model, "custom-model");
    }

    #[test]
    fn real_openai_review_generator_requires_api_key() {
        let result = RigOpenAiReviewGenerator::from_optional_api_key(
            ReviewConfig::default(),
            prompt("v1", "prompt text"),
            None,
        );

        assert!(matches!(
            result,
            Err(RigOpenAiReviewGeneratorError::MissingOpenAiApiKey)
        ));
    }

    #[tokio::test]
    async fn rig_review_generator_uses_configured_model_and_prompt_version() {
        let provider = FakeReviewProvider::succeeding("review text");
        let generator = RigOpenAiReviewGenerator::new(
            ReviewConfig {
                model: "custom-model".to_string(),
            },
            prompt("custom-prompt", "injected instructions"),
            provider.clone(),
        );

        assert_eq!(generator.model(), "custom-model");
        assert_eq!(generator.prompt_version(), "custom-prompt");
        assert_eq!(
            generator
                .generate_daily_review(&[JournalEntryWithExtraction {
                    id: 1,
                    entry: entry(28, "wrote a test"),
                    extraction: None,
                }])
                .await
                .unwrap(),
            "review text"
        );
        assert_eq!(provider.models(), vec!["custom-model".to_string()]);
        assert_eq!(
            provider.instructions(),
            vec!["injected instructions".to_string()]
        );
    }

    #[tokio::test]
    async fn generated_prompt_contains_only_entries_passed_to_generator() {
        let provider = FakeReviewProvider::succeeding("review text");
        let generator = RigOpenAiReviewGenerator::new(
            ReviewConfig::default(),
            prompt("v1", "injected instructions"),
            provider.clone(),
        );

        generator
            .generate_daily_review(&[JournalEntryWithExtraction {
                id: 1,
                entry: entry(28, "requested date entry"),
                extraction: None,
            }])
            .await
            .unwrap();

        let prompts = provider.prompts();
        assert_eq!(prompts.len(), 1);
        assert!(prompts[0].contains("requested date entry"));
        assert!(!prompts[0].contains("previous date entry"));
    }

    #[tokio::test]
    async fn maps_provider_failure_to_generation_error() {
        let provider = FakeReviewProvider::failing("provider down");
        let generator = RigOpenAiReviewGenerator::new(
            ReviewConfig::default(),
            prompt("v1", "injected instructions"),
            provider,
        );

        let error = generator
            .generate_daily_review(&[JournalEntryWithExtraction {
                id: 1,
                entry: entry(28, "wrote a test"),
                extraction: None,
            }])
            .await
            .unwrap_err();

        assert_eq!(error, ReviewGenerationError::new("provider down"));
    }

    #[test]
    fn generated_prompt_requests_review_format() {
        let prompt_text = build_daily_review_prompt(&[JournalEntryWithExtraction {
            id: 1,
            entry: entry(28, "finished the feature"),
            extraction: None,
        }]);

        assert!(prompt_text.contains("Summary:"));
        assert!(prompt_text.contains("Themes:"));
        assert!(prompt_text.contains("Pay attention tomorrow:"));
        assert!(prompt_text.contains("finished the feature"));
    }

    #[test]
    fn build_daily_review_prompt_includes_extraction_json_when_available() {
        let extraction = JournalEntryExtractionResult {
            summary: "Extracted".to_string(),
            domains: vec!["test".to_string()],
            emotions: vec![],
            behaviors: vec![],
            needs: vec![],
            possible_patterns: vec![],
        };
        let prompt_text = build_daily_review_prompt(&[JournalEntryWithExtraction {
            id: 1,
            entry: entry(28, "entry with extraction"),
            extraction: Some(extraction),
        }]);

        assert!(prompt_text.contains("entry with extraction"));
        assert!(prompt_text.contains("Entry #1"));
        assert!(prompt_text.contains("Structured extraction:"));
        assert!(prompt_text.contains("\"summary\":\"Extracted\""));
    }
}
