use std::sync::{Arc, Mutex};

use chrono::NaiveDate;
use chrono::{TimeZone, Utc};
use sqlx::SqlitePool;

use super::*;
use crate::{
    database,
    journal::{
        command::{DEFAULT_RECENT_LIMIT, JournalCommand, JournalCommandRequest, MAX_RECENT_LIMIT},
        embedding::{
            EmbedderError, Embedding, SUPPORTED_EMBEDDING_DIMENSIONS, SqliteEmbeddingRepository,
        },
        extraction::repository::JournalEntryExtractionRepository,
        extraction::service::JournalEntryExtractionServiceError,
        repository::JournalRepository,
        review::{
            DailyReview, DailyReviewResult, DailyReviewStatus,
            generator::fake::FakeReviewGenerator,
            repository::DailyReviewRepository,
            service::{DailyReviewRunner, DailyReviewService, DailyReviewServiceError},
        },
        search::SemanticSearchService,
        week_review::{
            WeeklyReview, WeeklyReviewStatus,
            service::{WeeklyReviewResult, WeeklyReviewRunner, WeeklyReviewServiceError},
        },
    },
    messages::MessageSource,
};

async fn setup() -> JournalService {
    database::register_sqlite_vec_extension();
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::migrate!().run(&pool).await.unwrap();
    JournalService::new(JournalRepository::new(pool))
}

