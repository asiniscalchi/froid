use std::panic::AssertUnwindSafe;
use std::sync::Arc;

use async_trait::async_trait;
use futures::FutureExt;
use tracing::{error, warn};

use crate::{
    handler::MessageHandler,
    journal::{
        command::JournalCommandRequest,
        embedding::{
            Embedder, EmbedderError, Embedding, EmbeddingIndex, EmbeddingRepositoryError,
            PendingEmbeddingCounter,
        },
        extraction::service::JournalEntryExtractionRunner,
        responses::message_saved_response,
        review::{
            search::{DailyReviewSearchService, SemanticDailyReviewSearchService},
            service::DailyReviewRunner,
        },
        search::{SearchService, SemanticSearchService},
        status::EmbeddingStatusConfig,
        store::JournalEntryStore,
    },
    messages::{IncomingMessage, OutgoingMessage},
};

use super::repository::JournalRepository;

mod commands;

#[derive(Clone)]
pub struct JournalService {
    repository: JournalRepository,
    store: JournalEntryStore,
    search: Option<Arc<dyn SearchService>>,
    daily_review_search: Option<Arc<dyn DailyReviewSearchService>>,
    capture_embedding: Option<Arc<dyn CaptureEmbeddingService>>,
    entry_extraction: Option<Arc<dyn JournalEntryExtractionRunner>>,
    daily_review: Option<Arc<dyn DailyReviewRunner>>,
    embedding_status_config: Option<EmbeddingStatusConfig>,
    pending_embedding_counter: Option<Arc<dyn PendingEmbeddingCounter>>,
    daily_review_prompt_version: Option<String>,
    daily_review_delivery_configured: bool,
}

impl JournalService {
    pub fn new(repository: JournalRepository) -> Self {
        let store = JournalEntryStore::new(repository.clone_pool());
        Self {
            repository,
            store,
            search: None,
            daily_review_search: None,
            capture_embedding: None,
            entry_extraction: None,
            daily_review: None,
            embedding_status_config: None,
            pending_embedding_counter: None,
            daily_review_prompt_version: None,
            daily_review_delivery_configured: false,
        }
    }

    pub fn with_search<I, E>(mut self, search: SemanticSearchService<I, E>) -> Self
    where
        I: EmbeddingIndex<i64> + Send + Sync + 'static,
        E: Embedder + Send + Sync + 'static,
    {
        self.search = Some(Arc::new(search));
        self
    }

    pub fn with_daily_review_search<I, E>(
        mut self,
        search: SemanticDailyReviewSearchService<I, E>,
    ) -> Self
    where
        I: EmbeddingIndex<i64> + Send + Sync + 'static,
        E: Embedder + Send + Sync + 'static,
    {
        self.daily_review_search = Some(Arc::new(search));
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
        I: EmbeddingIndex<i64> + Send + Sync + 'static,
        E: Embedder + Send + Sync + 'static,
    {
        self.capture_embedding = Some(Arc::new(ImmediateCaptureEmbeddingService::new(
            index, embedder,
        )));
        self
    }

    pub fn with_entry_extraction_runner<R>(mut self, runner: R) -> Self
    where
        R: JournalEntryExtractionRunner + 'static,
    {
        self.entry_extraction = Some(Arc::new(runner));
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

    pub fn with_daily_review_delivery_configured(mut self) -> Self {
        self.daily_review_delivery_configured = true;
        self
    }

    pub async fn process(&self, message: &IncomingMessage) -> Result<OutgoingMessage, sqlx::Error> {
        if let Some(journal_entry_id) = self.store.store(message).await? {
            self.spawn_background_tasks(journal_entry_id, message.text.clone());
        }

        Ok(OutgoingMessage {
            text: message_saved_response(),
        })
    }

    fn spawn_background_tasks(&self, journal_entry_id: i64, text: String) {
        let capture_embedding = self.capture_embedding.clone();
        let entry_extraction = self.entry_extraction.clone();

        tokio::spawn(async move {
            // Wrap with catch_unwind so a panic inside the embedder or
            // extractor surfaces as an error log instead of being lost
            // to a fire-and-forget JoinHandle.
            let outcome = AssertUnwindSafe(run_capture_followups(
                journal_entry_id,
                text,
                capture_embedding,
                entry_extraction,
            ))
            .catch_unwind()
            .await;

            if outcome.is_err() {
                error!(
                    journal_entry_id,
                    "background capture task panicked; payload swallowed by catch_unwind"
                );
            }
        });
    }
}

async fn run_capture_followups(
    journal_entry_id: i64,
    text: String,
    capture_embedding: Option<Arc<dyn CaptureEmbeddingService>>,
    entry_extraction: Option<Arc<dyn JournalEntryExtractionRunner>>,
) {
    if let Some(capture_embedding) = capture_embedding
        && let Err(error) = capture_embedding.embed_entry(journal_entry_id, &text).await
    {
        warn!(
            journal_entry_id,
            error = %error,
            "failed to create journal entry embedding after capture"
        );
    }

    if let Some(entry_extraction) = entry_extraction
        && let Err(error) = entry_extraction
            .extract_entry(journal_entry_id, &text)
            .await
    {
        warn!(
            journal_entry_id,
            error = %error,
            "failed to process journal entry extraction after capture"
        );
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
    I: EmbeddingIndex<i64> + Send + Sync,
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
mod tests;
