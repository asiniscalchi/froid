use std::{error::Error, fmt};

use tracing::warn;

use super::{
    JournalEntryExtractionCandidate,
    repository::{JournalEntryExtractionRepository, JournalEntryExtractionRepositoryError},
    service::{JournalEntryExtractionRunner, JournalEntryExtractionServiceError},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractionBackfillResult {
    pub attempted: u32,
    pub errored: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtractionBackfillError {
    Repository(JournalEntryExtractionRepositoryError),
}

impl fmt::Display for ExtractionBackfillError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Repository(error) => write!(f, "{error}"),
        }
    }
}

impl Error for ExtractionBackfillError {}

impl From<JournalEntryExtractionRepositoryError> for ExtractionBackfillError {
    fn from(error: JournalEntryExtractionRepositoryError) -> Self {
        Self::Repository(error)
    }
}

#[derive(Debug, Clone)]
pub struct ExtractionBackfillService<R> {
    repository: JournalEntryExtractionRepository,
    runner: R,
}

impl<R> ExtractionBackfillService<R>
where
    R: JournalEntryExtractionRunner,
{
    pub fn new(repository: JournalEntryExtractionRepository, runner: R) -> Self {
        Self { repository, runner }
    }

    pub fn model(&self) -> &str {
        self.runner.model()
    }

    pub fn prompt_version(&self) -> &str {
        self.runner.prompt_version()
    }

    pub async fn backfill_missing_or_failed_extractions(
        &self,
        limit: u32,
    ) -> Result<ExtractionBackfillResult, ExtractionBackfillError> {
        let candidates = self
            .repository
            .find_entries_missing_or_failed_extraction(limit)
            .await?;

        let mut result = ExtractionBackfillResult {
            attempted: candidates.len() as u32,
            errored: 0,
        };

        for candidate in candidates {
            if let Err(error) = self.process_candidate(&candidate).await {
                result.errored += 1;
                warn!(
                    journal_entry_id = candidate.journal_entry_id,
                    error = %error,
                    "extraction reconciliation candidate failed"
                );
            }
        }

        Ok(result)
    }

    async fn process_candidate(
        &self,
        candidate: &JournalEntryExtractionCandidate,
    ) -> Result<(), ProcessCandidateError> {
        self.repository
            .delete_failed_if_present(candidate.journal_entry_id)
            .await
            .map_err(ProcessCandidateError::Repository)?;

        self.runner
            .extract_entry(candidate.journal_entry_id, &candidate.raw_text)
            .await
            .map_err(ProcessCandidateError::Extraction)?;

        Ok(())
    }
}

#[derive(Debug)]
enum ProcessCandidateError {
    Repository(JournalEntryExtractionRepositoryError),
    Extraction(JournalEntryExtractionServiceError),
}

