use chrono::{Duration, NaiveDate, TimeZone, Utc};
use sqlx::{Row, SqlitePool};

use crate::messages::IncomingMessage;

use super::entry::{JournalEntry, JournalStats};

#[derive(Debug, Clone)]
pub struct JournalRepository {
    pool: SqlitePool,
}

impl JournalRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn store(&self, message: &IncomingMessage) -> Result<(), sqlx::Error> {
        let mut tx = self.pool.begin().await?;

        sqlx::query(
            r#"
            INSERT OR IGNORE INTO journal_entries
                (user_id, source, source_conversation_id, source_message_id, raw_text, received_at)
            VALUES (?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&message.user_id)
        .bind(message.source.to_string())
        .bind(&message.source_conversation_id)
        .bind(&message.source_message_id)
        .bind(&message.text)
        .bind(message.received_at)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;

        Ok(())
    }

    pub async fn fetch_recent(
        &self,
        user_id: &str,
        limit: u32,
    ) -> Result<Vec<JournalEntry>, sqlx::Error> {
        let rows = sqlx::query(
            r#"
            SELECT raw_text, received_at
            FROM journal_entries
            WHERE user_id = ?
            ORDER BY received_at DESC, id DESC
            LIMIT ?
            "#,
        )
        .bind(user_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        let entries = rows
            .into_iter()
            .map(|row| JournalEntry {
                text: row.get("raw_text"),
                received_at: row.get("received_at"),
            })
            .collect();

        Ok(entries)
    }

    pub async fn fetch_today(
        &self,
        user_id: &str,
        date: NaiveDate,
    ) -> Result<Vec<JournalEntry>, sqlx::Error> {
        let start = Utc.from_utc_datetime(&date.and_hms_opt(0, 0, 0).unwrap());
        let end = start + Duration::days(1);

        let rows = sqlx::query(
            r#"
            SELECT raw_text, received_at
            FROM journal_entries
            WHERE user_id = ?
              AND received_at >= ?
              AND received_at < ?
            ORDER BY received_at ASC, id ASC
            "#,
        )
        .bind(user_id)
        .bind(start)
        .bind(end)
        .fetch_all(&self.pool)
        .await?;

        let entries = rows
            .into_iter()
            .map(|row| JournalEntry {
                text: row.get("raw_text"),
                received_at: row.get("received_at"),
            })
            .collect();

        Ok(entries)
    }

    pub async fn stats(
        &self,
        user_id: &str,
        today: NaiveDate,
    ) -> Result<JournalStats, sqlx::Error> {
        let start = Utc.from_utc_datetime(&today.and_hms_opt(0, 0, 0).unwrap());
        let end = start + Duration::days(1);

        let row = sqlx::query(
            r#"
            SELECT
                COUNT(*) AS total_entries,
                COALESCE(SUM(CASE WHEN received_at >= ? AND received_at < ? THEN 1 ELSE 0 END), 0) AS entries_today,
                MAX(received_at) AS latest_received_at
            FROM journal_entries
            WHERE user_id = ?
            "#,
        )
        .bind(start)
        .bind(end)
        .bind(user_id)
        .fetch_one(&self.pool)
        .await?;

        Ok(JournalStats {
            total_entries: row.get("total_entries"),
            entries_today: row.get("entries_today"),
            latest_received_at: row.get("latest_received_at"),
        })
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use sqlx::SqlitePool;

    use super::*;
    use crate::messages::MessageSource;

    async fn setup() -> JournalRepository {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        JournalRepository::new(pool)
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

    fn date() -> chrono::NaiveDate {
        chrono::NaiveDate::from_ymd_opt(2026, 4, 28).unwrap()
    }

    #[tokio::test]
    async fn stores_incoming_message() {
        let repo = setup().await;
        let message = incoming("100", "hello froid", Utc::now());

        repo.store(&message).await.unwrap();

        let row = sqlx::query(
            "SELECT user_id, source, source_conversation_id, source_message_id, raw_text FROM journal_entries",
        )
        .fetch_one(&repo.pool)
        .await
        .unwrap();

        assert_eq!(row.get::<String, _>("user_id"), "7");
        assert_eq!(row.get::<String, _>("source"), "telegram");
        assert_eq!(row.get::<String, _>("source_conversation_id"), "42");
        assert_eq!(row.get::<String, _>("source_message_id"), "100");
        assert_eq!(row.get::<String, _>("raw_text"), "hello froid");
    }

    #[tokio::test]
    async fn ignores_duplicate_source_message() {
        let repo = setup().await;
        let message = incoming("100", "hello froid", Utc::now());

        repo.store(&message).await.unwrap();
        repo.store(&message).await.unwrap();

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM journal_entries")
            .fetch_one(&repo.pool)
            .await
            .unwrap();

        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn stores_different_messages_independently() {
        let repo = setup().await;

        repo.store(&incoming("100", "hello froid", Utc::now()))
            .await
            .unwrap();
        repo.store(&incoming("101", "hello froid", Utc::now()))
            .await
            .unwrap();

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM journal_entries")
            .fetch_one(&repo.pool)
            .await
            .unwrap();

        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn fetch_recent_returns_entries_newest_first() {
        let repo = setup().await;

        repo.store(&incoming("1", "first", at(10, 0)))
            .await
            .unwrap();
        repo.store(&incoming("2", "second", at(11, 0)))
            .await
            .unwrap();
        repo.store(&incoming("3", "third", at(12, 0)))
            .await
            .unwrap();

        let entries = repo.fetch_recent("7", 10).await.unwrap();

        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].text, "third");
        assert_eq!(entries[1].text, "second");
        assert_eq!(entries[2].text, "first");
    }

    #[tokio::test]
    async fn fetch_recent_respects_limit() {
        let repo = setup().await;

        repo.store(&incoming("1", "first", at(10, 0)))
            .await
            .unwrap();
        repo.store(&incoming("2", "second", at(11, 0)))
            .await
            .unwrap();
        repo.store(&incoming("3", "third", at(12, 0)))
            .await
            .unwrap();

        let entries = repo.fetch_recent("7", 2).await.unwrap();

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].text, "third");
        assert_eq!(entries[1].text, "second");
    }

    #[tokio::test]
    async fn fetch_recent_returns_empty_for_unknown_user() {
        let repo = setup().await;

        let entries = repo.fetch_recent("unknown", 10).await.unwrap();

        assert!(entries.is_empty());
    }

    #[tokio::test]
    async fn fetch_today_returns_entries_oldest_first_for_user() {
        let repo = setup().await;

        repo.store(&incoming("1", "first", at(10, 0)))
            .await
            .unwrap();
        repo.store(&incoming("2", "second", at(11, 0)))
            .await
            .unwrap();
        repo.store(&incoming(
            "3",
            "tomorrow",
            Utc.with_ymd_and_hms(2026, 4, 29, 9, 0, 0).unwrap(),
        ))
        .await
        .unwrap();

        let entries = repo.fetch_today("7", date()).await.unwrap();

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].text, "first");
        assert_eq!(entries[1].text, "second");
    }

    #[tokio::test]
    async fn stats_returns_counts_and_latest_timestamp_for_user() {
        let repo = setup().await;

        repo.store(&incoming("1", "first", at(10, 0)))
            .await
            .unwrap();
        repo.store(&incoming(
            "2",
            "tomorrow",
            Utc.with_ymd_and_hms(2026, 4, 29, 9, 0, 0).unwrap(),
        ))
        .await
        .unwrap();

        let stats = repo.stats("7", date()).await.unwrap();

        assert_eq!(stats.total_entries, 2);
        assert_eq!(stats.entries_today, 1);
        assert_eq!(
            stats.latest_received_at,
            Some(Utc.with_ymd_and_hms(2026, 4, 29, 9, 0, 0).unwrap())
        );
    }

    #[tokio::test]
    async fn stats_returns_zeroes_for_unknown_user() {
        let repo = setup().await;

        let stats = repo.stats("unknown", date()).await.unwrap();

        assert_eq!(stats.total_entries, 0);
        assert_eq!(stats.entries_today, 0);
        assert_eq!(stats.latest_received_at, None);
    }
}
