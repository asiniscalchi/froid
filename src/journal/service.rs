use std::sync::Arc;

use async_trait::async_trait;
use tracing::{error, warn};

use crate::{
    handler::MessageHandler,
    journal::{
        command::{JournalCommand, JournalCommandRequest, MAX_RECENT_LIMIT},
        embedding::{
            Embedder, EmbedderError, Embedding, EmbeddingIndex, EmbeddingRepositoryError,
            PendingEmbeddingCounter,
        },
        responses::{
            daily_review_failure_response, daily_review_unavailable_response,
            daily_review_usage_response, format_daily_review, format_daily_review_for_date,
            format_entries, help_response, message_saved_response, no_entries_for_date_response,
            no_entries_response, no_entries_today_response, recent_usage_response, start_response,
            stats_response, status_response,
        },
        review::{DailyReview, DailyReviewResult, service::DailyReviewRunner},
        search::{
            SearchService, SemanticSearchService, format_search_results, search_empty_response,
            search_error_response, search_unavailable_response, search_usage_response,
        },
        status::{
            DailyReviewDateMode, DailyReviewDeliveryStatus, DailyReviewGenerationStatus,
            DailyReviewStatus, EmbeddingStatus, EmbeddingStatusConfig, SemanticSearchStatus,
            StatusReport,
        },
    },
    messages::{IncomingMessage, OutgoingMessage},
};

use super::repository::JournalRepository;

#[derive(Clone)]
pub struct JournalService {
    repository: JournalRepository,
    search: Option<Arc<dyn SearchService>>,
    capture_embedding: Option<Arc<dyn CaptureEmbeddingService>>,
    daily_review: Option<Arc<dyn DailyReviewRunner>>,
    embedding_status_config: Option<EmbeddingStatusConfig>,
    pending_embedding_counter: Option<Arc<dyn PendingEmbeddingCounter>>,
    daily_review_prompt_version: Option<String>,
}

impl JournalService {
    pub fn new(repository: JournalRepository) -> Self {
        Self {
            repository,
            search: None,
            capture_embedding: None,
            daily_review: None,
            embedding_status_config: None,
            pending_embedding_counter: None,
            daily_review_prompt_version: None,
        }
    }

    pub fn with_search<I, E>(mut self, search: SemanticSearchService<I, E>) -> Self
    where
        I: EmbeddingIndex + Send + Sync + 'static,
        E: Embedder + Send + Sync + 'static,
    {
        self.search = Some(Arc::new(search));
        self
    }

    pub fn with_embedding_status_config(mut self, config: EmbeddingStatusConfig) -> Self {
        self.embedding_status_config = Some(config);
        self
    }

    pub fn with_pending_embedding_counter<C>(mut self, counter: C) -> Self
    where
        C: PendingEmbeddingCounter + 'static,
    {
        self.pending_embedding_counter = Some(Arc::new(counter));
        self
    }

    pub fn with_capture_embedding<I, E>(mut self, index: I, embedder: E) -> Self
    where
        I: EmbeddingIndex + Send + Sync + 'static,
        E: Embedder + Send + Sync + 'static,
    {
        self.capture_embedding = Some(Arc::new(ImmediateCaptureEmbeddingService::new(
            index, embedder,
        )));
        self
    }

    pub fn with_daily_review_runner<R>(mut self, daily_review: R) -> Self
    where
        R: DailyReviewRunner + 'static,
    {
        self.daily_review = Some(Arc::new(daily_review));
        self
    }

    pub fn with_daily_review_prompt_version(mut self, prompt_version: impl Into<String>) -> Self {
        self.daily_review_prompt_version = Some(prompt_version.into());
        self
    }

    pub async fn process(&self, message: &IncomingMessage) -> Result<OutgoingMessage, sqlx::Error> {
        if let Some(journal_entry_id) = self.repository.store(message).await?
            && let Some(capture_embedding) = &self.capture_embedding
            && let Err(error) = capture_embedding
                .embed_entry(journal_entry_id, &message.text)
                .await
        {
            warn!(
                journal_entry_id,
                error = %error,
                "failed to create journal entry embedding after capture"
            );
        }

        Ok(OutgoingMessage {
            text: message_saved_response(),
        })
    }

