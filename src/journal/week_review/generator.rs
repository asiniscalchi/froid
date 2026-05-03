use std::{env, error::Error, fmt, sync::Arc};

use async_trait::async_trait;
use rig::{
    client::CompletionClient,
    completion::Prompt,
    providers::openai::{Client as OpenAiClient, completion::GPT_5_MINI},
};

use crate::journal::review::signals::types::DailyReviewSignal;

use super::{DailyReviewSlice, WeeklyReviewInput, prompt::WeeklyReviewPrompt};

pub const DEFAULT_WEEK_REVIEW_MODEL: &str = GPT_5_MINI;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WeeklyReviewConfig {
    pub model: String,
}

impl Default for WeeklyReviewConfig {
    fn default() -> Self {
        Self {
            model: DEFAULT_WEEK_REVIEW_MODEL.to_string(),
        }
    }
}

impl WeeklyReviewConfig {
    pub fn from_env() -> Self {
        Self::from_values(env::var("FROID_WEEK_REVIEW_MODEL").ok())
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
pub struct WeeklyReviewGenerationError {
    message: String,
}

impl WeeklyReviewGenerationError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for WeeklyReviewGenerationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl Error for WeeklyReviewGenerationError {}

#[async_trait]
pub trait WeeklyReviewGenerator: Send + Sync {
    fn model(&self) -> &str;
    fn prompt_version(&self) -> &str;

    async fn generate_weekly_review(
        &self,
        input: &WeeklyReviewInput,
    ) -> Result<String, WeeklyReviewGenerationError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RigOpenAiWeeklyReviewGeneratorError {
    MissingOpenAiApiKey,
    Client(String),
}

impl fmt::Display for RigOpenAiWeeklyReviewGeneratorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingOpenAiApiKey => write!(f, "OPENAI_API_KEY is required"),
            Self::Client(message) => {
                write!(
                    f,
                    "failed to construct OpenAI weekly review generator: {message}"
                )
            }
        }
    }
}

impl Error for RigOpenAiWeeklyReviewGeneratorError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WeeklyReviewProviderError {
    Request(String),
}

impl fmt::Display for WeeklyReviewProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Request(message) => write!(f, "{message}"),
        }
    }
}

impl Error for WeeklyReviewProviderError {}

#[async_trait]
pub(crate) trait WeeklyReviewProvider: Send + Sync {
    async fn complete_weekly_review(
        &self,
        model: &str,
        instructions: &str,
        prompt: &str,
    ) -> Result<String, WeeklyReviewProviderError>;
}

#[derive(Clone)]
struct RigOpenAiWeeklyReviewProvider {
    client: OpenAiClient,
}

impl RigOpenAiWeeklyReviewProvider {
    fn new(api_key: &str) -> Result<Self, RigOpenAiWeeklyReviewGeneratorError> {
        let client = OpenAiClient::new(api_key)
            .map_err(|error| RigOpenAiWeeklyReviewGeneratorError::Client(error.to_string()))?;
        Ok(Self { client })
    }
}

#[async_trait]
impl WeeklyReviewProvider for RigOpenAiWeeklyReviewProvider {
    async fn complete_weekly_review(
        &self,
        model: &str,
        instructions: &str,
        prompt: &str,
    ) -> Result<String, WeeklyReviewProviderError> {
        let agent = self.client.agent(model).preamble(instructions).build();

        agent
            .prompt(prompt)
            .await
            .map_err(|error| WeeklyReviewProviderError::Request(error.to_string()))
    }
}

#[derive(Clone)]
pub struct RigOpenAiWeeklyReviewGenerator {
    config: WeeklyReviewConfig,
    prompt: WeeklyReviewPrompt,
    provider: Arc<dyn WeeklyReviewProvider>,
}

impl RigOpenAiWeeklyReviewGenerator {
    pub fn from_optional_api_key(
        config: WeeklyReviewConfig,
        prompt: WeeklyReviewPrompt,
        api_key: Option<String>,
    ) -> Result<Self, RigOpenAiWeeklyReviewGeneratorError> {
        let api_key = api_key
            .filter(|value| !value.trim().is_empty())
            .ok_or(RigOpenAiWeeklyReviewGeneratorError::MissingOpenAiApiKey)?;
        let provider = RigOpenAiWeeklyReviewProvider::new(&api_key)?;

        Ok(Self {
            config,
            prompt,
            provider: Arc::new(provider),
        })
    }

    #[cfg(test)]
    pub(crate) fn new<P>(
        config: WeeklyReviewConfig,
        prompt: WeeklyReviewPrompt,
        provider: P,
    ) -> Self
    where
        P: WeeklyReviewProvider + 'static,
    {
        Self {
            config,
            prompt,
            provider: Arc::new(provider),
        }
    }
}

