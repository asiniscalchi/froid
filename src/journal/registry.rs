use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::info;

use crate::cli::ServeConfig;
use crate::database;
use crate::handler::MessageHandler;
use crate::journal::command::JournalCommandRequest;
use crate::journal::embedding::EmbeddingConfig;
use crate::journal::extraction::JournalEntryExtractionRuntimeConfig;
use crate::journal::review::DailyReviewRuntimeConfig;
use crate::journal::review::signals::wiring::DailyReviewSignalRuntimeConfig;
use crate::journal::service::JournalService;
use crate::journal::week_review::WeeklyReviewRuntimeConfig;
use crate::messages::{IncomingMessage, OutgoingMessage};
use crate::prompts::PromptRepository;

pub struct JournalServiceRegistryConfig {
    pub config: ServeConfig,
    pub embedding_config: Option<EmbeddingConfig>,
    pub entry_extraction_config: JournalEntryExtractionRuntimeConfig,
    pub daily_review_config: DailyReviewRuntimeConfig,
    pub weekly_review_config: WeeklyReviewRuntimeConfig,
    pub signal_runtime_config: DailyReviewSignalRuntimeConfig,
    pub delivery_configured: bool,
    pub shutdown: CancellationToken,
}

#[derive(Clone)]
pub struct JournalServiceRegistry {
    config: Arc<ServeConfig>,
    embedding_config: Option<EmbeddingConfig>,
    entry_extraction_config: JournalEntryExtractionRuntimeConfig,
    daily_review_config: DailyReviewRuntimeConfig,
    weekly_review_config: WeeklyReviewRuntimeConfig,
    signal_runtime_config: DailyReviewSignalRuntimeConfig,
    delivery_configured: bool,
    shutdown: CancellationToken,

    base_dir: PathBuf,

    // Active connection caches
    services: Arc<RwLock<HashMap<String, JournalService>>>,
    spawned_workers: Arc<RwLock<HashSet<String>>>,
}

impl JournalServiceRegistry {
    pub fn new(config: JournalServiceRegistryConfig) -> Self {
        let data_dir = std::path::Path::new(&config.config.database_path)
            .parent()
            .unwrap_or_else(|| std::path::Path::new("data"));
        let base_dir = data_dir.join("journals");

        Self {
            config: Arc::new(config.config),
            embedding_config: config.embedding_config,
            entry_extraction_config: config.entry_extraction_config,
            daily_review_config: config.daily_review_config,
            weekly_review_config: config.weekly_review_config,
            signal_runtime_config: config.signal_runtime_config,
            delivery_configured: config.delivery_configured,
            shutdown: config.shutdown,
            base_dir,
            services: Arc::new(RwLock::new(HashMap::new())),
            spawned_workers: Arc::new(RwLock::new(HashSet::new())),
        }
    }

    /// Override the default base directory (mainly for testing)
    pub fn with_base_dir(mut self, base_dir: PathBuf) -> Self {
        self.base_dir = base_dir;
        self
    }

    /// Discover existing tenant databases on disk and pre-register/initialize them
    pub async fn discover_and_register_existing(
        &self,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if self.base_dir.exists() {
            let mut entries = tokio::fs::read_dir(&self.base_dir).await?;
            while let Some(entry) = entries.next_entry().await? {
                let path = entry.path();
                if path.is_file()
                    && path.extension().is_some_and(|ext| ext == "sqlite3")
                    && let Some(file_name) = path.file_stem().and_then(|s| s.to_str())
                    && let Some(chat_id) = file_name.strip_prefix("user_")
                {
                    info!(
                        chat_id,
                        "discovered existing tenant database; pre-registering"
                    );
                    let _ = self.get_or_create(chat_id).await?;
                }
            }
        }

        Ok(())
    }

    /// Retrieve or dynamically create a `JournalService` for the given Telegram `chat_id`
    pub async fn get_or_create(
        &self,
        chat_id: &str,
    ) -> Result<JournalService, Box<dyn std::error::Error + Send + Sync>> {
        // First check read lock for cached instance
        {
            let guard = self.services.read().await;
            if let Some(service) = guard.get(chat_id) {
                return Ok(service.clone());
            }
        }

        // Cache miss: lock write lock to load/create
        let mut guard = self.services.write().await;
        // Double-check to avoid race condition
        if let Some(service) = guard.get(chat_id) {
            return Ok(service.clone());
        }

        info!(chat_id, "initializing tenant database connection");

        // Ensure journals directory exists
        tokio::fs::create_dir_all(&self.base_dir).await?;
        let db_path = self.base_dir.join(format!("user_{}.sqlite3", chat_id));

        let database_url = format!("sqlite:{}", db_path.display());
        let pool = database::connect_pool(&database_url).await?;

        // Run migrations on this isolated database
        sqlx::migrate!().run(&pool).await?;

        // Build prompt repository for this isolated database
        let prompt_repository = PromptRepository::new(pool.clone());

        // Build the JournalService for this pool
        let service = crate::app::build_journal_service(
            pool.clone(),
            &prompt_repository,
            self.embedding_config.clone(),
            self.entry_extraction_config.clone(),
            self.daily_review_config.clone(),
            self.weekly_review_config.clone(),
            self.delivery_configured,
        )
        .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { e.to_string().into() })?;

        // Spawn background workers for this tenant pool if not already spawned
        let mut spawned = self.spawned_workers.write().await;
        if !spawned.contains(chat_id) {
            info!(chat_id, "spawning background workers for tenant");
            crate::app::spawn_tenant_workers(crate::app::JournalTenantWorkersConfig {
                pool,
                config: self.config.clone(),
                prompt_repository,
                embedding_config: self.embedding_config.clone(),
                entry_extraction_config: self.entry_extraction_config.clone(),
                daily_review_config: self.daily_review_config.clone(),
                weekly_review_config: self.weekly_review_config.clone(),
                signal_runtime_config: self.signal_runtime_config.clone(),
                shutdown: self.shutdown.clone(),
            });
            spawned.insert(chat_id.to_string());
        }

        guard.insert(chat_id.to_string(), service.clone());
        Ok(service)
    }
}

impl MessageHandler for JournalServiceRegistry {
    async fn process(
        &self,
        message: &IncomingMessage,
    ) -> Result<OutgoingMessage, Box<dyn std::error::Error + Send + Sync>> {
        let service = self.get_or_create(&message.source_conversation_id).await?;
        service.process(message).await.map_err(Into::into)
    }

    async fn command(
        &self,
        request: &JournalCommandRequest,
    ) -> Result<OutgoingMessage, Box<dyn std::error::Error + Send + Sync>> {
        let service = self.get_or_create(&request.source_conversation_id).await?;
        service.command(request).await.map_err(Into::into)
    }
}