    pub async fn command(
        &self,
        request: &JournalCommandRequest,
    ) -> Result<OutgoingMessage, sqlx::Error> {
        match &request.command {
            JournalCommand::Start => Ok(OutgoingMessage {
                text: start_response(),
            }),
            JournalCommand::Help => Ok(OutgoingMessage {
                text: help_response(),
            }),
            JournalCommand::Recent { requested_limit } => {
                self.recent(&request.user_id, *requested_limit).await
            }
            JournalCommand::RecentUsage => Ok(OutgoingMessage {
                text: recent_usage_response(),
            }),
            JournalCommand::Today => {
                self.today(&request.user_id, request.received_at.date_naive())
                    .await
            }
            JournalCommand::Stats => {
                self.stats(&request.user_id, request.received_at.date_naive())
                    .await
            }
            JournalCommand::Status => {
                self.status(&request.user_id, request.received_at.date_naive())
                    .await
            }
            JournalCommand::ReviewToday => Ok(self
                .review_today(&request.user_id, request.received_at.date_naive())
                .await),
            JournalCommand::ReviewDate { date } => {
                Ok(self.review_date(&request.user_id, *date).await)
            }
            JournalCommand::ReviewUsage => Ok(OutgoingMessage {
                text: daily_review_usage_response(),
            }),
            JournalCommand::ReviewError { message } => Ok(OutgoingMessage {
                text: message.clone(),
            }),
            JournalCommand::Search { query } => {
                Ok(self.search_command(&request.user_id, query).await)
            }
            JournalCommand::SearchUsage => Ok(OutgoingMessage {
                text: search_usage_response(),
            }),
        }
    }

    async fn review_today(&self, user_id: &str, date: chrono::NaiveDate) -> OutgoingMessage {
        self.run_review(
            user_id,
            date,
            format_daily_review,
            no_entries_today_response(),
        )
        .await
    }

    async fn review_date(&self, user_id: &str, date: chrono::NaiveDate) -> OutgoingMessage {
        self.run_review(
            user_id,
            date,
            |review| format_daily_review_for_date(review, date),
            no_entries_for_date_response(date),
        )
        .await
    }

    async fn run_review(
        &self,
        user_id: &str,
        date: chrono::NaiveDate,
        format_review: impl Fn(&DailyReview) -> String,
        empty_text: String,
    ) -> OutgoingMessage {
        let Some(daily_review) = &self.daily_review else {
            return OutgoingMessage {
                text: daily_review_unavailable_response(),
            };
        };

        match daily_review.review_day(user_id, date).await {
            Ok(DailyReviewResult::Existing(review) | DailyReviewResult::Generated(review)) => {
                OutgoingMessage {
                    text: format_review(&review),
                }
            }
            Ok(DailyReviewResult::EmptyDay) => OutgoingMessage { text: empty_text },
            Ok(DailyReviewResult::GenerationFailed(failure)) => {
                warn!(
                    user_id = %failure.user_id,
                    review_date = %failure.review_date,
                    model = %failure.model,
                    prompt_version = %failure.prompt_version,
                    error = %failure.error_message,
                    "failed to generate daily review"
                );
                OutgoingMessage {
                    text: daily_review_failure_response(),
                }
            }
            Err(error) => {
                error!(%error, "failed to process daily review command");
                OutgoingMessage {
                    text: daily_review_failure_response(),
                }
            }
        }
    }

    async fn search_command(&self, user_id: &str, query: &str) -> OutgoingMessage {
        let Some(search) = &self.search else {
            return OutgoingMessage {
                text: search_unavailable_response(),
            };
        };

        match search.search(user_id, query).await {
            Ok(results) if results.is_empty() => OutgoingMessage {
                text: search_empty_response(),
            },
            Ok(results) => OutgoingMessage {
                text: format_search_results(query, &results),
            },
            Err(e) => OutgoingMessage {
                text: search_error_response(&e),
            },
        }
    }

