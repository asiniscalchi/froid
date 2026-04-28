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

    pub async fn recent(
        &self,
        user_id: &str,
        limit: u32,
    ) -> Result<Option<OutgoingMessage>, sqlx::Error> {
        let entries = self.repository.fetch_recent(user_id, limit).await?;

        if entries.is_empty() {
            return Ok(None);
        }

        let text = entries
            .iter()
            .map(|e| {
                format!("{} — {}", e.received_at.format("%Y-%m-%d %H:%M"), e.text)
            })
            .collect::<Vec<_>>()
            .join("\n");

        Ok(Some(OutgoingMessage { text }))
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use sqlx::SqlitePool;

    use super::*;
    use crate::{journal::repository::JournalRepository, messages::MessageSource};

    async fn setup() -> JournalService {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        JournalService::new(JournalRepository::new(pool))
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
    async fn recent_returns_none_when_no_entries() {
        let service = setup().await;

        let result = service.recent("7", 10).await.unwrap();

        assert!(result.is_none());
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

        let outgoing = service.recent("7", 10).await.unwrap().unwrap();

        assert_eq!(
            outgoing.text,
            "2026-04-28 11:00 — second\n2026-04-28 10:00 — first"
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

        let outgoing = service.recent("7", 2).await.unwrap().unwrap();

        assert!(outgoing.text.contains("third"));
        assert!(outgoing.text.contains("second"));
        assert!(!outgoing.text.contains("first"));
    }
}