async fn wait_until<F, Fut, T>(mut f: F) -> Option<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Option<T>>,
{
    for _ in 0..100 {
        if let Some(res) = f().await {
            return Some(res);
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    None
}

async fn setup_with_pool() -> (JournalService, SqlitePool) {
    database::register_sqlite_vec_extension();
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::migrate!().run(&pool).await.unwrap();
    (
        JournalService::new(JournalRepository::new(pool.clone())),
        pool,
    )
}

async fn setup_with_daily_review_runner<R>(runner: R) -> JournalService
where
    R: DailyReviewRunner + 'static,
{
    database::register_sqlite_vec_extension();
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::migrate!().run(&pool).await.unwrap();
    JournalService::new(JournalRepository::new(pool)).with_daily_review_runner(runner)
}

async fn setup_with_entry_extraction_runner<R>(runner: R) -> (JournalService, JournalRepository)
where
    R: JournalEntryExtractionRunner + 'static,
{
    database::register_sqlite_vec_extension();
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::migrate!().run(&pool).await.unwrap();
    let repo = JournalRepository::new(pool);
    (
        JournalService::new(repo.clone()).with_entry_extraction_runner(runner),
        repo,
    )
}

async fn setup_with_daily_review_service(
    generator: FakeReviewGenerator,
) -> (JournalService, DailyReviewRepository, JournalRepository) {
    database::register_sqlite_vec_extension();
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::migrate!().run(&pool).await.unwrap();
    let journal_repo = JournalRepository::new(pool.clone());
    let daily_review_repo = DailyReviewRepository::new(pool.clone());
    let extractions = JournalEntryExtractionRepository::new(pool.clone());
    let daily_review_service = DailyReviewService::new(
        daily_review_repo.clone(),
        JournalRepository::new(pool),
        extractions,
        generator,
    );
    let service =
        JournalService::new(journal_repo.clone()).with_daily_review_runner(daily_review_service);

    (service, daily_review_repo, journal_repo)
}

async fn setup_with_search(
    embedder: FakeEmbedder,
) -> (JournalService, SqliteEmbeddingRepository, JournalRepository) {
    database::register_sqlite_vec_extension();
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::migrate!().run(&pool).await.unwrap();
    let repo = JournalRepository::new(pool.clone());
    let index = SqliteEmbeddingRepository::new(pool.clone());
    let search_repo = JournalRepository::new(pool.clone());
    let search = SemanticSearchService::new(index.clone(), embedder, search_repo);
    let service = JournalService::new(repo.clone()).with_search(search);
    (service, index, repo)
}

async fn setup_with_capture_embedding(
    embedder: FakeEmbedder,
) -> (JournalService, SqliteEmbeddingRepository, JournalRepository) {
    database::register_sqlite_vec_extension();
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::migrate!().run(&pool).await.unwrap();
    let repo = JournalRepository::new(pool.clone());
    let index = SqliteEmbeddingRepository::new(pool.clone());
    let service = JournalService::new(repo.clone()).with_capture_embedding(index.clone(), embedder);
    (service, index, repo)
}

async fn setup_with_search_and_capture_embedding(
    search_embedder: FakeEmbedder,
    capture_embedder: FakeEmbedder,
) -> (JournalService, SqliteEmbeddingRepository) {
    database::register_sqlite_vec_extension();
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::migrate!().run(&pool).await.unwrap();
    let repo = JournalRepository::new(pool.clone());
    let search_index = SqliteEmbeddingRepository::new(pool.clone());
    let capture_index = SqliteEmbeddingRepository::new(pool.clone());
    let search = SemanticSearchService::new(
        search_index,
        search_embedder,
        JournalRepository::new(pool.clone()),
    );
    let service = JournalService::new(repo)
        .with_search(search)
        .with_capture_embedding(capture_index.clone(), capture_embedder);
    (service, capture_index)
}

const TEST_MODEL: &str = "test-model";

#[derive(Debug, Clone)]
struct FakeDailyReviewRunner {
    fetch_result: Result<Option<DailyReview>, DailyReviewServiceError>,
    calls: Arc<Mutex<Vec<(String, NaiveDate)>>>,
}

impl FakeDailyReviewRunner {
    fn new() -> Self {
        Self::with_fetch_result(Ok(None))
    }

    fn with_fetch_result(
        fetch_result: Result<Option<DailyReview>, DailyReviewServiceError>,
    ) -> Self {
        Self {
            fetch_result,
            calls: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn calls(&self) -> Vec<(String, NaiveDate)> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl DailyReviewRunner for FakeDailyReviewRunner {
    async fn review_day(
        &self,
        _user_id: &str,
        _utc_date: NaiveDate,
    ) -> Result<DailyReviewResult, DailyReviewServiceError> {
        Ok(DailyReviewResult::EmptyDay)
    }

    async fn fetch_review(
        &self,
        user_id: &str,
        utc_date: NaiveDate,
    ) -> Result<Option<DailyReview>, DailyReviewServiceError> {
        self.calls
            .lock()
            .unwrap()
            .push((user_id.to_string(), utc_date));
        self.fetch_result.clone()
    }
}

#[derive(Clone)]
struct FakeEmbedder {
    result: Result<Embedding, EmbedderError>,
}

impl FakeEmbedder {
    fn succeeds() -> Self {
        Self {
            result: Ok(Embedding::new(
                vec![1.0; SUPPORTED_EMBEDDING_DIMENSIONS],
                SUPPORTED_EMBEDDING_DIMENSIONS,
            )
            .unwrap()),
        }
    }

    fn fails() -> Self {
        Self {
            result: Err(EmbedderError::Provider("provider down".to_string())),
        }
    }
}

#[async_trait::async_trait]
impl Embedder for FakeEmbedder {
    fn model(&self) -> &str {
        TEST_MODEL
    }

    fn dimensions(&self) -> usize {
        SUPPORTED_EMBEDDING_DIMENSIONS
    }

    async fn embed(&self, _text: &str) -> Result<Embedding, EmbedderError> {
        self.result.clone()
    }
}

#[derive(Clone)]
struct FakeJournalEntryExtractionRunner {
    calls: Arc<Mutex<Vec<(i64, String)>>>,
}

impl FakeJournalEntryExtractionRunner {
    fn new() -> Self {
        Self {
            calls: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn calls(&self) -> Vec<(i64, String)> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl JournalEntryExtractionRunner for FakeJournalEntryExtractionRunner {
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
        Ok(())
    }
}

#[derive(Clone)]
struct FailingPendingEmbeddingCounter;

#[async_trait::async_trait]
impl PendingEmbeddingCounter for FailingPendingEmbeddingCounter {
    async fn count_entries_missing_embedding_for_user(
        &self,
        _user_id: &str,
        _embedding_model: &str,
    ) -> Result<i64, EmbeddingRepositoryError> {
        Err(EmbeddingRepositoryError::Database(
            "database path /tmp/secret.sqlite unavailable".to_string(),
        ))
    }
}

fn incoming(
    source_message_id: &str,
    text: &str,
    received_at: chrono::DateTime<Utc>,
) -> IncomingMessage {
    incoming_for_conversation("42", source_message_id, text, received_at)
}

fn incoming_for_conversation(
    source_conversation_id: &str,
    source_message_id: &str,
    text: &str,
    received_at: chrono::DateTime<Utc>,
) -> IncomingMessage {
    IncomingMessage {
        source: MessageSource::Telegram,
        source_conversation_id: source_conversation_id.to_string(),
        source_message_id: source_message_id.to_string(),
        user_id: "7".to_string(),
        text: text.to_string(),
        received_at,
    }
}

fn at(h: u32, m: u32) -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 4, 28, h, m, 0).unwrap()
}

fn command(command: JournalCommand) -> JournalCommandRequest {
    JournalCommandRequest {
        source: MessageSource::Telegram,
        source_conversation_id: "42".to_string(),
        user_id: "7".to_string(),
        received_at: at(12, 0),
        command,
    }
}

fn daily_review(review_text: &str) -> DailyReview {
    DailyReview {
        id: 1,
        user_id: "7".to_string(),
        review_date: date(),
        review_text: Some(review_text.to_string()),
        model: "test-model".to_string(),
        prompt_version: "v1".to_string(),
        status: DailyReviewStatus::Completed,
        error_message: None,
        delivered_at: None,
        delivery_error: None,
        signals_status: None,
        signals_error: None,
        signals_model: None,
        signals_prompt_version: None,
        signals_updated_at: None,
        created_at: at(10, 0),
        updated_at: at(10, 0),
    }
}

fn date() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 4, 28).unwrap()
}

#[tokio::test]
async fn returns_confirmation_after_storing() {
    let service = setup().await;
    let message = incoming("100", "hello froid", Utc::now());

    let outgoing = service.process(&message).await.unwrap();

    assert_eq!(outgoing.text, "Message saved.");
}

#[tokio::test]
async fn returns_confirmation_for_duplicate_without_error() {
    let service = setup().await;
    let message = incoming("100", "hello froid", Utc::now());

    service.process(&message).await.unwrap();
    let outgoing = service.process(&message).await.unwrap();

    assert_eq!(outgoing.text, "Message saved.");
}

#[tokio::test]
async fn process_embeds_new_entry_when_capture_embedding_is_configured() {
    let (service, index, repo) = setup_with_capture_embedding(FakeEmbedder::succeeds()).await;
    let message = incoming("100", "hello froid", at(10, 0));

    let outgoing = service.process(&message).await.unwrap();
    let entry_id: i64 =
        sqlx::query_scalar("SELECT id FROM journal_entries WHERE source_message_id = '100'")
            .fetch_one(repo.pool())
            .await
            .unwrap();

    assert_eq!(outgoing.text, "Message saved.");
    assert!(
        wait_until(|| async {
            if index
                .has_embedding(entry_id, TEST_MODEL)
                .await
                .unwrap_or(false)
            {
                Some(())
            } else {
                None
            }
        })
        .await
        .is_some()
    );
}

#[tokio::test]
async fn process_still_saves_message_when_capture_embedding_fails() {
    let (service, index, repo) = setup_with_capture_embedding(FakeEmbedder::fails()).await;
    let message = incoming("100", "hello froid", at(10, 0));

    let outgoing = service.process(&message).await.unwrap();
    let entry_id: i64 =
        sqlx::query_scalar("SELECT id FROM journal_entries WHERE source_message_id = '100'")
            .fetch_one(repo.pool())
            .await
            .unwrap();

    assert_eq!(outgoing.text, "Message saved.");
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    assert!(!index.has_embedding(entry_id, TEST_MODEL).await.unwrap());
}

#[tokio::test]
async fn process_runs_entry_extraction_without_exposing_content_to_user() {
    let runner = FakeJournalEntryExtractionRunner::new();
    let (service, repo) = setup_with_entry_extraction_runner(runner.clone()).await;
    let message = incoming("100", "private structured meaning source", at(10, 0));

    let outgoing = service.process(&message).await.unwrap();
    let entry_id: i64 =
        sqlx::query_scalar("SELECT id FROM journal_entries WHERE source_message_id = '100'")
            .fetch_one(repo.pool())
            .await
            .unwrap();

    assert_eq!(outgoing.text, "Message saved.");
    assert!(
        wait_until(|| async {
            if !runner.calls().is_empty() {
                Some(())
            } else {
                None
            }
        })
        .await
        .is_some()
    );
    assert_eq!(
        runner.calls(),
        vec![(entry_id, "private structured meaning source".to_string())]
    );
    assert!(!outgoing.text.contains("structured meaning"));
}

#[tokio::test]
async fn command_search_can_find_entry_embedded_during_capture() {
    let (service, _) =
        setup_with_search_and_capture_embedding(FakeEmbedder::succeeds(), FakeEmbedder::succeeds())
            .await;

    service
        .process(&incoming("100", "journal entry text", at(10, 0)))
        .await
        .unwrap();

    let search_result = wait_until(|| async {
        let outgoing = service
            .command(&command(JournalCommand::Search {
                query: "query".to_string(),
            }))
            .await
            .unwrap();
        if outgoing.text.contains("journal entry text") {
            Some(outgoing.text)
        } else {
            None
        }
    })
    .await
    .expect("Search did not find entry in time");

    assert!(search_result.contains("Search results for: query"));
    assert!(search_result.contains("journal entry text"));
}

#[tokio::test]
async fn command_start_returns_welcome_message() {
    let service = setup().await;

    let outgoing = service
        .command(&command(JournalCommand::Start))
        .await
        .unwrap();

    assert!(outgoing.text.contains("Froid is running."));
    assert!(outgoing.text.contains("/recent [number]"));
}

#[tokio::test]
async fn command_help_returns_available_commands() {
    let service = setup().await;

    let outgoing = service
        .command(&command(JournalCommand::Help))
        .await
        .unwrap();

    assert!(outgoing.text.contains("/recent [number]"));
    assert!(outgoing.text.contains("/today"));
    assert!(
        outgoing
            .text
            .contains("/review [today|YYYY-MM-DD|-N] - show daily review")
    );
    assert!(outgoing.text.contains("/stats"));
    assert!(outgoing.text.contains("/status"));
}

#[tokio::test]
async fn status_returns_stable_sections_when_optional_subsystems_are_unavailable() {
    let service = setup().await;

    let outgoing = service
        .command(&command(JournalCommand::Status))
        .await
        .unwrap();

    assert_eq!(
        outgoing.text,
        "Froid status\n\nJournal:\n- Total entries: 0\n- Entries today: 0\n\nEmbeddings:\n- Semantic search: unavailable\n- Model: unavailable\n- Dimensions: unavailable\n- Pending embeddings: unavailable\n\nDaily review:\n- Generation: not configured\n- Delivery: not configured"
    );
}

#[tokio::test]
async fn status_uses_user_scoped_journal_stats_and_command_received_at_date() {
    let (service, pool) = setup_with_pool().await;
    service
        .process(&incoming(
            "1",
            "previous day",
            Utc.with_ymd_and_hms(2026, 4, 28, 23, 59, 0).unwrap(),
        ))
        .await
        .unwrap();
    service
        .process(&incoming(
            "2",
            "requested day",
            Utc.with_ymd_and_hms(2026, 4, 29, 0, 0, 0).unwrap(),
        ))
        .await
        .unwrap();
    JournalRepository::new(pool.clone())
        .store(&IncomingMessage {
            source: MessageSource::Telegram,
            source_conversation_id: "42".to_string(),
            source_message_id: "3".to_string(),
            user_id: "8".to_string(),
            text: "other user".to_string(),
            received_at: Utc.with_ymd_and_hms(2026, 4, 29, 9, 0, 0).unwrap(),
        })
        .await
        .unwrap();

    let outgoing = service
        .command(&JournalCommandRequest {
            source: MessageSource::Telegram,
            source_conversation_id: "42".to_string(),
            user_id: "7".to_string(),
            received_at: Utc.with_ymd_and_hms(2026, 4, 29, 12, 0, 0).unwrap(),
            command: JournalCommand::Status,
        })
        .await
        .unwrap();

    assert!(outgoing.text.contains("- Total entries: 2"));
    assert!(outgoing.text.contains("- Entries today: 1"));
}

#[tokio::test]
async fn status_command_does_not_store_command_text_as_journal_entry() {
    let (service, pool) = setup_with_pool().await;

    service
        .command(&command(JournalCommand::Status))
        .await
        .unwrap();

    let entry_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM journal_entries")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(entry_count, 0);
}

#[tokio::test]
async fn status_reports_configured_embedding_status_and_user_scoped_pending_count() {
    let (service, index, repo) = setup_with_search(FakeEmbedder::fails()).await;
    let service = service
        .with_embedding_status_config(EmbeddingStatusConfig {
            model: TEST_MODEL.to_string(),
            dimensions: SUPPORTED_EMBEDDING_DIMENSIONS,
        })
        .with_pending_embedding_counter(index.clone());
    repo.store(&incoming("1", "embedded entry", at(10, 0)))
        .await
        .unwrap();
    repo.store(&incoming("2", "pending entry", at(11, 0)))
        .await
        .unwrap();
    let embedded_entry_id: i64 =
        sqlx::query_scalar("SELECT id FROM journal_entries WHERE source_message_id = '1'")
            .fetch_one(repo.pool())
            .await
            .unwrap();
    index
        .store_embedding(
            embedded_entry_id,
            TEST_MODEL,
            SUPPORTED_EMBEDDING_DIMENSIONS,
            &Embedding::new(
                vec![1.0; SUPPORTED_EMBEDDING_DIMENSIONS],
                SUPPORTED_EMBEDDING_DIMENSIONS,
            )
            .unwrap(),
        )
        .await
        .unwrap();
    repo.store(&IncomingMessage {
        source: MessageSource::Telegram,
        source_conversation_id: "42".to_string(),
        source_message_id: "3".to_string(),
        user_id: "8".to_string(),
        text: "other user pending entry".to_string(),
        received_at: at(12, 0),
    })
    .await
    .unwrap();

    let outgoing = service
        .command(&command(JournalCommand::Status))
        .await
        .unwrap();

    assert!(outgoing.text.contains("- Semantic search: enabled"));
    assert!(outgoing.text.contains("- Model: test-model"));
    assert!(outgoing.text.contains("- Dimensions: 1536"));
    assert!(outgoing.text.contains("- Pending embeddings: 1"));
}

#[tokio::test]
async fn status_reports_pending_embeddings_unavailable_when_counter_fails() {
    let (service, _, _) = setup_with_search(FakeEmbedder::fails()).await;
    let service = service
        .with_embedding_status_config(EmbeddingStatusConfig {
            model: TEST_MODEL.to_string(),
            dimensions: SUPPORTED_EMBEDDING_DIMENSIONS,
        })
        .with_pending_embedding_counter(FailingPendingEmbeddingCounter);

    let outgoing = service
        .command(&command(JournalCommand::Status))
        .await
        .unwrap();

    assert!(outgoing.text.contains("- Semantic search: enabled"));
    assert!(outgoing.text.contains("- Pending embeddings: unavailable"));
    assert!(!outgoing.text.contains("/tmp/secret.sqlite"));
    assert!(!outgoing.text.contains("database path"));
}

#[tokio::test]
async fn status_reports_daily_review_prompt_when_configured() {
    let runner = FakeDailyReviewRunner::new();
    let service = setup_with_daily_review_runner(runner)
        .await
        .with_daily_review_prompt_version("daily-review-v1");

    let outgoing = service
        .command(&command(JournalCommand::Status))
        .await
        .unwrap();

    assert!(outgoing.text.contains("- Generation: configured"));
    assert!(outgoing.text.contains("- Prompt: daily-review-v1"));
    assert!(outgoing.text.contains("- Delivery: not configured"));
}

#[tokio::test]
async fn status_reports_delivery_configured_when_delivery_is_wired() {
    let runner = FakeDailyReviewRunner::new();
    let service = setup_with_daily_review_runner(runner)
        .await
        .with_daily_review_delivery_configured();

    let outgoing = service
        .command(&command(JournalCommand::Status))
        .await
        .unwrap();

    assert!(outgoing.text.contains("- Delivery: configured"));
}

#[tokio::test]
async fn status_does_not_expose_secrets_or_raw_internal_errors() {
    let (service, _, _) = setup_with_search(FakeEmbedder::fails()).await;
    let service = service
        .with_embedding_status_config(EmbeddingStatusConfig {
            model: TEST_MODEL.to_string(),
            dimensions: SUPPORTED_EMBEDDING_DIMENSIONS,
        })
        .with_pending_embedding_counter(FailingPendingEmbeddingCounter);

    let outgoing = service
        .command(&command(JournalCommand::Status))
        .await
        .unwrap();

    for forbidden in [
        "OPENAI_API_KEY",
        "TELEGRAM_BOT_TOKEN",
        "bot token",
        "sqlite:",
        "/tmp/secret.sqlite",
        "provider down",
        "database path",
        "stack trace",
    ] {
        assert!(!outgoing.text.contains(forbidden), "{forbidden}");
    }
}

#[tokio::test]
async fn review_usage_returns_usage_message() {
    let service = setup().await;

    let outgoing = service
        .command(&command(JournalCommand::ReviewUsage))
        .await
        .unwrap();

    assert_eq!(
        outgoing.text,
        "Usage: /review [today|YYYY-MM-DD|-N]\n\nExamples:\n/review\n/review today\n/review 2026-04-29\n/review -1\n/review -7"
    );
}

#[tokio::test]
async fn review_today_returns_unavailable_when_runner_is_not_configured() {
    let (service, pool) = setup_with_pool().await;

    let outgoing = service
        .command(&command(JournalCommand::ReviewToday))
        .await
        .unwrap();
    let review_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM daily_reviews")
        .fetch_one(&pool)
        .await
        .unwrap();

    assert_eq!(
        outgoing.text,
        "Daily review generation is not configured yet."
    );
    assert_eq!(review_count, 0);
}

#[tokio::test]
async fn review_today_returns_existing_review() {
    let runner = FakeDailyReviewRunner::with_fetch_result(Ok(Some(daily_review("stored review"))));
    let service = setup_with_daily_review_runner(runner.clone()).await;

    let outgoing = service
        .command(&command(JournalCommand::ReviewToday))
        .await
        .unwrap();

    assert_eq!(outgoing.text, "Today's review\n\nstored review");
    assert_eq!(runner.calls(), vec![("7".to_string(), date())]);
}

#[tokio::test]
async fn review_today_returns_not_available_when_no_review_exists() {
    let runner = FakeDailyReviewRunner::with_fetch_result(Ok(None));
    let service = setup_with_daily_review_runner(runner).await;

    let outgoing = service
        .command(&command(JournalCommand::ReviewToday))
        .await
        .unwrap();

    assert_eq!(outgoing.text, "No review available for today yet.");
}

#[tokio::test]
async fn review_today_returns_not_available_on_fetch_error() {
    let runner = FakeDailyReviewRunner::with_fetch_result(Err(DailyReviewServiceError::Storage(
        "database unavailable".to_string(),
    )));
    let service = setup_with_daily_review_runner(runner).await;

    let outgoing = service
        .command(&command(JournalCommand::ReviewToday))
        .await
        .unwrap();

    assert_eq!(outgoing.text, "No review available for today yet.");
}

#[tokio::test]
async fn review_today_command_does_not_store_command_text_as_journal_entry() {
    let (service, pool) = setup_with_pool().await;

    service
        .command(&command(JournalCommand::ReviewToday))
        .await
        .unwrap();

    let entry_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM journal_entries")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(entry_count, 0);
}

#[tokio::test]
async fn undo_deletes_daily_review_for_deleted_entry_date() {
    let (service, daily_reviews, journal_entries) =
        setup_with_daily_review_service(FakeReviewGenerator::succeeding("any")).await;
    journal_entries
        .store(&incoming("1", "entry for review", at(10, 0)))
        .await
        .unwrap();
    daily_reviews
        .upsert_completed("7", date(), "persisted review", "model", "v1")
        .await
        .unwrap();

    let undo = service
        .command(&command(JournalCommand::Undo))
        .await
        .unwrap();
    let review = service
        .command(&command(JournalCommand::ReviewToday))
        .await
        .unwrap();
    let persisted_review = daily_reviews
        .find_by_user_and_date("7", date())
        .await
        .unwrap();

    assert_eq!(undo.text, "Deleted last entry.");
    assert_eq!(review.text, "No review available for today yet.");
    assert!(persisted_review.is_none());
}

#[tokio::test]
async fn review_date_returns_unavailable_when_runner_is_not_configured() {
    let (service, pool) = setup_with_pool().await;

    let outgoing = service
        .command(&command(JournalCommand::ReviewDate { date: date() }))
        .await
        .unwrap();
    let review_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM daily_reviews")
        .fetch_one(&pool)
        .await
        .unwrap();

    assert_eq!(
        outgoing.text,
        "Daily review generation is not configured yet."
    );
    assert_eq!(review_count, 0);
}

#[tokio::test]
async fn review_date_returns_existing_review() {
    let runner = FakeDailyReviewRunner::with_fetch_result(Ok(Some(daily_review("stored review"))));
    let service = setup_with_daily_review_runner(runner.clone()).await;

    let outgoing = service
        .command(&command(JournalCommand::ReviewDate { date: date() }))
        .await
        .unwrap();

    assert_eq!(
        outgoing.text,
        "Daily review for 2026-04-28\n\nstored review"
    );
    assert_eq!(runner.calls(), vec![("7".to_string(), date())]);
}

#[tokio::test]
async fn review_date_returns_not_available_when_no_review_exists() {
    let runner = FakeDailyReviewRunner::with_fetch_result(Ok(None));
    let service = setup_with_daily_review_runner(runner).await;

    let outgoing = service
        .command(&command(JournalCommand::ReviewDate { date: date() }))
        .await
        .unwrap();

    assert_eq!(outgoing.text, "No review available for 2026-04-28 yet.");
}

#[tokio::test]
async fn review_date_returns_not_available_on_fetch_error() {
    let runner = FakeDailyReviewRunner::with_fetch_result(Err(DailyReviewServiceError::Storage(
        "database unavailable".to_string(),
    )));
    let service = setup_with_daily_review_runner(runner).await;

    let outgoing = service
        .command(&command(JournalCommand::ReviewDate { date: date() }))
        .await
        .unwrap();

    assert_eq!(outgoing.text, "No review available for 2026-04-28 yet.");
}

#[tokio::test]
async fn review_date_does_not_store_command_text_as_journal_entry() {
    let (service, pool) = setup_with_pool().await;

    service
        .command(&command(JournalCommand::ReviewDate { date: date() }))
        .await
        .unwrap();

    let entry_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM journal_entries")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(entry_count, 0);
}

#[tokio::test]
async fn review_error_returns_error_message() {
    let service = setup().await;

    let outgoing = service
        .command(&command(JournalCommand::ReviewError {
            message: "Date 2026-05-01 is in the future. Only past and present dates are supported."
                .to_string(),
        }))
        .await
        .unwrap();

    assert_eq!(
        outgoing.text,
        "Date 2026-05-01 is in the future. Only past and present dates are supported."
    );
}

#[tokio::test]
async fn command_recent_usage_returns_usage_message() {
    let service = setup().await;

    let outgoing = service
        .command(&command(JournalCommand::RecentUsage))
        .await
        .unwrap();

    assert_eq!(
        outgoing.text,
        "Usage: /recent [number]\n\nExamples:\n/recent\n/recent 5"
    );
}

#[tokio::test]
async fn last_returns_empty_response_when_no_entry_in_conversation() {
    let service = setup().await;

    let outgoing = service
        .command(&command(JournalCommand::Last))
        .await
        .unwrap();

    assert_eq!(outgoing.text, "No journal entry found.");
}

#[tokio::test]
async fn last_formats_latest_entry_for_current_conversation() {
    let service = setup().await;
    service
        .process(&incoming_for_conversation(
            "42",
            "1",
            "current old",
            at(10, 0),
        ))
        .await
        .unwrap();
    service
        .process(&incoming_for_conversation(
            "99",
            "2",
            "other newer",
            at(12, 0),
        ))
        .await
        .unwrap();
    service
        .process(&incoming_for_conversation(
            "42",
            "3",
            "current new",
            at(11, 0),
        ))
        .await
        .unwrap();

    let outgoing = service
        .command(&command(JournalCommand::Last))
        .await
        .unwrap();

    assert_eq!(
        outgoing.text,
        "Last entry:\n\n\"current new\"\n\nReceived at: 2026-04-28 11:00\n\nUse /undo to delete it."
    );
}

#[tokio::test]
async fn undo_returns_empty_response_when_no_entry_in_conversation() {
    let service = setup().await;

    let outgoing = service
        .command(&command(JournalCommand::Undo))
        .await
        .unwrap();

    assert_eq!(outgoing.text, "No journal entry to delete.");
}

#[tokio::test]
async fn undo_deletes_latest_entry_for_current_conversation() {
    let service = setup().await;
    service
        .process(&incoming_for_conversation(
            "42",
            "1",
            "current old",
            at(10, 0),
        ))
        .await
        .unwrap();
    service
        .process(&incoming_for_conversation(
            "99",
            "2",
            "other newer",
            at(12, 0),
        ))
        .await
        .unwrap();
    service
        .process(&incoming_for_conversation(
            "42",
            "3",
            "current new",
            at(11, 0),
        ))
        .await
        .unwrap();

    let undo = service
        .command(&command(JournalCommand::Undo))
        .await
        .unwrap();
    let last_current = service
        .command(&command(JournalCommand::Last))
        .await
        .unwrap();
    let last_other = service
        .command(&JournalCommandRequest {
            source: MessageSource::Telegram,
            source_conversation_id: "99".to_string(),
            user_id: "7".to_string(),
            received_at: at(12, 0),
            command: JournalCommand::Last,
        })
        .await
        .unwrap();

    assert_eq!(undo.text, "Deleted last entry.");
    assert_eq!(
        last_current.text,
        "Last entry:\n\n\"current old\"\n\nReceived at: 2026-04-28 10:00\n\nUse /undo to delete it."
    );
    assert_eq!(
        last_other.text,
        "Last entry:\n\n\"other newer\"\n\nReceived at: 2026-04-28 12:00\n\nUse /undo to delete it."
    );
}

#[tokio::test]
async fn recent_returns_empty_response_when_no_entries() {
    let service = setup().await;

    let result = service
        .command(&command(JournalCommand::Recent {
            requested_limit: DEFAULT_RECENT_LIMIT,
        }))
        .await
        .unwrap();

    assert_eq!(result.text, "No journal entries found.");
}

#[tokio::test]
async fn recent_formats_entries_newest_first() {
    let service = setup().await;

    service
        .process(&incoming("1", "first", at(10, 0)))
        .await
        .unwrap();
    service
        .process(&incoming("2", "second", at(11, 0)))
        .await
        .unwrap();

    let outgoing = service
        .command(&command(JournalCommand::Recent {
            requested_limit: 10,
        }))
        .await
        .unwrap();

    assert_eq!(
        outgoing.text,
        "2026-04-28 11:00 - second\n2026-04-28 10:00 - first"
    );
}

#[tokio::test]
async fn recent_respects_limit() {
    let service = setup().await;

    service
        .process(&incoming("1", "first", at(10, 0)))
        .await
        .unwrap();
    service
        .process(&incoming("2", "second", at(11, 0)))
        .await
        .unwrap();
    service
        .process(&incoming("3", "third", at(12, 0)))
        .await
        .unwrap();

    let outgoing = service
        .command(&command(JournalCommand::Recent { requested_limit: 2 }))
        .await
        .unwrap();

    assert!(outgoing.text.contains("third"));
    assert!(outgoing.text.contains("second"));
    assert!(!outgoing.text.contains("first"));
}

#[tokio::test]
async fn recent_caps_requested_limit() {
    let service = setup().await;

    for index in 1..=51 {
        service
            .process(&incoming(
                &index.to_string(),
                &format!("entry {index}"),
                Utc.with_ymd_and_hms(2026, 4, 28, 0, index, 0).unwrap(),
            ))
            .await
            .unwrap();
    }

    let outgoing = service
        .command(&command(JournalCommand::Recent {
            requested_limit: 100,
        }))
        .await
        .unwrap();

    assert_eq!(outgoing.text.lines().count(), MAX_RECENT_LIMIT as usize);
    assert!(outgoing.text.contains("entry 51"));
    assert!(!outgoing.text.contains("2026-04-28 00:01 - entry 1"));
}

#[tokio::test]
async fn today_formats_entries_oldest_first() {
    let service = setup().await;

    service
        .process(&incoming("1", "first", at(10, 0)))
        .await
        .unwrap();
    service
        .process(&incoming("2", "second", at(11, 0)))
        .await
        .unwrap();

    let outgoing = service
        .command(&command(JournalCommand::Today))
        .await
        .unwrap();

    assert_eq!(
        outgoing.text,
        "2026-04-28 10:00 - first\n2026-04-28 11:00 - second"
    );
}

#[tokio::test]
async fn today_returns_empty_response_when_no_entries() {
    let service = setup().await;

    let outgoing = service
        .command(&command(JournalCommand::Today))
        .await
        .unwrap();

    assert_eq!(outgoing.text, "No journal entries found for today.");
}

#[tokio::test]
async fn stats_formats_basic_statistics() {
    let service = setup().await;

    service
        .process(&incoming("1", "first", at(10, 0)))
        .await
        .unwrap();
    service
        .process(&incoming(
            "2",
            "tomorrow",
            Utc.with_ymd_and_hms(2026, 4, 29, 9, 0, 0).unwrap(),
        ))
        .await
        .unwrap();

    let outgoing = service
        .command(&command(JournalCommand::Stats))
        .await
        .unwrap();

    assert_eq!(
        outgoing.text,
        "Journal stats:\nTotal entries: 2\nEntries today: 1\nLatest entry: 2026-04-29 09:00"
    );
}

#[tokio::test]
async fn command_help_includes_search() {
    let service = setup().await;

    let outgoing = service
        .command(&command(JournalCommand::Help))
        .await
        .unwrap();

    assert!(outgoing.text.contains("/search <query>"));
}

#[tokio::test]
async fn command_search_usage_returns_usage_message() {
    let service = setup().await;

    let outgoing = service
        .command(&command(JournalCommand::SearchUsage))
        .await
        .unwrap();

    assert!(outgoing.text.contains("Usage: /search <query>"));
}

#[tokio::test]
async fn unknown_command_returns_help_response() {
    let service = setup().await;

    let outgoing = service
        .command(&command(JournalCommand::Unknown {
            command: "/other".to_string(),
        }))
        .await
        .unwrap();

    assert!(outgoing.text.starts_with("Unknown command: /other"));
    assert!(outgoing.text.contains("/help - show commands"));
}

#[tokio::test]
async fn unknown_command_does_not_store_command_text_as_journal_entry() {
    let (service, pool) = setup_with_pool().await;

    service
        .command(&command(JournalCommand::Unknown {
            command: "/other".to_string(),
        }))
        .await
        .unwrap();

    let entry_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM journal_entries")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(entry_count, 0);
}

#[tokio::test]
async fn command_search_returns_not_configured_when_search_not_set_up() {
    let service = setup().await;

    let outgoing = service
        .command(&command(JournalCommand::Search {
            query: "something".to_string(),
        }))
        .await
        .unwrap();

    assert_eq!(outgoing.text, "Search is not configured.");
}

#[tokio::test]
async fn command_search_returns_empty_when_no_embeddings_exist() {
    let (service, _, repo) = setup_with_search(FakeEmbedder::succeeds()).await;

    repo.store(&incoming("1", "some text", at(10, 0)))
        .await
        .unwrap();

    let outgoing = service
        .command(&command(JournalCommand::Search {
            query: "query".to_string(),
        }))
        .await
        .unwrap();

    assert_eq!(outgoing.text, "No results found.");
}

#[tokio::test]
async fn command_search_returns_results_when_embeddings_exist() {
    let (service, index, repo) = setup_with_search(FakeEmbedder::succeeds()).await;

    repo.store(&incoming("1", "journal entry text", at(10, 0)))
        .await
        .unwrap();
    let entry_id: i64 =
        sqlx::query_scalar("SELECT id FROM journal_entries WHERE source_message_id = '1'")
            .fetch_one(repo.pool())
            .await
            .unwrap();
    index
        .store_embedding(
            entry_id,
            TEST_MODEL,
            SUPPORTED_EMBEDDING_DIMENSIONS,
            &Embedding::new(
                vec![1.0; SUPPORTED_EMBEDDING_DIMENSIONS],
                SUPPORTED_EMBEDDING_DIMENSIONS,
            )
            .unwrap(),
        )
        .await
        .unwrap();

    let outgoing = service
        .command(&command(JournalCommand::Search {
            query: "query".to_string(),
        }))
        .await
        .unwrap();

    assert!(outgoing.text.contains("Search results for: query"));
    assert!(outgoing.text.contains("journal entry text"));
}

#[tokio::test]
async fn command_search_returns_error_message_when_embedder_fails() {
    let (service, _, _) = setup_with_search(FakeEmbedder::fails()).await;

    let outgoing = service
        .command(&command(JournalCommand::Search {
            query: "query".to_string(),
        }))
        .await
        .unwrap();

    assert!(outgoing.text.starts_with("Search failed:"));
}

#[derive(Clone)]
struct PanickingExtractionRunner;

#[async_trait::async_trait]
impl JournalEntryExtractionRunner for PanickingExtractionRunner {
    fn model(&self) -> &str {
        "panicking-extraction-model"
    }

    fn prompt_version(&self) -> &str {
        "panicking_v1"
    }

    async fn extract_entry(
        &self,
        _journal_entry_id: i64,
        _text: &str,
    ) -> Result<(), JournalEntryExtractionServiceError> {
        panic!("intentional capture-time panic");
    }
}

#[tokio::test]
async fn process_survives_panic_in_capture_time_extraction() {
    // The spawned background task panics inside the extraction runner.
    // catch_unwind must absorb it so the runtime stays healthy and
    // subsequent calls to `process` keep working.
    let (service, _repo) = setup_with_entry_extraction_runner(PanickingExtractionRunner).await;

    let first = service
        .process(&incoming("200", "first", at(10, 0)))
        .await
        .expect("first message must be saved despite panicking background task");
    assert_eq!(first.text, "Message saved.");

    // Give the background task a moment to run-and-panic-and-be-caught.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let second = service
        .process(&incoming("201", "second", at(10, 1)))
        .await
        .expect("service must remain healthy after a background panic");
    assert_eq!(second.text, "Message saved.");
}

#[derive(Debug, Clone)]
struct FakeWeeklyReviewRunner {
    fetch_result: Result<Option<WeeklyReview>, WeeklyReviewServiceError>,
    calls: Arc<Mutex<Vec<(String, NaiveDate)>>>,
}

impl FakeWeeklyReviewRunner {
    fn with_fetch_result(
        fetch_result: Result<Option<WeeklyReview>, WeeklyReviewServiceError>,
    ) -> Self {
        Self {
            fetch_result,
            calls: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn calls(&self) -> Vec<(String, NaiveDate)> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl WeeklyReviewRunner for FakeWeeklyReviewRunner {
    async fn review_week(
        &self,
        _user_id: &str,
        _week_start: NaiveDate,
    ) -> Result<WeeklyReviewResult, WeeklyReviewServiceError> {
        Ok(WeeklyReviewResult::SparseWeek)
    }

    async fn fetch_review(
        &self,
        user_id: &str,
        week_start: NaiveDate,
    ) -> Result<Option<WeeklyReview>, WeeklyReviewServiceError> {
        self.calls
            .lock()
            .unwrap()
            .push((user_id.to_string(), week_start));
        self.fetch_result.clone()
    }
}

fn weekly_review(text: &str, week_start: NaiveDate) -> WeeklyReview {
    WeeklyReview {
        id: 1,
        user_id: "7".to_string(),
        week_start_date: week_start,
        review_text: Some(text.to_string()),
        model: "test-model".to_string(),
        prompt_version: "v1".to_string(),
        status: WeeklyReviewStatus::Completed,
        error_message: None,
        delivered_at: None,
        delivery_error: None,
        inputs_snapshot: None,
        created_at: at(10, 0),
        updated_at: at(10, 0),
    }
}

async fn setup_with_weekly_review_runner<R>(runner: R) -> JournalService
where
    R: WeeklyReviewRunner + 'static,
{
    database::register_sqlite_vec_extension();
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::migrate!().run(&pool).await.unwrap();
    JournalService::new(JournalRepository::new(pool)).with_weekly_review_runner(runner)
}

// 2026-04-28 is a Tuesday. previous ISO Monday relative to today is 2026-04-20.
fn previous_week_monday() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 4, 20).unwrap()
}

#[tokio::test]
async fn week_review_returns_unavailable_when_runner_is_not_configured() {
    let service = setup().await;

    let outgoing = service
        .command(&command(JournalCommand::WeekReviewLast))
        .await
        .unwrap();

    assert_eq!(
        outgoing.text,
        "Weekly review generation is not configured yet."
    );
}

#[tokio::test]
async fn week_review_fetches_previous_iso_week_monday() {
    let runner = FakeWeeklyReviewRunner::with_fetch_result(Ok(Some(weekly_review(
        "stored weekly review",
        previous_week_monday(),
    ))));
    let service = setup_with_weekly_review_runner(runner.clone()).await;

    let outgoing = service
        .command(&command(JournalCommand::WeekReviewLast))
        .await
        .unwrap();

    assert_eq!(
        outgoing.text,
        "Weekly review for week of 2026-04-20\n\nstored weekly review"
    );
    assert_eq!(
        runner.calls(),
        vec![("7".to_string(), previous_week_monday())]
    );
}

#[tokio::test]
async fn week_review_returns_not_available_when_no_review_exists() {
    let runner = FakeWeeklyReviewRunner::with_fetch_result(Ok(None));
    let service = setup_with_weekly_review_runner(runner).await;

    let outgoing = service
        .command(&command(JournalCommand::WeekReviewLast))
        .await
        .unwrap();

    assert_eq!(
        outgoing.text,
        "No weekly review available for the week of 2026-04-20 yet."
    );
}

#[tokio::test]
async fn week_review_returns_not_available_on_fetch_error() {
    let runner = FakeWeeklyReviewRunner::with_fetch_result(Err(WeeklyReviewServiceError::Storage(
        "database unavailable".to_string(),
    )));
    let service = setup_with_weekly_review_runner(runner).await;

    let outgoing = service
        .command(&command(JournalCommand::WeekReviewLast))
        .await
        .unwrap();

    assert_eq!(
        outgoing.text,
        "No weekly review available for the week of 2026-04-20 yet."
    );
}
