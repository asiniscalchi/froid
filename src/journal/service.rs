use crate::messages::{IncomingMessage, OutgoingMessage};

use super::repository::JournalRepository;

#[derive(Debug, Clone)]
pub struct JournalService {
    repository: JournalRepository,
}

impl JournalService {
    pub fn new(repository: JournalRepository) -> Self {
        Self { repository }
    }

    pub async fn process(&self, message: &IncomingMessage) -> Result<OutgoingMessage, sqlx::Error> {
        self.repository.store(message).await?;
        Ok(OutgoingMessage {
            text: "Message saved.".to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use sqlx::SqlitePool;

    use super::*;
    use crate::{
        journal::repository::JournalRepository,
        messages::MessageSource,
    };

    async fn setup() -> JournalService {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        JournalService::new(JournalRepository::new(pool))
    }

    fn incoming() -> IncomingMessage {
        IncomingMessage {
            source: MessageSource::Telegram,
            source_chat_id: "42".to_string(),
            source_message_id: "100".to_string(),
            user_id: "7".to_string(),
            text: "hello froid".to_string(),
            received_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn returns_confirmation_after_storing() {
        let service = setup().await;
        let message = incoming();

        let outgoing = service.process(&message).await.unwrap();

        assert_eq!(outgoing.text, "Message saved.");
    }

    #[tokio::test]
    async fn returns_confirmation_for_duplicate_without_error() {
        let service = setup().await;
        let message = incoming();

        service.process(&message).await.unwrap();
        let outgoing = service.process(&message).await.unwrap();

        assert_eq!(outgoing.text, "Message saved.");
    }
}
