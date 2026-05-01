use std::{error::Error, fmt};

use async_trait::async_trait;

use crate::journal::extraction::{
    JournalEntryExtractionGenerator,
    repository::{JournalEntryExtractionRepository, JournalEntryExtractionRepositoryError},
    validation::validate_extraction_json,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalEntryExtractionBackfillResult {
    pub attempted: u32,
    pub completed: u32,
    pub failed: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JournalEntryExtractionRunResult {
    Completed,
    Failed,
    AlreadyExists,
}

#[derive(Debug)]
pub enum JournalEntryExtractionServiceError {
    Repository(JournalEntryExtractionRepositoryError),
}

impl fmt::Display for JournalEntryExtractionServiceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Repository(error) => write!(f, "{error}"),
        }
    }
}

impl Error for JournalEntryExtractionServiceError {}

impl From<JournalEntryExtractionRepositoryError> for JournalEntryExtractionServiceError {
    fn from(error: JournalEntryExtractionRepositoryError) -> Self {
        Self::Repository(error)
    }
}

#[async_trait]
pub trait JournalEntryExtractionRunner: Send + Sync {
    async fn extract_entry(
        &self,
        journal_entry_id: i64,
        text: &str,
    ) -> Result<(), JournalEntryExtractionServiceError>;
}

#[derive(Clone)]
pub struct JournalEntryExtractionService<G> {
    repository: JournalEntryExtractionRepository,
    generator: G,
}

impl<G> JournalEntryExtractionService<G> {
    pub fn new(repository: JournalEntryExtractionRepository, generator: G) -> Self {
        Self {
            repository,
            generator,
        }
    }
}

#[async_trait]
impl<G> JournalEntryExtractionRunner for JournalEntryExtractionService<G>
where
    G: JournalEntryExtractionGenerator,
{
    async fn extract_entry(
        &self,
        journal_entry_id: i64,
        text: &str,
    ) -> Result<(), JournalEntryExtractionServiceError> {
        self.extract_entry_with_result(journal_entry_id, text)
            .await
            .map(|_| ())
    }
}

impl<G> JournalEntryExtractionService<G>
where
    G: JournalEntryExtractionGenerator,
{
    pub async fn backfill_missing_extractions(
        &self,
        limit: u32,
    ) -> Result<JournalEntryExtractionBackfillResult, JournalEntryExtractionServiceError> {
        let candidates = self
            .repository
            .find_entries_missing_extraction(limit)
            .await?;

        let mut result = JournalEntryExtractionBackfillResult {
            attempted: candidates.len() as u32,
            completed: 0,
            failed: 0,
        };

        for candidate in candidates {
            match self
                .extract_entry_with_result(candidate.journal_entry_id, &candidate.raw_text)
                .await?
            {
                JournalEntryExtractionRunResult::Completed => result.completed += 1,
                JournalEntryExtractionRunResult::Failed => result.failed += 1,
                JournalEntryExtractionRunResult::AlreadyExists => {}
            }
        }

        Ok(result)
    }

    async fn extract_entry_with_result(
        &self,
        journal_entry_id: i64,
        text: &str,
    ) -> Result<JournalEntryExtractionRunResult, JournalEntryExtractionServiceError> {
        let inserted = self
            .repository
            .insert_pending_if_absent(
                journal_entry_id,
                self.generator.model(),
                self.generator.prompt_version(),
            )
            .await?;

        if !inserted {
            return Ok(JournalEntryExtractionRunResult::AlreadyExists);
        }

        let result = match self.generator.generate_entry_extraction(text).await {
            Ok(raw_json) => match validate_extraction_json(&raw_json) {
                Ok(valid_json) => {
                    self.repository
                        .mark_completed(
                            journal_entry_id,
                            &valid_json,
                            self.generator.model(),
                            self.generator.prompt_version(),
                        )
                        .await?;
                    JournalEntryExtractionRunResult::Completed
                }
                Err(error) => {
                    self.record_failure(journal_entry_id, error).await?;
                    JournalEntryExtractionRunResult::Failed
                }
            },
            Err(error) => {
                self.record_failure(journal_entry_id, error).await?;
                JournalEntryExtractionRunResult::Failed
            }
        };

        Ok(result)
    }
}

