use std::sync::Arc;

use crate::{
    handler::MessageHandler,
    journal::{
        command::{JournalCommand, JournalCommandRequest, MAX_RECENT_LIMIT},
        embedding::{Embedder, EmbeddingIndex},
        search::{
            SearchService, SemanticSearchService, format_search_results, search_empty_response,
            search_error_response, search_usage_response,
        },
    },
    messages::{IncomingMessage, OutgoingMessage},
};

use super::repository::JournalRepository;

#[derive(Clone)]
pub struct JournalService {
    repository: JournalRepository,
    search: Option<Arc<dyn SearchService>>,
}

impl JournalService {
    pub fn new(repository: JournalRepository) -> Self {
        Self {
            repository,
            search: None,
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

    pub async fn process(&self, message: &IncomingMessage) -> Result<OutgoingMessage, sqlx::Error> {
        self.repository.store(message).await?;
        Ok(OutgoingMessage {
            text: "Message saved.".to_string(),
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
            JournalCommand::Search { query } => {
                Ok(self.search_command(&request.user_id, query).await)
            }
            JournalCommand::SearchUsage => Ok(OutgoingMessage {
                text: search_usage_response(),
            }),
        }
    }

    async fn search_command(&self, user_id: &str, query: &str) -> OutgoingMessage {
        let Some(search) = &self.search else {
            return OutgoingMessage {
                text: "Search is not configured.".to_string(),
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
                text: "No journal entries found.".to_string(),
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
                text: "No journal entries found for today.".to_string(),
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
        let latest = stats
            .latest_received_at
            .map(|timestamp| timestamp.format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_else(|| "none".to_string());

        Ok(OutgoingMessage {
            text: format!(
                "Journal stats:\nTotal entries: {}\nEntries today: {}\nLatest entry: {}",
                stats.total_entries, stats.entries_today, latest
            ),
        })
    }
}

fn start_response() -> String {
    format!(
        "Froid is running.\n\nSend me any text message and I will store it as a journal entry.\n\n{}",
        help_response()
    )
}

fn help_response() -> String {
    "Commands:\n/recent [number] - show recent entries\n/today - show today's entries\n/stats - show journal stats\n/search <query> - search entries by meaning\n/help - show commands".to_string()
}

fn recent_usage_response() -> String {
    "Usage: /recent [number]\n\nExamples:\n/recent\n/recent 5".to_string()
}

fn format_entries(entries: &[super::entry::JournalEntry]) -> String {
    entries
        .iter()
        .map(|e| format!("{} - {}", e.received_at.format("%Y-%m-%d %H:%M"), e.text))
        .collect::<Vec<_>>()
        .join("\n")
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

    const TEST_MODEL: &str = "test-model";

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
        assert!(outgoing.text.contains("/stats"));
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