    async fn recent(&self, user_id: &str, limit: u32) -> Result<OutgoingMessage, sqlx::Error> {
        let limit = limit.min(MAX_RECENT_LIMIT);
        let entries = self.repository.fetch_recent(user_id, limit).await?;

        if entries.is_empty() {
            return Ok(OutgoingMessage {
                text: no_entries_response(),
            });
        }

        Ok(OutgoingMessage {
            text: format_entries(&entries),
        })
    }

    async fn today(
        &self,
        user_id: &str,
        date: chrono::NaiveDate,
    ) -> Result<OutgoingMessage, sqlx::Error> {
        let entries = self.repository.fetch_today(user_id, date).await?;

        if entries.is_empty() {
            return Ok(OutgoingMessage {
                text: no_entries_today_response(),
            });
        }

        Ok(OutgoingMessage {
            text: format_entries(&entries),
        })
    }

    async fn stats(
        &self,
        user_id: &str,
        today: chrono::NaiveDate,
    ) -> Result<OutgoingMessage, sqlx::Error> {
        let stats = self.repository.stats(user_id, today).await?;

        Ok(OutgoingMessage {
            text: stats_response(&stats),
        })
    }

    async fn status(
        &self,
        user_id: &str,
        today: chrono::NaiveDate,
    ) -> Result<OutgoingMessage, sqlx::Error> {
        let journal = self.repository.stats(user_id, today).await?;
        let embeddings = self.embedding_status(user_id).await;
        let daily_review = self.daily_review_status();

        Ok(OutgoingMessage {
            text: status_response(&StatusReport {
                journal,
                embeddings,
                daily_review,
            }),
        })
    }

    async fn embedding_status(&self, user_id: &str) -> EmbeddingStatus {
        let semantic_search = if self.search.is_some() && self.embedding_status_config.is_some() {
            SemanticSearchStatus::Enabled
        } else {
            SemanticSearchStatus::Unavailable
        };

        let pending_embeddings = match (
            self.embedding_status_config.as_ref(),
            self.pending_embedding_counter.as_ref(),
        ) {
            (Some(config), Some(counter)) => {
                match counter
                    .count_entries_missing_embedding_for_user(user_id, &config.model)
                    .await
                {
                    Ok(count) => Some(count),
                    Err(error) => {
                        warn!(%error, "failed to count pending embeddings for status");
                        None
                    }
                }
            }
            _ => None,
        };

        EmbeddingStatus {
            semantic_search,
            config: self.embedding_status_config.clone(),
            pending_embeddings,
        }
    }

    fn daily_review_status(&self) -> DailyReviewStatus {
        let generation = if self.daily_review.is_some() {
            DailyReviewGenerationStatus::Configured
        } else {
            DailyReviewGenerationStatus::NotConfigured
        };

        DailyReviewStatus {
            generation,
            prompt_version: self.daily_review_prompt_version.clone(),
            delivery: DailyReviewDeliveryStatus::NotImplemented,
            date_mode: DailyReviewDateMode::Utc,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CaptureEmbeddingError {
    Embedder(EmbedderError),
    Index(EmbeddingRepositoryError),
}

impl std::fmt::Display for CaptureEmbeddingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Embedder(error) => write!(f, "failed to embed journal entry: {error}"),
            Self::Index(error) => write!(f, "failed to store journal entry embedding: {error}"),
        }
    }
}

impl std::error::Error for CaptureEmbeddingError {}

#[async_trait]
trait CaptureEmbeddingService: Send + Sync {
    async fn embed_entry(
        &self,
        journal_entry_id: i64,
        text: &str,
    ) -> Result<(), CaptureEmbeddingError>;
}

#[derive(Debug, Clone)]
struct ImmediateCaptureEmbeddingService<I, E> {
    index: I,
    embedder: E,
}

impl<I, E> ImmediateCaptureEmbeddingService<I, E> {
    fn new(index: I, embedder: E) -> Self {
        Self { index, embedder }
    }
}

