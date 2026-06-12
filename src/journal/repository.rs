use std::sync::{Arc, Mutex};

use chrono::{Duration, NaiveDate, TimeZone, Utc};
use sqlx::{Row, SqlitePool, sqlite::SqliteRow};

use crate::messages::{IncomingMessage, MessageSource};

use super::entry::{JournalEntry, JournalStats, StoredJournalEntry};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalConversation {
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

#[derive(Clone)]
pub struct JournalRepository {
    pool: SqlitePool,
    id_generator: Arc<Mutex<ulid::Generator>>,
}

impl std::fmt::Debug for JournalRepository {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JournalRepository").finish_non_exhaustive()
    }
}

impl JournalRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self {
            pool,
            id_generator: Arc::new(Mutex::new(ulid::Generator::new())),
        }
    }

    pub(crate) fn clone_pool(&self) -> SqlitePool {
        self.pool.clone()
    }

    #[cfg(test)]
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    fn next_id(&self) -> String {
        self.id_generator
            .lock()
            .unwrap()
            .generate()
            .expect("ULID monotonic counter exhausted within a single millisecond")
            .to_string()
    }

    pub async fn store(&self, message: &IncomingMessage) -> Result<Option<String>, sqlx::Error> {
        let id = self.next_id();
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

    pub async fn fetch_recent(&self, limit: u32) -> Result<Vec<StoredJournalEntry>, sqlx::Error> {
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
                source_conversation_id: row.get("source_conversation_id"),
            })
            .collect())
    }

    pub async fn fetch_by_ids(
        &self,
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

    pub async fn stats(&self, today: NaiveDate) -> Result<JournalStats, sqlx::Error> {
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