#[async_trait]
impl WeeklyReviewGenerator for RigOpenAiWeeklyReviewGenerator {
    fn model(&self) -> &str {
        &self.config.model
    }

    fn prompt_version(&self) -> &str {
        &self.prompt.version
    }

    async fn generate_weekly_review(
        &self,
        input: &WeeklyReviewInput,
    ) -> Result<String, WeeklyReviewGenerationError> {
        let prompt = build_weekly_review_prompt(input);
        self.provider
            .complete_weekly_review(&self.config.model, &self.prompt.text, &prompt)
            .await
            .map_err(|error| WeeklyReviewGenerationError::new(error.to_string()))
    }
}

fn build_weekly_review_prompt(input: &WeeklyReviewInput) -> String {
    let formatted_days = input
        .days
        .iter()
        .enumerate()
        .map(|(index, day)| format_day(index + 1, day))
        .collect::<Vec<_>>()
        .join("\n\n");

    format!(
        r#"Write a weekly review using only these daily reviews and signals.

Return this format:
Summary:
...

Themes across the week:
- ...
- ...

Tensions and unmet needs:
- ...

Pay attention next week:
- ...

Week of {} (Monday):

Daily reviews and signals:
{formatted_days}"#,
        input.week_start
    )
}

fn format_day(day_index: usize, day: &DailyReviewSlice) -> String {
    let weekday = day.date.format("%A");
    let mut formatted = format!(
        "[Day {day_index}] {} ({weekday}):\nReview:\n{}",
        day.date, day.review_text
    );

    if !day.signals.is_empty() {
        formatted.push_str("\n\nSignals:");
        for signal in &day.signals {
            formatted.push_str("\n- ");
            formatted.push_str(&format_signal(signal));
        }
    }

    formatted
}

fn format_signal(signal: &DailyReviewSignal) -> String {
    let mut parts = format!("{}: {}", signal.signal_type.as_str(), signal.label);

    if let Some(status) = &signal.status {
        parts.push_str(&format!(" [{}]", status_str(status)));
    }
    if let Some(valence) = &signal.valence {
        parts.push_str(&format!(" [{}]", valence_str(valence)));
    }

    parts.push_str(&format!(
        " (strength={:.2}, confidence={:.2}) — \"{}\"",
        signal.strength, signal.confidence, signal.evidence
    ));

    parts
}

fn status_str(status: &crate::journal::extraction::NeedStatus) -> &'static str {
    use crate::journal::extraction::NeedStatus;
    match status {
        NeedStatus::Activated => "activated",
        NeedStatus::Unmet => "unmet",
        NeedStatus::Fulfilled => "fulfilled",
        NeedStatus::Unclear => "unclear",
    }
}

