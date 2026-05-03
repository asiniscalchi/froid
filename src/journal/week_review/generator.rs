use std::{error::Error, fmt};

use async_trait::async_trait;

use super::WeeklyReviewInput;

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