impl fmt::Display for ProcessCandidateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Repository(error) => write!(f, "repository error: {error}"),
            Self::Extraction(error) => write!(f, "extraction error: {error}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use chrono::TimeZone;
    use sqlx::SqlitePool;

    use super::*;
    use crate::{
        database,
        journal::{
            extraction::{
                repository::JournalEntryExtractionRepository,
                service::{JournalEntryExtractionRunner, JournalEntryExtractionServiceError},
            },
            repository::JournalRepository,
        },
        messages::{IncomingMessage, MessageSource},
    };

    async fn setup() -> (JournalRepository, JournalEntryExtractionRepository) {
        database::register_sqlite_vec_extension();
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        (
            JournalRepository::new(pool.clone()),
            JournalEntryExtractionRepository::new(pool),
        )
    }

    async fn store_entry(
        journal_repo: &JournalRepository,
        source_message_id: &str,
        text: &str,
        h: u32,
    ) -> i64 {
        let received_at = chrono::Utc.with_ymd_and_hms(2026, 1, 1, h, 0, 0).unwrap();
        let message = IncomingMessage {
            source: MessageSource::Telegram,
            source_conversation_id: "42".to_string(),
            source_message_id: source_message_id.to_string(),
            user_id: "7".to_string(),
            text: text.to_string(),
            received_at,
        };
        journal_repo.store(&message).await.unwrap().unwrap()
    }

    fn valid_json() -> &'static str {
        r#"{"summary":"ok","domains":[],"emotions":[],"behaviors":[],"needs":[],"possible_patterns":[]}"#
    }

    #[derive(Clone)]
    struct FakeRunner {
        result: Result<(), String>,
        calls: Arc<Mutex<Vec<(i64, String)>>>,
    }

    impl FakeRunner {
        fn succeeding() -> Self {
            Self {
                result: Ok(()),
                calls: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn failing(message: &str) -> Self {
            Self {
                result: Err(message.to_string()),
                calls: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn calls(&self) -> Vec<(i64, String)> {
            self.calls.lock().unwrap().clone()
        }

        fn call_count(&self) -> usize {
            self.calls.lock().unwrap().len()
        }
    }

    #[async_trait]
    impl JournalEntryExtractionRunner for FakeRunner {
        fn model(&self) -> &str {
            "test-extraction-model"
        }

        fn prompt_version(&self) -> &str {
            "entry_extraction_v1"
        }

        async fn extract_entry(
            &self,
            journal_entry_id: i64,
            text: &str,
        ) -> Result<(), JournalEntryExtractionServiceError> {
            self.calls
                .lock()
                .unwrap()
                .push((journal_entry_id, text.to_string()));
            self.result.clone().map_err(|msg| {
                JournalEntryExtractionServiceError::Repository(
                    crate::journal::extraction::repository::JournalEntryExtractionRepositoryError::Storage(msg),
                )
            })
        }
    }

    fn backfill_service(
        extraction_repo: JournalEntryExtractionRepository,
        runner: FakeRunner,
    ) -> ExtractionBackfillService<FakeRunner> {
        ExtractionBackfillService::new(extraction_repo, runner)
    }

    #[tokio::test]
    async fn returns_zero_when_no_candidates() {
        let (_, extraction_repo) = setup().await;
        let service = backfill_service(extraction_repo, FakeRunner::succeeding());

        let result = service
            .backfill_missing_or_failed_extractions(10)
            .await
            .unwrap();

        assert_eq!(
            result,
            ExtractionBackfillResult {
                attempted: 0,
                errored: 0,
            }
        );
    }

    #[tokio::test]
    async fn processes_entries_with_no_extraction_record() {
        let (journal_repo, extraction_repo) = setup().await;
        let runner = FakeRunner::succeeding();
        store_entry(&journal_repo, "1", "first entry", 10).await;
        store_entry(&journal_repo, "2", "second entry", 11).await;
        let service = backfill_service(extraction_repo, runner.clone());

        let result = service
            .backfill_missing_or_failed_extractions(10)
            .await
            .unwrap();

        assert_eq!(result.attempted, 2);
        assert_eq!(result.errored, 0);
        assert_eq!(runner.call_count(), 2);
    }

    #[tokio::test]
    async fn retries_failed_entries_by_deleting_failed_row_first() {
        let (journal_repo, extraction_repo) = setup().await;
        let runner = FakeRunner::succeeding();
        let entry_id = store_entry(&journal_repo, "1", "my note", 10).await;
        extraction_repo
            .insert_pending_if_absent(entry_id, "model-a", "v1")
            .await
            .unwrap();
        extraction_repo
            .mark_failed(entry_id, "model-a", "v1", "provider down")
            .await
            .unwrap();
        let service = backfill_service(extraction_repo.clone(), runner.clone());

        let result = service
            .backfill_missing_or_failed_extractions(10)
            .await
            .unwrap();

        assert_eq!(result.attempted, 1);
        assert_eq!(result.errored, 0);
        assert_eq!(runner.calls(), vec![(entry_id, "my note".to_string())]);
        assert!(
            extraction_repo
                .find_by_journal_entry_id(entry_id)
                .await
                .unwrap()
                .is_none(),
            "failed row should be deleted before runner is called"
        );
    }

    #[tokio::test]
    async fn skips_pending_and_completed_entries() {
        let (journal_repo, extraction_repo) = setup().await;
        let runner = FakeRunner::succeeding();
        let pending_id = store_entry(&journal_repo, "1", "pending", 10).await;
        let completed_id = store_entry(&journal_repo, "2", "completed", 11).await;
        extraction_repo
            .insert_pending_if_absent(pending_id, "model-a", "v1")
            .await
            .unwrap();
        extraction_repo
            .insert_pending_if_absent(completed_id, "model-a", "v1")
            .await
            .unwrap();
        extraction_repo
            .mark_completed(completed_id, valid_json(), "model-a", "v1")
            .await
            .unwrap();
        let service = backfill_service(extraction_repo, runner.clone());

        let result = service
            .backfill_missing_or_failed_extractions(10)
            .await
            .unwrap();

        assert_eq!(result.attempted, 0);
        assert_eq!(runner.call_count(), 0);
    }

    #[tokio::test]
    async fn respects_limit() {
        let (journal_repo, extraction_repo) = setup().await;
        let runner = FakeRunner::succeeding();
        store_entry(&journal_repo, "1", "first", 10).await;
        store_entry(&journal_repo, "2", "second", 11).await;
        store_entry(&journal_repo, "3", "third", 12).await;
        let service = backfill_service(extraction_repo, runner.clone());

        let result = service
            .backfill_missing_or_failed_extractions(2)
            .await
            .unwrap();

        assert_eq!(result.attempted, 2);
        assert_eq!(runner.call_count(), 2);
    }

    #[tokio::test]
    async fn continues_after_runner_error_and_counts_errored() {
        let (journal_repo, extraction_repo) = setup().await;
        let runner = FakeRunner::failing("provider down");
        store_entry(&journal_repo, "1", "first", 10).await;
        store_entry(&journal_repo, "2", "second", 11).await;
        let service = backfill_service(extraction_repo, runner.clone());

        let result = service
            .backfill_missing_or_failed_extractions(10)
            .await
            .unwrap();

        assert_eq!(result.attempted, 2);
        assert_eq!(result.errored, 2);
        assert_eq!(runner.call_count(), 2);
    }

    #[tokio::test]
    async fn processes_oldest_entries_first() {
        let (journal_repo, extraction_repo) = setup().await;
        let runner = FakeRunner::succeeding();
        let first = store_entry(&journal_repo, "1", "first", 10).await;
        let second = store_entry(&journal_repo, "2", "second", 11).await;
        let third = store_entry(&journal_repo, "3", "third", 12).await;
        let service = backfill_service(extraction_repo, runner.clone());

        service
            .backfill_missing_or_failed_extractions(10)
            .await
            .unwrap();

        let call_ids: Vec<i64> = runner.calls().into_iter().map(|(id, _)| id).collect();
        assert_eq!(call_ids, vec![first, second, third]);
    }

    #[tokio::test]
    async fn repeated_backfill_does_not_reprocess_completed_entries() {
        let (journal_repo, extraction_repo) = setup().await;

        // Simulate what the real runner does: insert pending, then mark completed
        let entry_id = store_entry(&journal_repo, "1", "first", 10).await;
        extraction_repo
            .insert_pending_if_absent(entry_id, "model-a", "v1")
            .await
            .unwrap();
        extraction_repo
            .mark_completed(entry_id, valid_json(), "model-a", "v1")
            .await
            .unwrap();

        let runner = FakeRunner::succeeding();
        let service = backfill_service(extraction_repo, runner.clone());

        let first_result = service
            .backfill_missing_or_failed_extractions(10)
            .await
            .unwrap();
        let second_result = service
            .backfill_missing_or_failed_extractions(10)
            .await
            .unwrap();

        assert_eq!(first_result.attempted, 0);
        assert_eq!(second_result.attempted, 0);
        assert_eq!(runner.call_count(), 0);
    }
}
