use chrono::{Duration, NaiveDate, TimeZone, Utc};
use sqlx::{Row, SqlitePool, sqlite::SqliteRow};

use crate::messages::{IncomingMessage, MessageSource, SINGLE_USER_ID};

use super::entry::{JournalEntry, JournalStats, StoredJournalEntry};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalConversation {
    pub user_id: String,
    pub source_conversation_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalEntryRecord {
    pub id: String,
    pub source: String,
    pub source_conversation_id: String,
    pub source_message_id: String,
    pub text: String,
    pub received_at: chrono::DateTime<Utc>,
}

#[derive(Debug)]
pub enum BulkImportError {
    Conflict {
        source: String,
        source_conversation_id: String,
        source_message_id: String,
    },
    Database(sqlx::Error),
}

impl std::fmt::Display for BulkImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BulkImportError::Conflict {
                source,
                source_conversation_id,
                source_message_id,
            } => write!(
                f,
                "conflict on ({source}, {source_conversation_id}, {source_message_id})"
            ),
            BulkImportError::Database(err) => write!(f, "database error: {err}"),
        }
    }
}

impl std::error::Error for BulkImportError {}

fn is_unique_violation(err: &dyn sqlx::error::DatabaseError) -> bool {
    err.code().as_deref() == Some("2067")
        || err.message().to_lowercase().contains("unique constraint")
}

fn map_entry(row: SqliteRow) -> JournalEntry {
    JournalEntry {
        text: row.get("raw_text"),
        received_at: row.get("received_at"),
    }
}

#[derive(Debug, Clone)]
pub struct JournalRepository {
    pool: SqlitePool,
}

