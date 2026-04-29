use std::{env, error::Error, fmt};

use async_trait::async_trait;
use rig::{
    client::CompletionClient,
    completion::Prompt,
    providers::openai::{Client as OpenAiClient, completion::GPT_5_MINI},
};

use crate::journal::entry::JournalEntry;

pub const DEFAULT_REVIEW_MODEL: &str = GPT_5_MINI;
pub const DEFAULT_REVIEW_PROMPT_VERSION: &str = "daily-review-v1";

const DAILY_REVIEW_PREAMBLE: &str = r#"You generate concise daily journal reviews.

Rules:
- Use only the journal entries provided in the prompt.
- Do not infer facts that are not supported by the entries.
- Do not refer to past days or long-term patterns.
- Summarize emotional and practical themes from today only.
- Identify notable patterns or tensions from today only.
- Suggest one or two practical points of attention for tomorrow.
- Keep the review concise, readable, and grounded.
- Avoid clinical diagnosis or therapy-style overreach.
- Do not include a top-level "Today's review" heading; the application adds it."#;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewConfig {
    pub model: String,
    pub prompt_version: String,
}

impl Default for ReviewConfig {
    fn default() -> Self {
        Self {
            model: DEFAULT_REVIEW_MODEL.to_string(),
            prompt_version: DEFAULT_REVIEW_PROMPT_VERSION.to_string(),
        }
    }
}

impl ReviewConfig {
    pub fn from_env() -> Self {
        Self::from_values(
            env::var("FROID_REVIEW_MODEL").ok(),
            env::var("FROID_REVIEW_PROMPT_VERSION").ok(),
        )
    }

    pub(crate) fn from_values(model: Option<String>, prompt_version: Option<String>) -> Self {
        let defaults = Self::default();
        Self {
            model: model
                .filter(|value| !value.trim().is_empty())
                .unwrap_or(defaults.model),
            prompt_version: prompt_version
                .filter(|value| !value.trim().is_empty())
                .unwrap_or(defaults.prompt_version),
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
        entries: &[JournalEntry],
    ) -> Result<String, ReviewGenerationError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RigOpenAiReviewGeneratorError {
    MissingOpenAiApiKey,
    Client(String),
}

impl fmt::Display for RigOpenAiReviewGeneratorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingOpenAiApiKey => write!(f, "OPENAI_API_KEY is required"),
            Self::Client(message) => {
                write!(f, "failed to construct OpenAI review generator: {message}")
            }
        }
    }
}

impl Error for RigOpenAiReviewGeneratorError {}

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
pub trait ReviewProvider: Send + Sync {
    async fn complete_daily_review(
        &self,
        model: &str,
        prompt: &str,
    ) -> Result<String, ReviewProviderError>;
}

#[derive(Clone)]
pub struct RigOpenAiReviewProvider {
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
        prompt: &str,
    ) -> Result<String, ReviewProviderError> {
        let agent = self
            .client
            .agent(model)
            .preamble(DAILY_REVIEW_PREAMBLE)
            .temperature(0.2)
            .max_tokens(700)
            .build();

        agent
            .prompt(prompt)
            .await
            .map_err(|error| ReviewProviderError::Request(error.to_string()))
    }
}

#[derive(Clone)]
pub struct RigOpenAiReviewGenerator<P = RigOpenAiReviewProvider> {
    config: ReviewConfig,
    provider: P,
}

impl RigOpenAiReviewGenerator<RigOpenAiReviewProvider> {
    pub fn from_env() -> Result<Self, RigOpenAiReviewGeneratorError> {
        Self::from_optional_api_key(ReviewConfig::from_env(), env::var("OPENAI_API_KEY").ok())
    }

    pub fn from_optional_api_key(
        config: ReviewConfig,
        api_key: Option<String>,
    ) -> Result<Self, RigOpenAiReviewGeneratorError> {
        let api_key = api_key
            .filter(|value| !value.trim().is_empty())
            .ok_or(RigOpenAiReviewGeneratorError::MissingOpenAiApiKey)?;
        let provider = RigOpenAiReviewProvider::new(&api_key)?;

        Ok(Self { config, provider })
    }
}

impl<P> RigOpenAiReviewGenerator<P>
where
    P: ReviewProvider,
{
    #[cfg(test)]
    pub(crate) fn new(config: ReviewConfig, provider: P) -> Self {
        Self { config, provider }
    }
}

