use std::{error::Error, fmt};
use tracing::{info, warn};

use async_trait::async_trait;

use crate::journal::extraction::{
    JournalEntryExtractionGenerator,
    repository::{JournalEntryExtractionRepository, JournalEntryExtractionRepositoryError},
    validation::validate_extraction_json,
};

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
    fn model(&self) -> &str;
    fn prompt_version(&self) -> &str;

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
    fn model(&self) -> &str {
        self.generator.model()
    }

    fn prompt_version(&self) -> &str {
        self.generator.prompt_version()
    }

    async fn extract_entry(
        &self,
        journal_entry_id: i64,
        text: &str,
    ) -> Result<(), JournalEntryExtractionServiceError> {
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
            Ok(result) => {
                let raw_json = match serde_json::to_string(&result) {
                    Ok(json) => json,
                    Err(error) => {
                        warn!(
                            journal_entry_id,
                            error = %error,
                            "failed to serialize extraction result"
                        );
                        self.record_failure(journal_entry_id, error).await?;
                        return Ok(());
                    }
                };

                match validate_extraction_json(&raw_json) {
                    Ok(valid_json) => {
                        self.repository
                            .mark_completed(
                                journal_entry_id,
                                &valid_json,
                                self.generator.model(),
                                self.generator.prompt_version(),
                            )
                            .await?;
                        info!(
                            journal_entry_id,
                            model = self.generator.model(),
                            "extraction completed successfully"
                        );
                    }
                    Err(error) => {
                        warn!(
                            journal_entry_id,
                            error = %error,
                            "extraction validation failed"
                        );
                        self.record_failure(journal_entry_id, error).await?;
                    }
                }
            }
            Err(error) => {
                warn!(
                    journal_entry_id,
                    error = %error,
                    "extraction generation failed"
                );
                self.record_failure(journal_entry_id, error).await?;
            }
        }

        Ok(())
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
            JournalEntryExtractionGenerationError, JournalEntryExtractionResult,
            JournalEntryExtractionStatus, repository::JournalEntryExtractionRepository,
        },
        messages::{IncomingMessage, MessageSource},
    };

    #[derive(Clone)]
    struct FakeGenerator {
        result: Result<JournalEntryExtractionResult, JournalEntryExtractionGenerationError>,
        calls: Arc<AtomicUsize>,
        notes: Arc<Mutex<Vec<String>>>,
    }

    impl FakeGenerator {
        fn succeeding(result: JournalEntryExtractionResult) -> Self {
            Self {
                result: Ok(result),
                calls: Arc::new(AtomicUsize::new(0)),
                notes: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn failing(message: &str) -> Self {
            Self {
                result: Err(JournalEntryExtractionGenerationError::new(message)),
                calls: Arc::new(AtomicUsize::new(0)),
                notes: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
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
        ) -> Result<JournalEntryExtractionResult, JournalEntryExtractionGenerationError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.notes.lock().unwrap().push(note.to_string());
            self.result.clone()
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

    fn valid_result() -> JournalEntryExtractionResult {
        JournalEntryExtractionResult {
            summary: "The note mentions work stress.".to_string(),
            domains: vec!["work".to_string()],
            emotions: vec![crate::journal::extraction::types::EmotionExtraction {
                label: "anxiety".to_string(),
                intensity: 0.6,
                confidence: 0.7,
            }],
            behaviors: vec![crate::journal::extraction::types::BehaviorExtraction {
                label: "overthinking".to_string(),
                valence: crate::journal::extraction::types::BehaviorValence::Negative,
                confidence: 0.6,
            }],
            needs: vec![crate::journal::extraction::types::NeedExtraction {
                label: "control".to_string(),
                status: crate::journal::extraction::types::NeedStatus::Activated,
                confidence: 0.5,
            }],
            possible_patterns: vec![crate::journal::extraction::types::PatternExtraction {
                description: "Work uncertainty may be associated with overthinking in this note."
                    .to_string(),
                confidence: 0.4,
            }],
        }
    }

    #[tokio::test]
    async fn stores_completed_extraction_when_generation_succeeds() {
        let (service, repo, pool) = setup(FakeGenerator::succeeding(valid_result())).await;
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
    async fn does_not_generate_duplicate_extraction_for_existing_entry() {
        let generator = FakeGenerator::succeeding(valid_result());
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
