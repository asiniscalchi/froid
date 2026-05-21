use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::info;

use crate::database;
use crate::cli::ServeConfig;
use crate::journal::service::JournalService;
use crate::prompts::PromptRepository;
use crate::journal::embedding::EmbeddingConfig;
use crate::journal::extraction::JournalEntryExtractionRuntimeConfig;
use crate::journal::review::DailyReviewRuntimeConfig;
use crate::journal::week_review::WeeklyReviewRuntimeConfig;
use crate::journal::review::signals::wiring::DailyReviewSignalRuntimeConfig;
use crate::handler::MessageHandler;
use crate::messages::{IncomingMessage, OutgoingMessage};
use crate::journal::command::JournalCommandRequest;

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
    legacy_db_path: PathBuf,
    allowed_user_id: Option<u64>,

    // Active connection caches
    services: Arc<RwLock<HashMap<String, JournalService>>>,
    spawned_workers: Arc<RwLock<HashSet<String>>>,
}

impl JournalServiceRegistry {
    pub fn new(
        config: ServeConfig,
        embedding_config: Option<EmbeddingConfig>,
        entry_extraction_config: JournalEntryExtractionRuntimeConfig,
        daily_review_config: DailyReviewRuntimeConfig,
        weekly_review_config: WeeklyReviewRuntimeConfig,
        signal_runtime_config: DailyReviewSignalRuntimeConfig,
        delivery_configured: bool,
        shutdown: CancellationToken,
    ) -> Self {
        let base_dir = PathBuf::from("data").join("journals");
        let legacy_db_path = PathBuf::from(&config.database_path);
        let allowed_user_id = config.telegram_allowed_user_id;

        Self {
            config: Arc::new(config),
            embedding_config,
            entry_extraction_config,
            daily_review_config,
            weekly_review_config,
            signal_runtime_config,
            delivery_configured,
            shutdown,
            base_dir,
            legacy_db_path,
            allowed_user_id,
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
    pub async fn discover_and_register_existing(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if self.base_dir.exists() {
            let mut entries = tokio::fs::read_dir(&self.base_dir).await?;
            while let Some(entry) = entries.next_entry().await? {
                let path = entry.path();
                if path.is_file() && path.extension().is_some_and(|ext| ext == "sqlite3") {
                    if let Some(file_name) = path.file_stem().and_then(|s| s.to_str()) {
                        if let Some(chat_id) = file_name.strip_prefix("user_") {
                            info!(chat_id, "discovered existing tenant database; pre-registering");
                            let _ = self.get_or_create(chat_id).await?;
                        }
                    }
                }
            }
        }

        // Also pre-register legacy database if allowed user is set and file exists
        if let Some(allowed_user_id) = self.allowed_user_id {
            if self.legacy_db_path.exists() {
                let chat_id = allowed_user_id.to_string();
                info!(chat_id, "discovered legacy database for allowed user; pre-registering");
                let _ = self.get_or_create(&chat_id).await?;
            }
        }

        Ok(())
    }

    /// Retrieve or dynamically create a `JournalService` for the given Telegram `chat_id`
    pub async fn get_or_create(&self, chat_id: &str) -> Result<JournalService, Box<dyn std::error::Error + Send + Sync>> {
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

        // Determine DB path: backward compatible legacy path or isolated path
        let db_path = if let Some(allowed_id) = self.allowed_user_id 
            && chat_id == allowed_id.to_string() 
        {
            self.legacy_db_path.clone()
        } else {
            // Ensure journals directory exists
            tokio::fs::create_dir_all(&self.base_dir).await?;
            self.base_dir.join(format!("user_{}.sqlite3", chat_id))
        };

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
            crate::app::spawn_tenant_workers(
                pool,
                self.config.clone(),
                prompt_repository,
                self.embedding_config.clone(),
                self.entry_extraction_config.clone(),
                self.daily_review_config.clone(),
                self.weekly_review_config.clone(),
                self.signal_runtime_config.clone(),
                self.shutdown.clone(),
            );
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
