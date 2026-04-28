use sqlx::SqlitePool;

use crate::messages::IncomingMessage;

#[derive(Debug, Clone)]
pub struct JournalRepository {
    pool: SqlitePool,
}

impl JournalRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn store(&self, message: &IncomingMessage) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"
            INSERT OR IGNORE INTO journal_entries
                (user_id, source, source_chat_id, source_message_id, raw_text, received_at)
            VALUES (?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&message.user_id)
        .bind(message.source.to_string())
        .bind(&message.source_chat_id)
        .bind(&message.source_message_id)
        .bind(&message.text)
        .bind(message.received_at)
        .execute(&self.pool)
        .await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use sqlx::SqlitePool;

    use super::*;
    use crate::messages::MessageSource;

    async fn setup() -> JournalRepository {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        JournalRepository::new(pool)
    }

    fn incoming(source_message_id: &str) -> IncomingMessage {
        IncomingMessage {
            source: MessageSource::Telegram,
            source_chat_id: "42".to_string(),
            source_message_id: source_message_id.to_string(),
            user_id: "7".to_string(),
            text: "hello froid".to_string(),
            received_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn stores_incoming_message() {
        let repo = setup().await;
        let message = incoming("100");

        repo.store(&message).await.unwrap();

        let row = sqlx::query(
            "SELECT user_id, source, source_chat_id, source_message_id, raw_text FROM journal_entries",
        )
        .fetch_one(&repo.pool)
        .await
        .unwrap();

        use sqlx::Row;
        assert_eq!(row.get::<String, _>("user_id"), "7");
        assert_eq!(row.get::<String, _>("source"), "telegram");
        assert_eq!(row.get::<String, _>("source_chat_id"), "42");
        assert_eq!(row.get::<String, _>("source_message_id"), "100");
        assert_eq!(row.get::<String, _>("raw_text"), "hello froid");
    }

    #[tokio::test]
    async fn ignores_duplicate_source_message() {
        let repo = setup().await;
        let message = incoming("100");

        repo.store(&message).await.unwrap();
        repo.store(&message).await.unwrap();

        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM journal_entries")
                .fetch_one(&repo.pool)
                .await
                .unwrap();

        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn stores_different_messages_independently() {
        let repo = setup().await;

        repo.store(&incoming("100")).await.unwrap();
        repo.store(&incoming("101")).await.unwrap();

        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM journal_entries")
                .fetch_one(&repo.pool)
                .await
                .unwrap();

        assert_eq!(count, 2);
    }
}