impl<G> JournalEntryExtractionService<G>
where
    G: JournalEntryExtractionGenerator,
{
    async fn record_failure(
        &self,
        journal_entry_id: i64,
        error: impl std::fmt::Display,
    ) -> Result<(), JournalEntryExtractionServiceError> {
        self.repository
            .mark_failed(
                journal_entry_id,
                self.generator.model(),
                self.generator.prompt_version(),
                &error.to_string(),
            )
            .await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::VecDeque,
        sync::{
            Arc, Mutex,
            atomic::{AtomicUsize, Ordering},
        },
    };

    use async_trait::async_trait;
    use sqlx::SqlitePool;

    use super::*;
    use crate::{
        database,
        journal::extraction::{
            JournalEntryExtractionGenerationError, JournalEntryExtractionStatus,
            repository::JournalEntryExtractionRepository,
        },
        messages::{IncomingMessage, MessageSource},
    };

    #[derive(Clone)]
    struct FakeGenerator {
        fallback: Result<String, JournalEntryExtractionGenerationError>,
        responses: Arc<Mutex<VecDeque<Result<String, JournalEntryExtractionGenerationError>>>>,
        calls: Arc<AtomicUsize>,
        notes: Arc<Mutex<Vec<String>>>,
    }

    impl FakeGenerator {
        fn succeeding(json: &str) -> Self {
            Self {
                fallback: Ok(json.to_string()),
                responses: Arc::new(Mutex::new(VecDeque::new())),
                calls: Arc::new(AtomicUsize::new(0)),
                notes: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn failing(message: &str) -> Self {
            Self {
                fallback: Err(JournalEntryExtractionGenerationError::new(message)),
                responses: Arc::new(Mutex::new(VecDeque::new())),
                calls: Arc::new(AtomicUsize::new(0)),
                notes: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn with_responses(
            responses: Vec<Result<String, JournalEntryExtractionGenerationError>>,
        ) -> Self {
            Self {
                fallback: Ok(valid_json().to_string()),
                responses: Arc::new(Mutex::new(VecDeque::from(responses))),
                calls: Arc::new(AtomicUsize::new(0)),
                notes: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }

        fn notes(&self) -> Vec<String> {
            self.notes.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl JournalEntryExtractionGenerator for FakeGenerator {
        fn model(&self) -> &str {
            "test-extraction-model"
        }

        fn prompt_version(&self) -> &str {
            "entry_extraction_v1"
        }

        async fn generate_entry_extraction(
            &self,
            note: &str,
        ) -> Result<String, JournalEntryExtractionGenerationError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.notes.lock().unwrap().push(note.to_string());
            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| self.fallback.clone())
        }
    }

    async fn setup(
        generator: FakeGenerator,
    ) -> (
        JournalEntryExtractionService<FakeGenerator>,
        JournalEntryExtractionRepository,
        SqlitePool,
    ) {
        database::register_sqlite_vec_extension();
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        let repo = JournalEntryExtractionRepository::new(pool.clone());
        let service = JournalEntryExtractionService::new(repo.clone(), generator);
        (service, repo, pool)
    }

    async fn insert_entry(pool: &SqlitePool, text: &str) -> i64 {
        insert_entry_with_message_id(pool, "100", text).await
    }

    async fn insert_entry_with_message_id(
        pool: &SqlitePool,
        source_message_id: &str,
        text: &str,
    ) -> i64 {
        let message = IncomingMessage {
            source: MessageSource::Telegram,
            source_conversation_id: "42".to_string(),
            source_message_id: source_message_id.to_string(),
            user_id: "7".to_string(),
            text: text.to_string(),
            received_at: chrono::Utc::now(),
        };

        crate::journal::repository::JournalRepository::new(pool.clone())
            .store(&message)
            .await
            .unwrap()
            .unwrap()
    }

    fn valid_json() -> &'static str {
        r#"{
            "summary": "The note mentions work stress.",
            "domains": ["work"],
            "emotions": [{"label": "anxiety", "intensity": 0.6, "confidence": 0.7}],
            "behaviors": [{"label": "overthinking", "valence": "negative", "confidence": 0.6}],
            "needs": [{"label": "control", "status": "activated", "confidence": 0.5}],
            "possible_patterns": [{"description": "Work uncertainty may be associated with overthinking in this note.", "confidence": 0.4}]
        }"#
    }

    #[tokio::test]
    async fn stores_completed_extraction_when_generation_succeeds() {
        let (service, repo, pool) = setup(FakeGenerator::succeeding(valid_json())).await;
        let entry_id = insert_entry(&pool, "Work felt stressful today").await;

        service
            .extract_entry(entry_id, "Work felt stressful today")
            .await
            .unwrap();

        let extraction = repo
            .find_by_journal_entry_id(entry_id)
            .await
            .unwrap()
            .unwrap();
        let json: serde_json::Value =
            serde_json::from_str(extraction.extraction_json.as_ref().unwrap()).unwrap();

        assert_eq!(extraction.status, JournalEntryExtractionStatus::Completed);
        assert_eq!(extraction.model, "test-extraction-model");
        assert_eq!(extraction.prompt_version, "entry_extraction_v1");
        assert_eq!(json["summary"], "The note mentions work stress.");
    }

    #[tokio::test]
    async fn records_failed_extraction_when_generation_fails() {
        let (service, repo, pool) = setup(FakeGenerator::failing("provider down")).await;
        let entry_id = insert_entry(&pool, "Work felt stressful today").await;

        service
            .extract_entry(entry_id, "Work felt stressful today")
            .await
            .unwrap();

        let extraction = repo
            .find_by_journal_entry_id(entry_id)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(extraction.status, JournalEntryExtractionStatus::Failed);
        assert_eq!(extraction.extraction_json, None);
        assert_eq!(extraction.error_message, Some("provider down".to_string()));
    }

    #[tokio::test]
    async fn records_failed_extraction_when_generation_returns_invalid_json() {
        let (service, repo, pool) = setup(FakeGenerator::succeeding("not json")).await;
        let entry_id = insert_entry(&pool, "Work felt stressful today").await;

        service
            .extract_entry(entry_id, "Work felt stressful today")
            .await
            .unwrap();

        let extraction = repo
            .find_by_journal_entry_id(entry_id)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(extraction.status, JournalEntryExtractionStatus::Failed);
        assert!(extraction.error_message.unwrap().contains("not valid JSON"));
    }

    #[tokio::test]
    async fn does_not_generate_duplicate_extraction_for_existing_entry() {
        let generator = FakeGenerator::succeeding(valid_json());
        let (service, repo, pool) = setup(generator.clone()).await;
        let entry_id = insert_entry(&pool, "Work felt stressful today").await;

        service.extract_entry(entry_id, "first call").await.unwrap();
        service
            .extract_entry(entry_id, "second call")
            .await
            .unwrap();
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM journal_entry_extractions")
            .fetch_one(&pool)
            .await
            .unwrap();

        assert_eq!(generator.calls(), 1);
        assert_eq!(count, 1);
        assert!(
            repo.find_by_journal_entry_id(entry_id)
                .await
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn backfill_processes_missing_entries_up_to_limit_oldest_first() {
        let generator = FakeGenerator::succeeding(valid_json());
        let (service, repo, pool) = setup(generator.clone()).await;
        let first = insert_entry_with_message_id(&pool, "1", "first").await;
        let second = insert_entry_with_message_id(&pool, "2", "second").await;
        insert_entry_with_message_id(&pool, "3", "third").await;

        let result = service.backfill_missing_extractions(2).await.unwrap();

        assert_eq!(
            result,
            JournalEntryExtractionBackfillResult {
                attempted: 2,
                completed: 2,
                failed: 0,
            }
        );
        assert_eq!(generator.calls(), 2);
        assert_eq!(generator.notes(), vec!["first", "second"]);
        assert!(
            repo.find_by_journal_entry_id(first)
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            repo.find_by_journal_entry_id(second)
                .await
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn backfill_skips_entries_that_already_have_extractions() {
        let generator = FakeGenerator::succeeding(valid_json());
        let (service, repo, pool) = setup(generator.clone()).await;
        let first = insert_entry_with_message_id(&pool, "1", "first").await;
        let second = insert_entry_with_message_id(&pool, "2", "second").await;
        service
            .extract_entry(first, "already extracted")
            .await
            .unwrap();

        let result = service.backfill_missing_extractions(10).await.unwrap();
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM journal_entry_extractions")
            .fetch_one(&pool)
            .await
            .unwrap();

        assert_eq!(
            result,
            JournalEntryExtractionBackfillResult {
                attempted: 1,
                completed: 1,
                failed: 0,
            }
        );
        assert_eq!(generator.calls(), 2);
        assert_eq!(count, 2);
        assert!(
            repo.find_by_journal_entry_id(second)
                .await
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn backfill_records_failure_and_continues() {
        let generator = FakeGenerator::with_responses(vec![
            Err(JournalEntryExtractionGenerationError::new("provider down")),
            Ok(valid_json().to_string()),
        ]);
        let (service, repo, pool) = setup(generator.clone()).await;
        let first = insert_entry_with_message_id(&pool, "1", "first").await;
        let second = insert_entry_with_message_id(&pool, "2", "second").await;

        let result = service.backfill_missing_extractions(10).await.unwrap();

        let first_extraction = repo.find_by_journal_entry_id(first).await.unwrap().unwrap();
        let second_extraction = repo
            .find_by_journal_entry_id(second)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(
            result,
            JournalEntryExtractionBackfillResult {
                attempted: 2,
                completed: 1,
                failed: 1,
            }
        );
        assert_eq!(generator.calls(), 2);
        assert_eq!(
            first_extraction.status,
            JournalEntryExtractionStatus::Failed
        );
        assert_eq!(
            first_extraction.error_message,
            Some("provider down".to_string())
        );
        assert_eq!(
            second_extraction.status,
            JournalEntryExtractionStatus::Completed
        );
    }

    #[tokio::test]
    async fn backfill_resumes_by_processing_only_entries_still_missing_extractions() {
        let generator = FakeGenerator::succeeding(valid_json());
        let (service, repo, pool) = setup(generator.clone()).await;
        let first = insert_entry_with_message_id(&pool, "1", "first").await;
        let second = insert_entry_with_message_id(&pool, "2", "second").await;

        let first_result = service.backfill_missing_extractions(1).await.unwrap();
        let second_result = service.backfill_missing_extractions(10).await.unwrap();

        assert_eq!(
            first_result,
            JournalEntryExtractionBackfillResult {
                attempted: 1,
                completed: 1,
                failed: 0,
            }
        );
        assert_eq!(
            second_result,
            JournalEntryExtractionBackfillResult {
                attempted: 1,
                completed: 1,
                failed: 0,
            }
        );
        assert_eq!(generator.calls(), 2);
        assert!(
            repo.find_by_journal_entry_id(first)
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            repo.find_by_journal_entry_id(second)
                .await
                .unwrap()
                .is_some()
        );
    }
}