impl JournalRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub(crate) fn clone_pool(&self) -> SqlitePool {
        self.pool.clone()
    }

    #[cfg(test)]
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    pub async fn store(&self, message: &IncomingMessage) -> Result<Option<String>, sqlx::Error> {
        let id = ulid::Ulid::new().to_string();
        let result = sqlx::query(
            r#"
            INSERT OR IGNORE INTO journal_entries
                (id, source, source_conversation_id, source_message_id, raw_text, received_at)
            VALUES (?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&id)
        .bind(message.source.to_string())
        .bind(&message.source_conversation_id)
        .bind(&message.source_message_id)
        .bind(&message.text)
        .bind(message.received_at)
        .execute(&self.pool)
        .await?;

        Ok((result.rows_affected() != 0).then_some(id))
    }

    pub async fn fetch_recent(
        &self,
        _user_id: &str,
        limit: u32,
    ) -> Result<Vec<StoredJournalEntry>, sqlx::Error> {
        let rows = sqlx::query(
            r#"
            SELECT id, raw_text, received_at
            FROM journal_entries
            ORDER BY received_at DESC, id DESC
            LIMIT ?
            "#,
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| StoredJournalEntry {
                id: row.get("id"),
                entry: map_entry(row),
            })
            .collect())
    }

    pub async fn fetch_last_for_conversation(
        &self,
        _user_id: &str,
        source: &MessageSource,
        source_conversation_id: &str,
    ) -> Result<Option<StoredJournalEntry>, sqlx::Error> {
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
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|row| StoredJournalEntry {
            id: row.get("id"),
            entry: map_entry(row),
        }))
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

        let entry = StoredJournalEntry {
            id: row.get("id"),
            entry: map_entry(row),
        };

        sqlx::query(
            r#"
            DELETE FROM journal_entries
            WHERE id = ?
            "#,
        )
        .bind(&entry.id)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;

        Ok(Some(entry))
    }

    pub async fn search_text(
        &self,
        _user_id: &str,
        query: &str,
        from_date: Option<NaiveDate>,
        to_date_exclusive: Option<NaiveDate>,
        limit: u32,
    ) -> Result<Vec<StoredJournalEntry>, sqlx::Error> {
        let mut sql = String::from(
            r#"SELECT id, raw_text, received_at
               FROM journal_entries
               WHERE LOWER(raw_text) LIKE LOWER(?)"#,
        );
        if from_date.is_some() {
            sql.push_str(" AND received_at >= ?");
        }
        if to_date_exclusive.is_some() {
            sql.push_str(" AND received_at < ?");
        }
        sql.push_str(" ORDER BY received_at DESC, id DESC LIMIT ?");

        let mut q = sqlx::query(&sql).bind(format!("%{query}%"));
        if let Some(d) = from_date {
            let start = Utc.from_utc_datetime(&d.and_hms_opt(0, 0, 0).unwrap());
            q = q.bind(start);
        }
        if let Some(d) = to_date_exclusive {
            let end = Utc.from_utc_datetime(&d.and_hms_opt(0, 0, 0).unwrap());
            q = q.bind(end);
        }
        q = q.bind(limit);

        let rows = q.fetch_all(&self.pool).await?;

        Ok(rows
            .into_iter()
            .map(|row| StoredJournalEntry {
                id: row.get("id"),
                entry: map_entry(row),
            })
            .collect())
    }

    pub async fn fetch_all(&self) -> Result<Vec<StoredJournalEntry>, sqlx::Error> {
        let rows = sqlx::query(
            r#"
            SELECT id, raw_text, received_at
            FROM journal_entries
            ORDER BY received_at DESC, id DESC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| StoredJournalEntry {
                id: row.get("id"),
                entry: map_entry(row),
            })
            .collect())
    }

    pub async fn fetch_all_for_export(&self) -> Result<Vec<JournalEntryRecord>, sqlx::Error> {
        let rows = sqlx::query(
            r#"
            SELECT id, source, source_conversation_id, source_message_id, raw_text, received_at
            FROM journal_entries
            ORDER BY received_at DESC, id DESC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| JournalEntryRecord {
                id: row.get("id"),
                source: row.get("source"),
                source_conversation_id: row.get("source_conversation_id"),
                source_message_id: row.get("source_message_id"),
                text: row.get("raw_text"),
                received_at: row.get("received_at"),
            })
            .collect())
    }

    pub async fn bulk_import(
        &self,
        records: &[JournalEntryRecord],
    ) -> Result<usize, BulkImportError> {
        let mut tx = self.pool.begin().await.map_err(BulkImportError::Database)?;
        for record in records {
            let id = if record.id.is_empty() {
                ulid::Ulid::new().to_string()
            } else {
                record.id.clone()
            };
            let result = sqlx::query(
                r#"
                INSERT INTO journal_entries
                    (id, source, source_conversation_id, source_message_id, raw_text, received_at)
                VALUES (?, ?, ?, ?, ?, ?)
                "#,
            )
            .bind(&id)
            .bind(&record.source)
            .bind(&record.source_conversation_id)
            .bind(&record.source_message_id)
            .bind(&record.text)
            .bind(record.received_at)
            .execute(&mut *tx)
            .await;

            if let Err(err) = result {
                return Err(match err {
                    sqlx::Error::Database(db_err) if is_unique_violation(db_err.as_ref()) => {
                        BulkImportError::Conflict {
                            source: record.source.clone(),
                            source_conversation_id: record.source_conversation_id.clone(),
                            source_message_id: record.source_message_id.clone(),
                        }
                    }
                    other => BulkImportError::Database(other),
                });
            }
        }
        tx.commit().await.map_err(BulkImportError::Database)?;
        Ok(records.len())
    }

    pub async fn fetch_in_range(
        &self,
        _user_id: &str,
        start_date: NaiveDate,
        end_date_exclusive: NaiveDate,
        limit: u32,
    ) -> Result<Vec<StoredJournalEntry>, sqlx::Error> {
        let start = Utc.from_utc_datetime(&start_date.and_hms_opt(0, 0, 0).unwrap());
        let end = Utc.from_utc_datetime(&end_date_exclusive.and_hms_opt(0, 0, 0).unwrap());

        let rows = sqlx::query(
            r#"
            SELECT id, raw_text, received_at
            FROM journal_entries
            WHERE received_at >= ?
              AND received_at < ?
            ORDER BY received_at DESC, id DESC
            LIMIT ?
            "#,
        )
        .bind(start)
        .bind(end)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| StoredJournalEntry {
                id: row.get("id"),
                entry: map_entry(row),
            })
            .collect())
    }

    pub async fn fetch_today(
        &self,
        _user_id: &str,
        date: NaiveDate,
    ) -> Result<Vec<StoredJournalEntry>, sqlx::Error> {
        let start = Utc.from_utc_datetime(&date.and_hms_opt(0, 0, 0).unwrap());
        let end = start + Duration::days(1);

        let rows = sqlx::query(
            r#"
            SELECT id, raw_text, received_at
            FROM journal_entries
            WHERE received_at >= ?
              AND received_at < ?
            ORDER BY received_at ASC, id ASC
            "#,
        )
        .bind(start)
        .bind(end)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| StoredJournalEntry {
                id: row.get("id"),
                entry: map_entry(row),
            })
            .collect())
    }

    pub async fn conversations_with_entries_for_date(
        &self,
        source: &MessageSource,
        date: NaiveDate,
    ) -> Result<Vec<JournalConversation>, sqlx::Error> {
        let start = Utc.from_utc_datetime(&date.and_hms_opt(0, 0, 0).unwrap());
        let end = start + Duration::days(1);

        let rows = sqlx::query(
            r#"
            SELECT DISTINCT source_conversation_id
            FROM journal_entries
            WHERE source = ?
              AND received_at >= ?
              AND received_at < ?
            ORDER BY source_conversation_id ASC
            "#,
        )
        .bind(source.to_string())
        .bind(start)
        .bind(end)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| JournalConversation {
                user_id: SINGLE_USER_ID.to_string(),
                source_conversation_id: row.get("source_conversation_id"),
            })
            .collect())
    }

    pub async fn conversations_with_entries_in_range(
        &self,
        source: &MessageSource,
        start_date: NaiveDate,
        end_date_exclusive: NaiveDate,
    ) -> Result<Vec<JournalConversation>, sqlx::Error> {
        let start = Utc.from_utc_datetime(&start_date.and_hms_opt(0, 0, 0).unwrap());
        let end = Utc.from_utc_datetime(&end_date_exclusive.and_hms_opt(0, 0, 0).unwrap());

        let rows = sqlx::query(
            r#"
            SELECT DISTINCT source_conversation_id
            FROM journal_entries
            WHERE source = ?
              AND received_at >= ?
              AND received_at < ?
            ORDER BY source_conversation_id ASC
            "#,
        )
        .bind(source.to_string())
        .bind(start)
        .bind(end)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| JournalConversation {
                user_id: SINGLE_USER_ID.to_string(),
                source_conversation_id: row.get("source_conversation_id"),
            })
            .collect())
    }

    pub async fn fetch_by_ids(
        &self,
        _user_id: &str,
        ids: &[String],
    ) -> Result<Vec<(String, JournalEntry)>, sqlx::Error> {
        if ids.is_empty() {
            return Ok(vec![]);
        }

        let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
        let sql = format!(
            "SELECT id, raw_text, received_at FROM journal_entries WHERE id IN ({placeholders})"
        );
        let mut query = sqlx::query(&sql);
        for id in ids {
            query = query.bind(id);
        }
        let rows = query.fetch_all(&self.pool).await?;

        Ok(rows
            .into_iter()
            .map(|row| {
                let id = row.get("id");
                (id, map_entry(row))
            })
            .collect())
    }

    pub async fn stats(
        &self,
        _user_id: &str,
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
            "#,
        )
        .bind(start)
        .bind(end)
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
    use crate::database;
    use crate::messages::MessageSource;

    async fn setup() -> JournalRepository {
        database::register_sqlite_vec_extension();
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        JournalRepository::new(pool)
    }

    fn incoming(
        source_message_id: &str,
        text: &str,
        received_at: chrono::DateTime<Utc>,
    ) -> IncomingMessage {
        incoming_for_conversation("42", source_message_id, text, received_at)
    }

    fn incoming_for_conversation(
        source_conversation_id: &str,
        source_message_id: &str,
        text: &str,
        received_at: chrono::DateTime<Utc>,
    ) -> IncomingMessage {
        IncomingMessage {
            source: MessageSource::Telegram,
            source_conversation_id: source_conversation_id.to_string(),
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

        let journal_entry_id = repo.store(&message).await.unwrap();

        let row = sqlx::query(
            "SELECT id, source, source_conversation_id, source_message_id, raw_text FROM journal_entries",
        )
        .fetch_one(&repo.pool)
        .await
        .unwrap();

        assert_eq!(journal_entry_id, Some(row.get("id")));
        assert_eq!(row.get::<String, _>("source"), "telegram");
        assert_eq!(row.get::<String, _>("source_conversation_id"), "42");
        assert_eq!(row.get::<String, _>("source_message_id"), "100");
        assert_eq!(row.get::<String, _>("raw_text"), "hello froid");
    }

    #[tokio::test]
    async fn ignores_duplicate_source_message() {
        let repo = setup().await;
        let message = incoming("100", "hello froid", Utc::now());

        let first = repo.store(&message).await.unwrap();
        let second = repo.store(&message).await.unwrap();

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM journal_entries")
            .fetch_one(&repo.pool)
            .await
            .unwrap();

        assert!(first.is_some());
        assert_eq!(second, None);
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
        assert_eq!(entries[0].entry.text, "third");
        assert_eq!(entries[1].entry.text, "second");
        assert_eq!(entries[2].entry.text, "first");
    }

    #[tokio::test]
    async fn fetch_all_returns_every_entry_newest_first() {
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

        let entries = repo.fetch_all().await.unwrap();

        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].entry.text, "third");
        assert_eq!(entries[1].entry.text, "second");
        assert_eq!(entries[2].entry.text, "first");
    }

    #[tokio::test]
    async fn fetch_all_for_export_returns_full_records() {
        let repo = setup().await;
        repo.store(&incoming("1", "hi", at(10, 0))).await.unwrap();

        let rows = repo.fetch_all_for_export().await.unwrap();

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].source, "telegram");
        assert_eq!(rows[0].source_conversation_id, "42");
        assert_eq!(rows[0].source_message_id, "1");
        assert_eq!(rows[0].text, "hi");
    }

    #[tokio::test]
    async fn bulk_import_inserts_all_records() {
        let repo = setup().await;

        let records = vec![
            JournalEntryRecord {
                id: String::new(),
                source: "telegram".to_string(),
                source_conversation_id: "42".to_string(),
                source_message_id: "imp-1".to_string(),
                text: "imported one".to_string(),
                received_at: at(10, 0),
            },
            JournalEntryRecord {
                id: String::new(),
                source: "telegram".to_string(),
                source_conversation_id: "42".to_string(),
                source_message_id: "imp-2".to_string(),
                text: "imported two".to_string(),
                received_at: at(11, 0),
            },
        ];

        let inserted = repo.bulk_import(&records).await.unwrap();
        assert_eq!(inserted, 2);

        let entries = repo.fetch_recent("7", 10).await.unwrap();
        assert_eq!(entries.len(), 2);
    }

    #[tokio::test]
    async fn bulk_import_rolls_back_on_unique_violation() {
        let repo = setup().await;
        repo.store(&incoming("dup", "existing", at(10, 0)))
            .await
            .unwrap();

        let records = vec![
            JournalEntryRecord {
                id: String::new(),
                source: "telegram".to_string(),
                source_conversation_id: "42".to_string(),
                source_message_id: "fresh".to_string(),
                text: "fresh entry".to_string(),
                received_at: at(11, 0),
            },
            JournalEntryRecord {
                id: String::new(),
                source: "telegram".to_string(),
                source_conversation_id: "42".to_string(),
                source_message_id: "dup".to_string(),
                text: "collides".to_string(),
                received_at: at(12, 0),
            },
        ];

        let err = repo.bulk_import(&records).await.unwrap_err();
        match err {
            BulkImportError::Conflict {
                source,
                source_conversation_id,
                source_message_id,
            } => {
                assert_eq!(source, "telegram");
                assert_eq!(source_conversation_id, "42");
                assert_eq!(source_message_id, "dup");
            }
            other => panic!("expected Conflict, got {other:?}"),
        }

        let entries = repo.fetch_recent("7", 10).await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].entry.text, "existing");
    }

    #[tokio::test]
    async fn fetch_all_returns_empty_when_no_entries() {
        let repo = setup().await;

        let entries = repo.fetch_all().await.unwrap();

        assert!(entries.is_empty());
    }

    #[tokio::test]
    async fn fetch_last_for_conversation_returns_latest_entry_for_current_conversation() {
        let repo = setup().await;
        repo.store(&incoming_for_conversation(
            "42",
            "1",
            "current old",
            at(10, 0),
        ))
        .await
        .unwrap();
        repo.store(&incoming_for_conversation(
            "42",
            "2",
            "current new",
            at(11, 0),
        ))
        .await
        .unwrap();
        repo.store(&incoming_for_conversation(
            "99",
            "3",
            "other conversation",
            at(12, 0),
        ))
        .await
        .unwrap();

        let entry = repo
            .fetch_last_for_conversation("7", &MessageSource::Telegram, "42")
            .await
            .unwrap()
            .unwrap();

        assert_eq!(entry.entry.text, "current new");
    }

    #[tokio::test]
    async fn fetch_last_for_conversation_breaks_timestamp_ties_by_id() {
        let repo = setup().await;
        repo.store(&incoming("1", "first inserted", at(10, 0)))
            .await
            .unwrap();
        repo.store(&incoming("2", "second inserted", at(10, 0)))
            .await
            .unwrap();

        let entry = repo
            .fetch_last_for_conversation("7", &MessageSource::Telegram, "42")
            .await
            .unwrap()
            .unwrap();

        assert_eq!(entry.entry.text, "second inserted");
    }

    #[tokio::test]
    async fn delete_last_for_conversation_deletes_same_entry_selected_by_fetch_last() {
        let repo = setup().await;
        repo.store(&incoming("1", "first inserted", at(10, 0)))
            .await
            .unwrap();
        repo.store(&incoming("2", "second inserted", at(10, 0)))
            .await
            .unwrap();

        let fetched = repo
            .fetch_last_for_conversation("7", &MessageSource::Telegram, "42")
            .await
            .unwrap()
            .unwrap();
        let deleted = repo
            .delete_last_for_conversation("7", &MessageSource::Telegram, "42")
            .await
            .unwrap()
            .unwrap();
        let remaining = repo.fetch_recent("7", 10).await.unwrap();

        assert_eq!(deleted.id, fetched.id);
        assert_eq!(deleted.entry.text, "second inserted");
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].entry.text, "first inserted");
    }

    #[tokio::test]
    async fn delete_last_for_conversation_does_not_delete_other_conversations() {
        let repo = setup().await;
        repo.store(&incoming_for_conversation("42", "1", "current", at(10, 0)))
            .await
            .unwrap();
        repo.store(&incoming_for_conversation("99", "2", "other", at(11, 0)))
            .await
            .unwrap();

        let deleted = repo
            .delete_last_for_conversation("7", &MessageSource::Telegram, "42")
            .await
            .unwrap()
            .unwrap();
        let other = repo
            .fetch_last_for_conversation("7", &MessageSource::Telegram, "99")
            .await
            .unwrap()
            .unwrap();

        assert_eq!(deleted.entry.text, "current");
        assert_eq!(other.entry.text, "other");
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
        assert_eq!(entries[0].entry.text, "third");
        assert_eq!(entries[1].entry.text, "second");
    }

    #[tokio::test]
    async fn fetch_recent_returns_empty_when_journal_has_no_entries() {
        let repo = setup().await;

        let entries = repo.fetch_recent("7", 10).await.unwrap();

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
        assert_eq!(entries[0].entry.text, "first");
        assert_eq!(entries[1].entry.text, "second");
    }

    fn ymd(y: i32, m: u32, d: u32) -> chrono::NaiveDate {
        chrono::NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn at_on(y: i32, m: u32, d: u32, h: u32, mi: u32) -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, h, mi, 0).unwrap()
    }

    #[tokio::test]
    async fn fetch_in_range_returns_entries_within_range_newest_first() {
        let repo = setup().await;

        repo.store(&incoming("1", "before", at_on(2026, 4, 27, 23, 0)))
            .await
            .unwrap();
        repo.store(&incoming("2", "first", at_on(2026, 4, 28, 10, 0)))
            .await
            .unwrap();
        repo.store(&incoming("3", "second", at_on(2026, 4, 28, 12, 0)))
            .await
            .unwrap();
        repo.store(&incoming("4", "after", at_on(2026, 4, 29, 0, 0)))
            .await
            .unwrap();

        let entries = repo
            .fetch_in_range("7", ymd(2026, 4, 28), ymd(2026, 4, 29), 10)
            .await
            .unwrap();

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].entry.text, "second");
        assert_eq!(entries[1].entry.text, "first");
    }

    #[tokio::test]
    async fn fetch_in_range_treats_end_date_as_exclusive() {
        let repo = setup().await;

        repo.store(&incoming("1", "midnight start", at_on(2026, 4, 28, 0, 0)))
            .await
            .unwrap();
        repo.store(&incoming("2", "midnight end", at_on(2026, 4, 29, 0, 0)))
            .await
            .unwrap();

        let entries = repo
            .fetch_in_range("7", ymd(2026, 4, 28), ymd(2026, 4, 29), 10)
            .await
            .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].entry.text, "midnight start");
    }

    #[tokio::test]
    async fn fetch_in_range_respects_limit() {
        let repo = setup().await;

        repo.store(&incoming("1", "first", at_on(2026, 4, 28, 9, 0)))
            .await
            .unwrap();
        repo.store(&incoming("2", "second", at_on(2026, 4, 28, 10, 0)))
            .await
            .unwrap();
        repo.store(&incoming("3", "third", at_on(2026, 4, 28, 11, 0)))
            .await
            .unwrap();

        let entries = repo
            .fetch_in_range("7", ymd(2026, 4, 28), ymd(2026, 4, 29), 2)
            .await
            .unwrap();

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].entry.text, "third");
        assert_eq!(entries[1].entry.text, "second");
    }

    #[tokio::test]
    async fn fetch_in_range_returns_all_entries_in_single_user_journal() {
        let repo = setup().await;

        repo.store(&incoming("1", "mine", at_on(2026, 4, 28, 10, 0)))
            .await
            .unwrap();
        repo.store(&IncomingMessage {
            source: MessageSource::Telegram,
            source_conversation_id: "99".to_string(),
            source_message_id: "2".to_string(),
            user_id: "other_user".to_string(),
            text: "theirs".to_string(),
            received_at: at_on(2026, 4, 28, 11, 0),
        })
        .await
        .unwrap();

        let entries = repo
            .fetch_in_range("7", ymd(2026, 4, 28), ymd(2026, 4, 29), 10)
            .await
            .unwrap();

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].entry.text, "theirs");
        assert_eq!(entries[1].entry.text, "mine");
    }

    #[tokio::test]
    async fn fetch_in_range_returns_empty_when_no_entries_in_range() {
        let repo = setup().await;

        repo.store(&incoming("1", "outside", at_on(2026, 4, 27, 10, 0)))
            .await
            .unwrap();

        let entries = repo
            .fetch_in_range("7", ymd(2026, 4, 28), ymd(2026, 4, 29), 10)
            .await
            .unwrap();

        assert!(entries.is_empty());
    }

    #[tokio::test]
    async fn fetch_in_range_returns_empty_for_empty_range() {
        let repo = setup().await;

        repo.store(&incoming("1", "entry", at_on(2026, 4, 28, 10, 0)))
            .await
            .unwrap();

        let entries = repo
            .fetch_in_range("7", ymd(2026, 4, 28), ymd(2026, 4, 28), 10)
            .await
            .unwrap();

        assert!(entries.is_empty());
    }

    #[tokio::test]
    async fn fetch_in_range_breaks_timestamp_ties_by_id_desc() {
        let repo = setup().await;

        repo.store(&incoming("1", "first inserted", at_on(2026, 4, 28, 10, 0)))
            .await
            .unwrap();
        repo.store(&incoming("2", "second inserted", at_on(2026, 4, 28, 10, 0)))
            .await
            .unwrap();

        let entries = repo
            .fetch_in_range("7", ymd(2026, 4, 28), ymd(2026, 4, 29), 10)
            .await
            .unwrap();

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].entry.text, "second inserted");
        assert_eq!(entries[1].entry.text, "first inserted");
    }

    #[tokio::test]
    async fn search_text_matches_substring_case_insensitively() {
        let repo = setup().await;

        repo.store(&incoming("1", "I felt anxious before the call", at(10, 0)))
            .await
            .unwrap();
        repo.store(&incoming("2", "calm afternoon", at(11, 0)))
            .await
            .unwrap();
        repo.store(&incoming("3", "ANXIETY again today", at(12, 0)))
            .await
            .unwrap();

        let entries = repo.search_text("7", "ANXI", None, None, 10).await.unwrap();

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].entry.text, "ANXIETY again today");
        assert_eq!(entries[1].entry.text, "I felt anxious before the call");
    }

    #[tokio::test]
    async fn search_text_returns_results_newest_first() {
        let repo = setup().await;

        repo.store(&incoming("1", "match one", at(10, 0)))
            .await
            .unwrap();
        repo.store(&incoming("2", "match two", at(12, 0)))
            .await
            .unwrap();
        repo.store(&incoming("3", "match three", at(11, 0)))
            .await
            .unwrap();

        let entries = repo
            .search_text("7", "match", None, None, 10)
            .await
            .unwrap();

        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].entry.text, "match two");
        assert_eq!(entries[1].entry.text, "match three");
        assert_eq!(entries[2].entry.text, "match one");
    }

    #[tokio::test]
    async fn search_text_respects_limit() {
        let repo = setup().await;

        for i in 0..5u32 {
            repo.store(&incoming(&i.to_string(), &format!("match {i}"), at(10, i)))
                .await
                .unwrap();
        }

        let entries = repo.search_text("7", "match", None, None, 2).await.unwrap();

        assert_eq!(entries.len(), 2);
    }

    #[tokio::test]
    async fn search_text_returns_matches_from_the_single_user_journal() {
        let repo = setup().await;

        repo.store(&incoming("1", "mine matches", at(10, 0)))
            .await
            .unwrap();
        repo.store(&IncomingMessage {
            source: MessageSource::Telegram,
            source_conversation_id: "99".to_string(),
            source_message_id: "2".to_string(),
            user_id: "other_user".to_string(),
            text: "theirs matches too".to_string(),
            received_at: at(11, 0),
        })
        .await
        .unwrap();

        let entries = repo
            .search_text("7", "matches", None, None, 10)
            .await
            .unwrap();

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].entry.text, "theirs matches too");
        assert_eq!(entries[1].entry.text, "mine matches");
    }

    #[tokio::test]
    async fn search_text_filters_by_date_range_with_exclusive_end() {
        let repo = setup().await;

        repo.store(&incoming("1", "match before", at_on(2026, 4, 27, 10, 0)))
            .await
            .unwrap();
        repo.store(&incoming("2", "match within", at_on(2026, 4, 28, 10, 0)))
            .await
            .unwrap();
        repo.store(&incoming("3", "match boundary", at_on(2026, 4, 29, 0, 0)))
            .await
            .unwrap();

        let entries = repo
            .search_text(
                "7",
                "match",
                Some(ymd(2026, 4, 28)),
                Some(ymd(2026, 4, 29)),
                10,
            )
            .await
            .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].entry.text, "match within");
    }

    #[tokio::test]
    async fn search_text_returns_empty_when_no_match() {
        let repo = setup().await;

        repo.store(&incoming("1", "calm afternoon", at(10, 0)))
            .await
            .unwrap();

        let entries = repo
            .search_text("7", "anxiety", None, None, 10)
            .await
            .unwrap();

        assert!(entries.is_empty());
    }

    #[tokio::test]
    async fn search_text_ignores_caller_user_id() {
        let repo = setup().await;

        repo.store(&incoming("1", "match", at(10, 0)))
            .await
            .unwrap();

        let entries = repo
            .search_text("unknown", "match", None, None, 10)
            .await
            .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].entry.text, "match");
    }

    #[tokio::test]
    async fn conversations_with_entries_for_date_returns_distinct_source_conversations() {
        let repo = setup().await;

        repo.store(&incoming_for_conversation("42", "1", "first", at(10, 0)))
            .await
            .unwrap();
        repo.store(&incoming_for_conversation("42", "2", "second", at(11, 0)))
            .await
            .unwrap();
        repo.store(&IncomingMessage {
            source: MessageSource::Telegram,
            source_conversation_id: "99".to_string(),
            source_message_id: "3".to_string(),
            user_id: "8".to_string(),
            text: "other user".to_string(),
            received_at: at(12, 0),
        })
        .await
        .unwrap();
        repo.store(&IncomingMessage {
            source: MessageSource::Telegram,
            source_conversation_id: "100".to_string(),
            source_message_id: "4".to_string(),
            user_id: "9".to_string(),
            text: "tomorrow".to_string(),
            received_at: Utc.with_ymd_and_hms(2026, 4, 29, 9, 0, 0).unwrap(),
        })
        .await
        .unwrap();

        let conversations = repo
            .conversations_with_entries_for_date(&MessageSource::Telegram, date())
            .await
            .unwrap();

        assert_eq!(
            conversations,
            vec![
                JournalConversation {
                    user_id: crate::messages::SINGLE_USER_ID.to_string(),
                    source_conversation_id: "42".to_string(),
                },
                JournalConversation {
                    user_id: crate::messages::SINGLE_USER_ID.to_string(),
                    source_conversation_id: "99".to_string(),
                },
            ]
        );
    }

    async fn stored_id(repo: &JournalRepository, source_message_id: &str) -> String {
        sqlx::query_scalar(
            "SELECT id FROM journal_entries WHERE source = 'telegram' AND source_message_id = ?",
        )
        .bind(source_message_id)
        .fetch_one(&repo.pool)
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn fetch_by_ids_returns_entries_matching_ids() {
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

        let first_id = stored_id(&repo, "1").await;
        let third_id = stored_id(&repo, "3").await;

        let rows = repo
            .fetch_by_ids("7", &[first_id.clone(), third_id.clone()])
            .await
            .unwrap();

        let ids: Vec<String> = rows.iter().map(|(id, _)| id.clone()).collect();
        assert_eq!(rows.len(), 2);
        assert!(ids.contains(&first_id));
        assert!(ids.contains(&third_id));
    }

    #[tokio::test]
    async fn fetch_by_ids_returns_all_matching_entries_in_single_user_journal() {
        let repo = setup().await;

        repo.store(&incoming("1", "mine", at(10, 0))).await.unwrap();
        let my_id = stored_id(&repo, "1").await;

        let other = IncomingMessage {
            source: MessageSource::Telegram,
            source_conversation_id: "99".to_string(),
            source_message_id: "2".to_string(),
            user_id: "other_user".to_string(),
            text: "theirs".to_string(),
            received_at: at(11, 0),
        };
        repo.store(&other).await.unwrap();
        let other_id = stored_id(&repo, "2").await;

        let rows = repo
            .fetch_by_ids("7", &[my_id.clone(), other_id.clone()])
            .await
            .unwrap();

        let ids: Vec<String> = rows.iter().map(|(id, _)| id.clone()).collect();
        assert_eq!(rows.len(), 2);
        assert!(ids.contains(&my_id));
        assert!(ids.contains(&other_id));
    }

    #[tokio::test]
    async fn fetch_by_ids_returns_empty_for_empty_id_list() {
        let repo = setup().await;

        let rows = repo.fetch_by_ids("7", &[]).await.unwrap();

        assert!(rows.is_empty());
    }

    #[tokio::test]
    async fn fetch_by_ids_returns_empty_when_no_ids_match() {
        let repo = setup().await;

        let rows = repo
            .fetch_by_ids("7", &["nonexistent".to_string()])
            .await
            .unwrap();

        assert!(rows.is_empty());
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
    async fn stats_returns_zeroes_when_journal_has_no_entries() {
        let repo = setup().await;

        let stats = repo.stats("7", date()).await.unwrap();

        assert_eq!(stats.total_entries, 0);
        assert_eq!(stats.entries_today, 0);
        assert_eq!(stats.latest_received_at, None);
    }
}