#[async_trait]
impl<I, E> CaptureEmbeddingService for ImmediateCaptureEmbeddingService<I, E>
where
    I: EmbeddingIndex + Send + Sync,
    E: Embedder + Send + Sync,
{
    async fn embed_entry(
        &self,
        journal_entry_id: i64,
        text: &str,
    ) -> Result<(), CaptureEmbeddingError> {
        let embedding: Embedding = self
            .embedder
            .embed(text)
            .await
            .map_err(CaptureEmbeddingError::Embedder)?;

        self.index
            .store_embedding(
                journal_entry_id,
                self.embedder.model(),
                self.embedder.dimensions(),
                &embedding,
            )
            .await
            .map_err(CaptureEmbeddingError::Index)?;

        Ok(())
    }
}

impl MessageHandler for JournalService {
    async fn process(
        &self,
        message: &IncomingMessage,
    ) -> Result<OutgoingMessage, Box<dyn std::error::Error + Send + Sync>> {
        JournalService::process(self, message)
            .await
            .map_err(Into::into)
    }

    async fn command(
        &self,
        request: &JournalCommandRequest,
    ) -> Result<OutgoingMessage, Box<dyn std::error::Error + Send + Sync>> {
        JournalService::command(self, request)
            .await
            .map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use chrono::NaiveDate;
    use chrono::{TimeZone, Utc};
    use sqlx::SqlitePool;

    use super::*;
    use crate::{
        database,
        journal::{
            command::{DEFAULT_RECENT_LIMIT, JournalCommand, JournalCommandRequest},
            embedding::{
                EmbedderError, Embedding, SUPPORTED_EMBEDDING_DIMENSIONS, SqliteEmbeddingRepository,
            },
            repository::JournalRepository,
            review::{
                DailyReview, DailyReviewFailure, DailyReviewResult, DailyReviewStatus,
                generator::fake::FakeReviewGenerator,
                repository::DailyReviewRepository,
                service::{DailyReviewRunner, DailyReviewService, DailyReviewServiceError},
            },
            search::SemanticSearchService,
        },
        messages::MessageSource,
    };

    async fn setup() -> JournalService {
        database::register_sqlite_vec_extension();
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        JournalService::new(JournalRepository::new(pool))
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

    async fn setup_with_daily_review_service(
        generator: FakeReviewGenerator,
    ) -> (JournalService, DailyReviewRepository, JournalRepository) {
        database::register_sqlite_vec_extension();
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        let journal_repo = JournalRepository::new(pool.clone());
        let daily_review_repo = DailyReviewRepository::new(pool.clone());
        let daily_review_service = DailyReviewService::new(
            daily_review_repo.clone(),
            JournalRepository::new(pool),
            generator,
        );
        let service = JournalService::new(journal_repo.clone())
            .with_daily_review_runner(daily_review_service);

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
        let service =
            JournalService::new(repo.clone()).with_capture_embedding(index.clone(), embedder);
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
        result: Result<DailyReviewResult, DailyReviewServiceError>,
        calls: Arc<Mutex<Vec<(String, NaiveDate)>>>,
    }

    impl FakeDailyReviewRunner {
        fn new(result: Result<DailyReviewResult, DailyReviewServiceError>) -> Self {
            Self {
                result,
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
            user_id: &str,
            utc_date: NaiveDate,
        ) -> Result<DailyReviewResult, DailyReviewServiceError> {
            self.calls
                .lock()
                .unwrap()
                .push((user_id.to_string(), utc_date));
            self.result.clone()
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
        IncomingMessage {
            source: MessageSource::Telegram,
            source_conversation_id: "42".to_string(),
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
        assert!(index.has_embedding(entry_id, TEST_MODEL).await.unwrap());
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
        assert!(!index.has_embedding(entry_id, TEST_MODEL).await.unwrap());
    }

    #[tokio::test]
    async fn command_search_can_find_entry_embedded_during_capture() {
        let (service, _) = setup_with_search_and_capture_embedding(
            FakeEmbedder::succeeds(),
            FakeEmbedder::succeeds(),
        )
        .await;

        service
            .process(&incoming("100", "journal entry text", at(10, 0)))
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
        assert!(outgoing.text.contains("/review [today|YYYY-MM-DD|-N]"));
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
            "Froid status\n\nJournal:\n- Total entries: 0\n- Entries today UTC: 0\n\nEmbeddings:\n- Semantic search: unavailable\n- Model: unavailable\n- Dimensions: unavailable\n- Pending embeddings: unavailable\n\nDaily review:\n- Generation: not configured\n- Delivery: not implemented\n- Date mode: UTC"
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
                user_id: "7".to_string(),
                received_at: Utc.with_ymd_and_hms(2026, 4, 29, 12, 0, 0).unwrap(),
                command: JournalCommand::Status,
            })
            .await
            .unwrap();

        assert!(outgoing.text.contains("- Total entries: 2"));
        assert!(outgoing.text.contains("- Entries today UTC: 1"));
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
        let runner = FakeDailyReviewRunner::new(Ok(DailyReviewResult::EmptyDay));
        let service = setup_with_daily_review_runner(runner)
            .await
            .with_daily_review_prompt_version("daily-review-v1");

        let outgoing = service
            .command(&command(JournalCommand::Status))
            .await
            .unwrap();

        assert!(outgoing.text.contains("- Generation: configured"));
        assert!(outgoing.text.contains("- Prompt: daily-review-v1"));
        assert!(outgoing.text.contains("- Delivery: not implemented"));
        assert!(outgoing.text.contains("- Date mode: UTC"));
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
        let runner = FakeDailyReviewRunner::new(Ok(DailyReviewResult::Existing(daily_review(
            "stored review",
        ))));
        let service = setup_with_daily_review_runner(runner.clone()).await;

        let outgoing = service
            .command(&command(JournalCommand::ReviewToday))
            .await
            .unwrap();

        assert_eq!(outgoing.text, "Today's review\n\nstored review");
        assert_eq!(runner.calls(), vec![("7".to_string(), date())]);
    }

    #[tokio::test]
    async fn review_today_returns_generated_review() {
        let runner = FakeDailyReviewRunner::new(Ok(DailyReviewResult::Generated(daily_review(
            "generated review",
        ))));
        let service = setup_with_daily_review_runner(runner).await;

        let outgoing = service
            .command(&command(JournalCommand::ReviewToday))
            .await
            .unwrap();

        assert_eq!(outgoing.text, "Today's review\n\ngenerated review");
    }

    #[tokio::test]
    async fn review_today_returns_empty_day_response() {
        let runner = FakeDailyReviewRunner::new(Ok(DailyReviewResult::EmptyDay));
        let service = setup_with_daily_review_runner(runner).await;

        let outgoing = service
            .command(&command(JournalCommand::ReviewToday))
            .await
            .unwrap();

        assert_eq!(outgoing.text, "No journal entries found for today.");
    }

    #[tokio::test]
    async fn review_today_returns_failure_response_for_generation_failure() {
        let runner = FakeDailyReviewRunner::new(Ok(DailyReviewResult::GenerationFailed(
            DailyReviewFailure {
                user_id: "7".to_string(),
                review_date: date(),
                model: "test-model".to_string(),
                prompt_version: "v1".to_string(),
                error_message: "provider down".to_string(),
            },
        )));
        let service = setup_with_daily_review_runner(runner).await;

        let outgoing = service
            .command(&command(JournalCommand::ReviewToday))
            .await
            .unwrap();

        assert_eq!(
            outgoing.text,
            "I could not generate today's review right now. Please try again later."
        );
    }

    #[tokio::test]
    async fn review_today_returns_failure_response_for_service_error() {
        let runner = FakeDailyReviewRunner::new(Err(DailyReviewServiceError::Storage(
            "database unavailable".to_string(),
        )));
        let service = setup_with_daily_review_runner(runner).await;

        let outgoing = service
            .command(&command(JournalCommand::ReviewToday))
            .await
            .unwrap();

        assert_eq!(
            outgoing.text,
            "I could not generate today's review right now. Please try again later."
        );
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
    async fn review_today_generates_and_persists_through_daily_review_service() {
        let (service, daily_reviews, journal_entries) =
            setup_with_daily_review_service(FakeReviewGenerator::succeeding("persisted review"))
                .await;
        journal_entries
            .store(&incoming("1", "entry for review", at(10, 0)))
            .await
            .unwrap();

        let outgoing = service
            .command(&command(JournalCommand::ReviewToday))
            .await
            .unwrap();
        let stored = daily_reviews
            .find_by_user_and_date("7", date())
            .await
            .unwrap()
            .unwrap();

        assert_eq!(outgoing.text, "Today's review\n\npersisted review");
        assert_eq!(stored.review_text, Some("persisted review".to_string()));
        assert_eq!(stored.status, DailyReviewStatus::Completed);
    }

    #[tokio::test]
    async fn review_today_uses_command_received_at_utc_date() {
        let (service, _daily_reviews, journal_entries) = setup_with_daily_review_service(
            FakeReviewGenerator::succeeding("requested date review"),
        )
        .await;
        journal_entries
            .store(&incoming(
                "1",
                "previous date",
                Utc.with_ymd_and_hms(2026, 4, 27, 10, 0, 0).unwrap(),
            ))
            .await
            .unwrap();
        journal_entries
            .store(&incoming("2", "requested date", at(10, 0)))
            .await
            .unwrap();

        let outgoing = service
            .command(&JournalCommandRequest {
                user_id: "7".to_string(),
                received_at: at(23, 59),
                command: JournalCommand::ReviewToday,
            })
            .await
            .unwrap();

        assert_eq!(outgoing.text, "Today's review\n\nrequested date review");
    }

    #[tokio::test]
    async fn review_today_is_isolated_by_user() {
        let (service, _daily_reviews, journal_entries) =
            setup_with_daily_review_service(FakeReviewGenerator::new(vec![
                Ok("user seven review".to_string()),
                Ok("user eight review".to_string()),
            ]))
            .await;
        journal_entries
            .store(&incoming("1", "user seven entry", at(10, 0)))
            .await
            .unwrap();
        let other_user = IncomingMessage {
            source: MessageSource::Telegram,
            source_conversation_id: "42".to_string(),
            source_message_id: "2".to_string(),
            user_id: "8".to_string(),
            text: "user eight entry".to_string(),
            received_at: at(11, 0),
        };
        journal_entries.store(&other_user).await.unwrap();

        let user_seven = service
            .command(&command(JournalCommand::ReviewToday))
            .await
            .unwrap();
        let user_eight = service
            .command(&JournalCommandRequest {
                user_id: "8".to_string(),
                received_at: at(12, 0),
                command: JournalCommand::ReviewToday,
            })
            .await
            .unwrap();

        assert_eq!(user_seven.text, "Today's review\n\nuser seven review");
        assert_eq!(user_eight.text, "Today's review\n\nuser eight review");
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
        let runner = FakeDailyReviewRunner::new(Ok(DailyReviewResult::Existing(daily_review(
            "stored review",
        ))));
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
    async fn review_date_returns_generated_review() {
        let runner = FakeDailyReviewRunner::new(Ok(DailyReviewResult::Generated(daily_review(
            "generated review",
        ))));
        let service = setup_with_daily_review_runner(runner).await;

        let outgoing = service
            .command(&command(JournalCommand::ReviewDate { date: date() }))
            .await
            .unwrap();

        assert_eq!(
            outgoing.text,
            "Daily review for 2026-04-28\n\ngenerated review"
        );
    }

    #[tokio::test]
    async fn review_date_returns_empty_day_response() {
        let runner = FakeDailyReviewRunner::new(Ok(DailyReviewResult::EmptyDay));
        let service = setup_with_daily_review_runner(runner).await;

        let outgoing = service
            .command(&command(JournalCommand::ReviewDate { date: date() }))
            .await
            .unwrap();

        assert_eq!(outgoing.text, "No journal entries found for 2026-04-28.");
    }

    #[tokio::test]
    async fn review_date_returns_failure_response_for_generation_failure() {
        let runner = FakeDailyReviewRunner::new(Ok(DailyReviewResult::GenerationFailed(
            DailyReviewFailure {
                user_id: "7".to_string(),
                review_date: date(),
                model: "test-model".to_string(),
                prompt_version: "v1".to_string(),
                error_message: "provider down".to_string(),
            },
        )));
        let service = setup_with_daily_review_runner(runner).await;

        let outgoing = service
            .command(&command(JournalCommand::ReviewDate { date: date() }))
            .await
            .unwrap();

        assert_eq!(
            outgoing.text,
            "I could not generate today's review right now. Please try again later."
        );
    }

    #[tokio::test]
    async fn review_date_returns_failure_response_for_service_error() {
        let runner = FakeDailyReviewRunner::new(Err(DailyReviewServiceError::Storage(
            "database unavailable".to_string(),
        )));
        let service = setup_with_daily_review_runner(runner).await;

        let outgoing = service
            .command(&command(JournalCommand::ReviewDate { date: date() }))
            .await
            .unwrap();

        assert_eq!(
            outgoing.text,
            "I could not generate today's review right now. Please try again later."
        );
    }

    #[tokio::test]
    async fn review_date_uses_specified_date_not_received_at() {
        let (service, _daily_reviews, journal_entries) = setup_with_daily_review_service(
            FakeReviewGenerator::succeeding("explicit date review"),
        )
        .await;
        let explicit_date = NaiveDate::from_ymd_opt(2026, 4, 27).unwrap();
        journal_entries
            .store(&incoming(
                "1",
                "explicit date entry",
                Utc.with_ymd_and_hms(2026, 4, 27, 10, 0, 0).unwrap(),
            ))
            .await
            .unwrap();
        journal_entries
            .store(&incoming("2", "other date entry", at(10, 0)))
            .await
            .unwrap();

        let outgoing = service
            .command(&JournalCommandRequest {
                user_id: "7".to_string(),
                received_at: at(23, 59),
                command: JournalCommand::ReviewDate {
                    date: explicit_date,
                },
            })
            .await
            .unwrap();

        assert_eq!(
            outgoing.text,
            "Daily review for 2026-04-27\n\nexplicit date review"
        );
    }

    #[tokio::test]
    async fn review_date_is_isolated_by_user() {
        let (service, _daily_reviews, journal_entries) =
            setup_with_daily_review_service(FakeReviewGenerator::new(vec![
                Ok("user seven review".to_string()),
                Ok("user eight review".to_string()),
            ]))
            .await;
        let explicit_date = NaiveDate::from_ymd_opt(2026, 4, 27).unwrap();
        journal_entries
            .store(&incoming(
                "1",
                "user seven entry",
                Utc.with_ymd_and_hms(2026, 4, 27, 10, 0, 0).unwrap(),
            ))
            .await
            .unwrap();
        let other_user = IncomingMessage {
            source: MessageSource::Telegram,
            source_conversation_id: "42".to_string(),
            source_message_id: "2".to_string(),
            user_id: "8".to_string(),
            text: "user eight entry".to_string(),
            received_at: Utc.with_ymd_and_hms(2026, 4, 27, 11, 0, 0).unwrap(),
        };
        journal_entries.store(&other_user).await.unwrap();

        let user_seven = service
            .command(&command(JournalCommand::ReviewDate {
                date: explicit_date,
            }))
            .await
            .unwrap();
        let user_eight = service
            .command(&JournalCommandRequest {
                user_id: "8".to_string(),
                received_at: at(12, 0),
                command: JournalCommand::ReviewDate {
                    date: explicit_date,
                },
            })
            .await
            .unwrap();

        assert_eq!(
            user_seven.text,
            "Daily review for 2026-04-27\n\nuser seven review"
        );
        assert_eq!(
            user_eight.text,
            "Daily review for 2026-04-27\n\nuser eight review"
        );
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
                message:
                    "Date 2026-05-01 is in the future. Only past and present dates are supported."
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
}