#[async_trait]
impl<P> ReviewGenerator for RigOpenAiReviewGenerator<P>
where
    P: ReviewProvider,
{
    fn model(&self) -> &str {
        &self.config.model
    }

    fn prompt_version(&self) -> &str {
        &self.config.prompt_version
    }

    async fn generate_daily_review(
        &self,
        entries: &[JournalEntry],
    ) -> Result<String, ReviewGenerationError> {
        let prompt = build_daily_review_prompt(entries);
        self.provider
            .complete_daily_review(&self.config.model, &prompt)
            .await
            .map_err(|error| ReviewGenerationError::new(error.to_string()))
    }
}

fn build_daily_review_prompt(entries: &[JournalEntry]) -> String {
    let formatted_entries = entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            format!(
                "{}. [{} UTC] {}",
                index + 1,
                entry.received_at.format("%Y-%m-%d %H:%M"),
                entry.text
            )
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
    use crate::journal::entry::JournalEntry;

    #[derive(Debug, Clone)]
    pub struct FakeReviewGenerator {
        model: String,
        prompt_version: String,
        results: Arc<Mutex<VecDeque<Result<String, ReviewGenerationError>>>>,
        calls: Arc<AtomicUsize>,
        entries_seen: Arc<Mutex<Vec<Vec<JournalEntry>>>>,
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

        pub fn entries_seen(&self) -> Vec<Vec<JournalEntry>> {
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
            entries: &[JournalEntry],
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

    #[derive(Debug, Clone)]
    struct FakeReviewProvider {
        result: Result<String, ReviewProviderError>,
        prompts: Arc<Mutex<Vec<String>>>,
        models: Arc<Mutex<Vec<String>>>,
    }

    impl FakeReviewProvider {
        fn succeeding(review_text: &str) -> Self {
            Self {
                result: Ok(review_text.to_string()),
                prompts: Arc::new(Mutex::new(Vec::new())),
                models: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn failing(error_message: &str) -> Self {
            Self {
                result: Err(ReviewProviderError::Request(error_message.to_string())),
                prompts: Arc::new(Mutex::new(Vec::new())),
                models: Arc::new(Mutex::new(Vec::new())),
            }
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
            prompt: &str,
        ) -> Result<String, ReviewProviderError> {
            self.models.lock().unwrap().push(model.to_string());
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

    #[test]
    fn review_config_uses_defaults() {
        let config = ReviewConfig::from_values(None, None);

        assert_eq!(config.model, DEFAULT_REVIEW_MODEL);
        assert_eq!(config.prompt_version, DEFAULT_REVIEW_PROMPT_VERSION);
    }

    #[test]
    fn review_config_accepts_overrides() {
        let config =
            ReviewConfig::from_values(Some("custom-model".to_string()), Some("v2".to_string()));

        assert_eq!(config.model, "custom-model");
        assert_eq!(config.prompt_version, "v2");
    }

    #[test]
    fn real_openai_review_generator_requires_api_key() {
        let result = RigOpenAiReviewGenerator::from_optional_api_key(ReviewConfig::default(), None);

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
                prompt_version: "custom-prompt".to_string(),
            },
            provider.clone(),
        );

        assert_eq!(generator.model(), "custom-model");
        assert_eq!(generator.prompt_version(), "custom-prompt");
        assert_eq!(
            generator
                .generate_daily_review(&[entry(28, "wrote a test")])
                .await
                .unwrap(),
            "review text"
        );
        assert_eq!(provider.models(), vec!["custom-model".to_string()]);
    }

    #[tokio::test]
    async fn prompt_contains_only_entries_passed_to_generator() {
        let provider = FakeReviewProvider::succeeding("review text");
        let generator = RigOpenAiReviewGenerator::new(ReviewConfig::default(), provider.clone());

        generator
            .generate_daily_review(&[entry(28, "requested date entry")])
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
        let generator = RigOpenAiReviewGenerator::new(ReviewConfig::default(), provider);

        let error = generator
            .generate_daily_review(&[entry(28, "wrote a test")])
            .await
            .unwrap_err();

        assert_eq!(error, ReviewGenerationError::new("provider down"));
    }

    #[test]
    fn prompt_instructs_grounded_same_day_review() {
        let prompt = build_daily_review_prompt(&[entry(28, "finished the feature")]);

        assert!(DAILY_REVIEW_PREAMBLE.contains("Use only the journal entries"));
        assert!(DAILY_REVIEW_PREAMBLE.contains("Do not refer to past days"));
        assert!(DAILY_REVIEW_PREAMBLE.contains("Avoid clinical diagnosis"));
        assert!(prompt.contains("Summary:"));
        assert!(prompt.contains("Themes:"));
        assert!(prompt.contains("Pay attention tomorrow:"));
        assert!(prompt.contains("finished the feature"));
    }
}
