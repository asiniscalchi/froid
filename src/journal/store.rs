use chrono::NaiveDate;
use sqlx::{Row, SqlitePool, sqlite::SqliteRow};

use crate::messages::{IncomingMessage, MessageSource};

use super::entry::{JournalEntry, StoredJournalEntry};

#[derive(Debug, Clone)]
pub struct JournalEntryStore {
    pool: SqlitePool,
}

impl JournalEntryStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn store(&self, message: &IncomingMessage) -> Result<Option<i64>, sqlx::Error> {
        let mut tx = self.pool.begin().await?;

        let result = sqlx::query(
            r#"
            INSERT OR IGNORE INTO journal_entries
                (source, source_conversation_id, source_message_id, raw_text, received_at)
            VALUES (?, ?, ?, ?, ?)
            "#,
        )
        .bind(message.source.to_string())
        .bind(&message.source_conversation_id)
        .bind(&message.source_message_id)
        .bind(&message.text)
        .bind(message.received_at)
        .execute(&mut *tx)
        .await?;

        let journal_entry_id = if result.rows_affected() == 0 {
            None
        } else {
            delete_daily_review(&mut tx, message.received_at.date_naive()).await?;
            Some(result.last_insert_rowid())
        };

        tx.commit().await?;

        Ok(journal_entry_id)
    }

    pub async fn delete_last_for_conversation(
        &self,
        _user_id: &str,
        source: &MessageSource,
        source_conversation_id: &str,
    ) -> Result<Option<StoredJournalEntry>, sqlx::Error> {
        let mut tx = self.pool.begin().await?;

        let row = sqlx::query(
            r#"
            SELECT id, raw_text, received_at
            FROM journal_entries
            WHERE source = ?
              AND source_conversation_id = ?
            ORDER BY received_at DESC, id DESC
            LIMIT 1
            "#,
        )
        .bind(source.to_string())
        .bind(source_conversation_id)
        .fetch_optional(&mut *tx)
        .await?;

        let Some(row) = row else {
            tx.commit().await?;
            return Ok(None);
        };

        let entry = row_to_stored_entry(row);

        sqlx::query(
            r#"
            DELETE FROM journal_entry_embedding_vec
            WHERE rowid IN (
                SELECT id
                FROM journal_entry_embedding_metadata
                WHERE journal_entry_id = ?
            )
            "#,
        )
        .bind(entry.id)
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            r#"
            DELETE FROM journal_entry_embedding_metadata
            WHERE journal_entry_id = ?
            "#,
        )
        .bind(entry.id)
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            r#"
            DELETE FROM journal_entries
            WHERE id = ?
            "#,
        )
        .bind(entry.id)
        .execute(&mut *tx)
        .await?;

        delete_daily_review(&mut tx, entry.entry.received_at.date_naive()).await?;

        tx.commit().await?;

        Ok(Some(entry))
    }
}

fn row_to_stored_entry(row: SqliteRow) -> StoredJournalEntry {
    StoredJournalEntry {
        id: row.get("id"),
        entry: JournalEntry {
            text: row.get("raw_text"),
            received_at: row.get("received_at"),
        },
    }
}

async fn delete_daily_review(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    date: NaiveDate,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        DELETE FROM daily_reviews
        WHERE review_date = ?
        "#,
    )
    .bind(date.to_string())
    .execute(&mut **tx)
    .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use sqlx::SqlitePool;

    use super::*;
    use crate::{
        database,
        journal::{
            embedding::{Embedding, SqliteEmbeddingRepository},
            repository::JournalRepository,
        },
    };

    async fn setup() -> (JournalEntryStore, JournalRepository, SqlitePool) {
        database::register_sqlite_vec_extension();
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        (
            JournalEntryStore::new(pool.clone()),
            JournalRepository::new(pool.clone()),
            pool,
        )
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
    async fn store_invalidates_daily_review_for_entry_date() {
        let (store, _repo, pool) = setup().await;
        sqlx::query(
            r#"
            INSERT INTO daily_reviews
                (review_date, review_text, model, prompt_version, status)
            VALUES ('2026-04-28', 'persisted review', 'model', 'v1', 'completed')
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();

        store
            .store(&incoming("1", "new entry", at(10, 0)))
            .await
            .unwrap();

        let review_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM daily_reviews")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(review_count, 0);
    }

    #[tokio::test]
    async fn delete_last_for_conversation_deletes_daily_review_for_entry_date() {
        let (store, _repo, pool) = setup().await;
        store
            .store(&incoming("1", "reviewed entry", at(10, 0)))
            .await
            .unwrap();
        sqlx::query(
            r#"
            INSERT INTO daily_reviews
                (review_date, review_text, model, prompt_version, status)
            VALUES ('2026-04-28', 'persisted review', 'model', 'v1', 'completed')
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();

        store
            .delete_last_for_conversation("7", &MessageSource::Telegram, "42")
            .await
            .unwrap();

        let review_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM daily_reviews")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(review_count, 0);
    }

    #[tokio::test]
    async fn delete_last_for_conversation_removes_embedding_rows() {
        let (store, _repo, pool) = setup().await;
        let embedding_repo = SqliteEmbeddingRepository::new(pool.clone());
        let entry_id = store
            .store(&incoming("1", "embedded entry", at(10, 0)))
            .await
            .unwrap()
            .unwrap();
        let embedding = Embedding::new(vec![0.1; 1536], 1536).unwrap();
        embedding_repo
            .store_embedding(entry_id, "test-model", 1536, &embedding)
            .await
            .unwrap();

        store
            .delete_last_for_conversation("7", &MessageSource::Telegram, "42")
            .await
            .unwrap();

        let metadata_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM journal_entry_embedding_metadata")
                .fetch_one(&pool)
                .await
                .unwrap();
        let vector_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM journal_entry_embedding_vec")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(metadata_count, 0);
        assert_eq!(vector_count, 0);
    }
}