fn valence_str(valence: &crate::journal::extraction::BehaviorValence) -> &'static str {
    use crate::journal::extraction::BehaviorValence;
    match valence {
        BehaviorValence::Positive => "positive",
        BehaviorValence::Negative => "negative",
        BehaviorValence::Ambiguous => "ambiguous",
        BehaviorValence::Neutral => "neutral",
        BehaviorValence::Unclear => "unclear",
    }
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

    use super::{WeeklyReviewGenerationError, WeeklyReviewGenerator};
    use crate::journal::week_review::WeeklyReviewInput;

    #[derive(Debug, Clone)]
    pub struct FakeWeeklyReviewGenerator {
        model: String,
        prompt_version: String,
        results: Arc<Mutex<VecDeque<Result<String, WeeklyReviewGenerationError>>>>,
        calls: Arc<AtomicUsize>,
        inputs_seen: Arc<Mutex<Vec<WeeklyReviewInput>>>,
    }

    impl FakeWeeklyReviewGenerator {
        pub fn succeeding(review_text: impl Into<String>) -> Self {
            Self::new(vec![Ok(review_text.into())])
        }

        pub fn failing(error_message: impl Into<String>) -> Self {
            Self::new(vec![Err(WeeklyReviewGenerationError::new(error_message))])
        }

        pub fn new(results: Vec<Result<String, WeeklyReviewGenerationError>>) -> Self {
            Self {
                model: "fake-weekly-review-model".to_string(),
                prompt_version: "fake-weekly-prompt-v1".to_string(),
                results: Arc::new(Mutex::new(VecDeque::from(results))),
                calls: Arc::new(AtomicUsize::new(0)),
                inputs_seen: Arc::new(Mutex::new(Vec::new())),
            }
        }

        pub fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }

        pub fn inputs_seen(&self) -> Vec<WeeklyReviewInput> {
            self.inputs_seen.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl WeeklyReviewGenerator for FakeWeeklyReviewGenerator {
        fn model(&self) -> &str {
            &self.model
        }

        fn prompt_version(&self) -> &str {
            &self.prompt_version
        }

        async fn generate_weekly_review(
            &self,
            input: &WeeklyReviewInput,
        ) -> Result<String, WeeklyReviewGenerationError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.inputs_seen.lock().unwrap().push(input.clone());

            self.results
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Ok("fake weekly review".to_string()))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use chrono::{Duration, NaiveDate};

    use super::*;
    use crate::journal::extraction::{BehaviorValence, NeedStatus};
    use crate::journal::review::signals::types::SignalType;
    use crate::journal::week_review::DailyReviewSlice;

    #[derive(Debug, Clone)]
    struct FakeWeeklyReviewProvider {
        result: Result<String, WeeklyReviewProviderError>,
        instructions: Arc<Mutex<Vec<String>>>,
        prompts: Arc<Mutex<Vec<String>>>,
        models: Arc<Mutex<Vec<String>>>,
    }

    impl FakeWeeklyReviewProvider {
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
                result: Err(WeeklyReviewProviderError::Request(
                    error_message.to_string(),
                )),
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
    impl WeeklyReviewProvider for FakeWeeklyReviewProvider {
        async fn complete_weekly_review(
            &self,
            model: &str,
            instructions: &str,
            prompt: &str,
        ) -> Result<String, WeeklyReviewProviderError> {
            self.models.lock().unwrap().push(model.to_string());
            self.instructions
                .lock()
                .unwrap()
                .push(instructions.to_string());
            self.prompts.lock().unwrap().push(prompt.to_string());
            self.result.clone()
        }
    }

    fn week_start() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 4, 27).unwrap()
    }

    fn day(offset: i64) -> NaiveDate {
        week_start() + Duration::days(offset)
    }

    fn slice(date: NaiveDate, text: &str, signals: Vec<DailyReviewSignal>) -> DailyReviewSlice {
        DailyReviewSlice {
            date,
            review_text: text.to_string(),
            signals,
        }
    }

    fn signal(
        signal_type: SignalType,
        label: &str,
        status: Option<NeedStatus>,
        valence: Option<BehaviorValence>,
        strength: f32,
        confidence: f32,
        evidence: &str,
    ) -> DailyReviewSignal {
        DailyReviewSignal {
            id: 0,
            daily_review_id: 0,
            user_id: "user-1".to_string(),
            review_date: day(0),
            signal_type,
            label: label.to_string(),
            status,
            valence,
            strength,
            confidence,
            evidence: evidence.to_string(),
            model: "model".to_string(),
            prompt_version: "v1".to_string(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    fn weekly_prompt(version: &str, text: &str) -> WeeklyReviewPrompt {
        WeeklyReviewPrompt {
            version: version.to_string(),
            text: text.to_string(),
        }
    }

    #[test]
    fn weekly_review_config_uses_defaults() {
        let config = WeeklyReviewConfig::from_values(None);

        assert_eq!(config.model, DEFAULT_WEEK_REVIEW_MODEL);
    }

    #[test]
    fn weekly_review_config_accepts_overrides() {
        let config = WeeklyReviewConfig::from_values(Some("custom-weekly-model".to_string()));

        assert_eq!(config.model, "custom-weekly-model");
    }

    #[test]
    fn real_openai_weekly_review_generator_requires_api_key() {
        let result = RigOpenAiWeeklyReviewGenerator::from_optional_api_key(
            WeeklyReviewConfig::default(),
            weekly_prompt("v1", "instructions"),
            None,
        );

        assert!(matches!(
            result,
            Err(RigOpenAiWeeklyReviewGeneratorError::MissingOpenAiApiKey)
        ));
    }

    #[tokio::test]
    async fn rig_weekly_review_generator_uses_configured_model_and_prompt_version() {
        let provider = FakeWeeklyReviewProvider::succeeding("week review");
        let generator = RigOpenAiWeeklyReviewGenerator::new(
            WeeklyReviewConfig {
                model: "custom-model".to_string(),
            },
            weekly_prompt("custom-prompt", "injected instructions"),
            provider.clone(),
        );

        assert_eq!(generator.model(), "custom-model");
        assert_eq!(generator.prompt_version(), "custom-prompt");

        let input = WeeklyReviewInput {
            week_start: week_start(),
            days: vec![slice(day(0), "monday text", vec![])],
        };

        let output = generator.generate_weekly_review(&input).await.unwrap();
        assert_eq!(output, "week review");
        assert_eq!(provider.models(), vec!["custom-model".to_string()]);
        assert_eq!(
            provider.instructions(),
            vec!["injected instructions".to_string()]
        );
    }

    #[tokio::test]
    async fn generated_prompt_contains_only_days_passed_to_generator() {
        let provider = FakeWeeklyReviewProvider::succeeding("ok");
        let generator = RigOpenAiWeeklyReviewGenerator::new(
            WeeklyReviewConfig::default(),
            weekly_prompt("v1", "instructions"),
            provider.clone(),
        );

        let input = WeeklyReviewInput {
            week_start: week_start(),
            days: vec![
                slice(day(0), "monday review", vec![]),
                slice(day(1), "tuesday review", vec![]),
            ],
        };

        generator.generate_weekly_review(&input).await.unwrap();

        let prompts = provider.prompts();
        assert_eq!(prompts.len(), 1);
        assert!(prompts[0].contains("monday review"));
        assert!(prompts[0].contains("tuesday review"));
        assert!(!prompts[0].contains("wednesday review"));
    }

    #[tokio::test]
    async fn maps_provider_failure_to_generation_error() {
        let provider = FakeWeeklyReviewProvider::failing("provider down");
        let generator = RigOpenAiWeeklyReviewGenerator::new(
            WeeklyReviewConfig::default(),
            weekly_prompt("v1", "instructions"),
            provider,
        );

        let input = WeeklyReviewInput {
            week_start: week_start(),
            days: vec![slice(day(0), "text", vec![])],
        };

        let error = generator.generate_weekly_review(&input).await.unwrap_err();

        assert_eq!(error, WeeklyReviewGenerationError::new("provider down"));
    }

    #[test]
    fn build_prompt_requests_review_format_and_includes_week_start() {
        let prompt_text = build_weekly_review_prompt(&WeeklyReviewInput {
            week_start: week_start(),
            days: vec![slice(day(0), "monday", vec![])],
        });

        assert!(prompt_text.contains("Summary:"));
        assert!(prompt_text.contains("Themes across the week:"));
        assert!(prompt_text.contains("Tensions and unmet needs:"));
        assert!(prompt_text.contains("Pay attention next week:"));
        assert!(prompt_text.contains("Week of 2026-04-27"));
    }

    #[test]
    fn build_prompt_includes_review_text_and_weekday_per_day() {
        let prompt_text = build_weekly_review_prompt(&WeeklyReviewInput {
            week_start: week_start(),
            days: vec![
                slice(day(0), "monday review text", vec![]),
                slice(day(2), "wednesday review text", vec![]),
            ],
        });

        assert!(prompt_text.contains("[Day 1] 2026-04-27 (Monday)"));
        assert!(prompt_text.contains("[Day 2] 2026-04-29 (Wednesday)"));
        assert!(prompt_text.contains("monday review text"));
        assert!(prompt_text.contains("wednesday review text"));
    }

    #[test]
    fn build_prompt_formats_signals_with_status_valence_and_evidence() {
        let prompt_text = build_weekly_review_prompt(&WeeklyReviewInput {
            week_start: week_start(),
            days: vec![slice(
                day(0),
                "review",
                vec![
                    signal(
                        SignalType::Theme,
                        "physical appearance",
                        None,
                        None,
                        0.8,
                        0.9,
                        "training and diet",
                    ),
                    signal(
                        SignalType::Need,
                        "control",
                        Some(NeedStatus::Unmet),
                        None,
                        0.7,
                        0.85,
                        "repeated attempts to regain control",
                    ),
                    signal(
                        SignalType::Behavior,
                        "exercise",
                        None,
                        Some(BehaviorValence::Positive),
                        0.6,
                        0.8,
                        "ran in the morning",
                    ),
                ],
            )],
        });

        assert!(prompt_text.contains("Signals:"));
        assert!(prompt_text.contains("theme: physical appearance"));
        assert!(prompt_text.contains("need: control [unmet]"));
        assert!(prompt_text.contains("behavior: exercise [positive]"));
        assert!(prompt_text.contains("strength=0.80, confidence=0.90"));
        assert!(prompt_text.contains("training and diet"));
    }

    #[test]
    fn build_prompt_omits_signals_section_when_day_has_no_signals() {
        let prompt_text = build_weekly_review_prompt(&WeeklyReviewInput {
            week_start: week_start(),
            days: vec![slice(day(0), "monday review", vec![])],
        });

        assert!(prompt_text.contains("monday review"));
        assert!(!prompt_text.contains("Signals:"));
    }
}
