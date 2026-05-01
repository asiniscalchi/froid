use std::{error::Error, fmt};

use async_trait::async_trait;

use crate::journal::extraction::{
    EntryExtractionGenerator,
    repository::{JournalEntryExtractionRepository, JournalEntryExtractionRepositoryError},
    validation::validate_extraction_json,
};

#[derive(Debug)]
pub enum EntryExtractionServiceError {
    Repository(JournalEntryExtractionRepositoryError),
}

impl fmt::Display for EntryExtractionServiceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Repository(error) => write!(f, "{error}"),
        }
    }
}

impl Error for EntryExtractionServiceError {}

impl From<JournalEntryExtractionRepositoryError> for EntryExtractionServiceError {
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
    ) -> Result<(), EntryExtractionServiceError>;
}

#[derive(Clone)]
pub struct EntryExtractionService<G> {
    repository: JournalEntryExtractionRepository,
    generator: G,
}

impl<G> EntryExtractionService<G> {
    pub fn new(repository: JournalEntryExtractionRepository, generator: G) -> Self {
        Self {
            repository,
            generator,
        }
    }
}

#[async_trait]
impl<G> JournalEntryExtractionRunner for EntryExtractionService<G>
where
    G: EntryExtractionGenerator,
{
    async fn extract_entry(
        &self,
        journal_entry_id: i64,
        text: &str,
    ) -> Result<(), EntryExtractionServiceError> {
        let inserted = self
            .repository
            .insert_pending_if_absent(
                journal_entry_id,
                self.generator.model(),
                self.generator.prompt_version(),
            )
            .await?;

        if !inserted {
            return Ok(());
        }

        match self.generator.generate_entry_extraction(text).await {
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
                }
                Err(error) => {
                    self.record_failure(journal_entry_id, error).await?;
                }
            },
            Err(error) => {
                self.record_failure(journal_entry_id, error).await?;
            }
        }

        Ok(())
    }
}

impl<G> EntryExtractionService<G>
where
    G: EntryExtractionGenerator,
{
    async fn record_failure(
        &self,
        journal_entry_id: i64,
        error: impl std::fmt::Display,
    ) -> Result<(), EntryExtractionServiceError> {
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
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    use async_trait::async_trait;
    use sqlx::SqlitePool;

    use super::*;
    use crate::{
        database,
        journal::extraction::{
            EntryExtractionGenerationError, JournalEntryExtractionStatus,
            repository::JournalEntryExtractionRepository,
        },
        messages::{IncomingMessage, MessageSource},
    };

    #[derive(Clone)]
    struct FakeGenerator {
        result: Result<String, EntryExtractionGenerationError>,
        calls: Arc<AtomicUsize>,
        notes: Arc<Mutex<Vec<String>>>,
    }

    impl FakeGenerator {
        fn succeeding(json: &str) -> Self {
            Self {
                result: Ok(json.to_string()),
                calls: Arc::new(AtomicUsize::new(0)),
                notes: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn failing(message: &str) -> Self {
            Self {
                result: Err(EntryExtractionGenerationError::new(message)),
                calls: Arc::new(AtomicUsize::new(0)),
                notes: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl EntryExtractionGenerator for FakeGenerator {
        fn model(&self) -> &str {
            "test-extraction-model"
        }

        fn prompt_version(&self) -> &str {
            "entry_extraction_v1"
        }

        async fn generate_entry_extraction(
            &self,
            note: &str,
        ) -> Result<String, EntryExtractionGenerationError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.notes.lock().unwrap().push(note.to_string());
            self.result.clone()
        }
    }

    async fn setup(
        generator: FakeGenerator,
    ) -> (
        EntryExtractionService<FakeGenerator>,
        JournalEntryExtractionRepository,
        SqlitePool,
    ) {
        database::register_sqlite_vec_extension();
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        let repo = JournalEntryExtractionRepository::new(pool.clone());
        let service = EntryExtractionService::new(repo.clone(), generator);
        (service, repo, pool)
    }

    async fn insert_entry(pool: &SqlitePool, text: &str) -> i64 {
        let message = IncomingMessage {
            source: MessageSource::Telegram,
            source_conversation_id: "42".to_string(),
            source_message_id: "100".to_string(),
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
}
