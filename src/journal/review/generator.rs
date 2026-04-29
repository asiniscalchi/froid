use std::{error::Error, fmt};

use async_trait::async_trait;

use crate::journal::entry::JournalEntry;

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
