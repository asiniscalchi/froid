use std::{error::Error, fmt};

use chrono::{DateTime, Utc};
use sqlx::{Row, SqlitePool, sqlite::SqliteRow};

use super::{JournalEntryExtraction, JournalEntryExtractionStatus};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalEntryExtractionCandidate {
    pub journal_entry_id: i64,
    pub raw_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JournalEntryExtractionRepositoryError {
    Storage(String),
    InvalidStatus(String),
}

impl fmt::Display for JournalEntryExtractionRepositoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Storage(message) => write!(f, "{message}"),
            Self::InvalidStatus(value) => {
                write!(
                    f,
                    "invalid journal entry extraction status stored in database: {value}"
                )
            }
        }
    }
}

impl Error for JournalEntryExtractionRepositoryError {}

impl From<sqlx::Error> for JournalEntryExtractionRepositoryError {
    fn from(error: sqlx::Error) -> Self {
        Self::Storage(error.to_string())
    }
}

#[derive(Debug, Clone)]
pub struct JournalEntryExtractionRepository {
    pool: SqlitePool,
}

impl JournalEntryExtractionRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn find_by_journal_entry_id(
        &self,
        journal_entry_id: i64,
    ) -> Result<Option<JournalEntryExtraction>, JournalEntryExtractionRepositoryError> {
        let row = sqlx::query(
            r#"
            SELECT id, journal_entry_id, extraction_json, model, prompt_version, status,
                   error_message, created_at, updated_at
            FROM journal_entry_extractions
            WHERE journal_entry_id = ?
            "#,
        )
        .bind(journal_entry_id)
        .fetch_optional(&self.pool)
        .await?;

        row.map(row_to_extraction).transpose()
    }

    pub async fn insert_pending_if_absent(
        &self,
        journal_entry_id: i64,
        model: &str,
        prompt_version: &str,
    ) -> Result<bool, JournalEntryExtractionRepositoryError> {
        let result = sqlx::query(
            r#"
            INSERT OR IGNORE INTO journal_entry_extractions
                (journal_entry_id, extraction_json, model, prompt_version, status, error_message)
            VALUES (?, NULL, ?, ?, 'pending', NULL)
            "#,
        )
        .bind(journal_entry_id)
        .bind(model)
        .bind(prompt_version)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected() != 0)
    }

    pub async fn find_entries_missing_extraction(
        &self,
        limit: u32,
    ) -> Result<Vec<JournalEntryExtractionCandidate>, JournalEntryExtractionRepositoryError> {
        let rows = sqlx::query(
            r#"
            SELECT journal_entries.id, journal_entries.raw_text
            FROM journal_entries
            LEFT JOIN journal_entry_extractions
              ON journal_entry_extractions.journal_entry_id = journal_entries.id
            WHERE journal_entry_extractions.id IS NULL
            ORDER BY journal_entries.received_at ASC, journal_entries.id ASC
            LIMIT ?
            "#,
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| JournalEntryExtractionCandidate {
                journal_entry_id: row.get("id"),
                raw_text: row.get("raw_text"),
            })
            .collect())
    }

    pub async fn mark_completed(
        &self,
        journal_entry_id: i64,
        extraction_json: &str,
        model: &str,
        prompt_version: &str,
    ) -> Result<(), JournalEntryExtractionRepositoryError> {
        sqlx::query(
            r#"
            UPDATE journal_entry_extractions
            SET extraction_json = ?,
                model = ?,
                prompt_version = ?,
                status = 'completed',
                error_message = NULL,
                updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
            WHERE journal_entry_id = ?
            "#,
        )
        .bind(extraction_json)
        .bind(model)
        .bind(prompt_version)
        .bind(journal_entry_id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn mark_failed(
        &self,
        journal_entry_id: i64,
        model: &str,
        prompt_version: &str,
        error_message: &str,
    ) -> Result<(), JournalEntryExtractionRepositoryError> {
        sqlx::query(
            r#"
            UPDATE journal_entry_extractions
            SET extraction_json = NULL,
                model = ?,
                prompt_version = ?,
                status = 'failed',
                error_message = ?,
                updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
            WHERE journal_entry_id = ?
            "#,
        )
        .bind(model)
        .bind(prompt_version)
        .bind(error_message)
        .bind(journal_entry_id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }
}

fn row_to_extraction(
    row: SqliteRow,
) -> Result<JournalEntryExtraction, JournalEntryExtractionRepositoryError> {
    let status = row.get::<String, _>("status");
    let status = match status.as_str() {
        "pending" => JournalEntryExtractionStatus::Pending,
        "completed" => JournalEntryExtractionStatus::Completed,
        "failed" => JournalEntryExtractionStatus::Failed,
        _ => return Err(JournalEntryExtractionRepositoryError::InvalidStatus(status)),
    };

    Ok(JournalEntryExtraction {
        id: row.get("id"),
        journal_entry_id: row.get("journal_entry_id"),
        extraction_json: row.get("extraction_json"),
        model: row.get("model"),
        prompt_version: row.get("prompt_version"),
        status,
        error_message: row.get("error_message"),
        created_at: row.get::<DateTime<Utc>, _>("created_at"),
        updated_at: row.get::<DateTime<Utc>, _>("updated_at"),
    })
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use sqlx::SqlitePool;

    use super::*;
    use crate::{
        database,
        journal::extraction::JournalEntryExtractionStatus,
        messages::{IncomingMessage, MessageSource},
    };

    async fn setup() -> (JournalEntryExtractionRepository, SqlitePool) {
        database::register_sqlite_vec_extension();
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        (JournalEntryExtractionRepository::new(pool.clone()), pool)
    }

    async fn insert_entry(pool: &SqlitePool) -> i64 {
        insert_entry_with_message_id(pool, "100", "hello froid", chrono::Utc::now()).await
    }

    async fn insert_entry_with_message_id(
        pool: &SqlitePool,
        source_message_id: &str,
        text: &str,
        received_at: chrono::DateTime<chrono::Utc>,
    ) -> i64 {
        let message = IncomingMessage {
            source: MessageSource::Telegram,
            source_conversation_id: "42".to_string(),
            source_message_id: source_message_id.to_string(),
            user_id: "7".to_string(),
            text: text.to_string(),
            received_at,
        };

        crate::journal::repository::JournalRepository::new(pool.clone())
            .store(&message)
            .await
            .unwrap()
            .unwrap()
    }

    fn at(h: u32) -> chrono::DateTime<chrono::Utc> {
        chrono::Utc.with_ymd_and_hms(2026, 4, 28, h, 0, 0).unwrap()
    }

    #[tokio::test]
    async fn insert_pending_if_absent_prevents_duplicates() {
        let (repo, pool) = setup().await;
        let entry_id = insert_entry(&pool).await;

        let first = repo
            .insert_pending_if_absent(entry_id, "model-a", "entry_extraction_v1")
            .await
            .unwrap();
        let second = repo
            .insert_pending_if_absent(entry_id, "model-a", "entry_extraction_v1")
            .await
            .unwrap();
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM journal_entry_extractions")
            .fetch_one(&pool)
            .await
            .unwrap();

        assert!(first);
        assert!(!second);
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn finds_entries_missing_extraction_oldest_first_with_limit() {
        let (repo, pool) = setup().await;
        let first = insert_entry_with_message_id(&pool, "1", "first", at(10)).await;
        let second = insert_entry_with_message_id(&pool, "2", "second", at(11)).await;
        insert_entry_with_message_id(&pool, "3", "third", at(12)).await;
        repo.insert_pending_if_absent(first, "model-a", "entry_extraction_v1")
            .await
            .unwrap();

        let candidates = repo.find_entries_missing_extraction(1).await.unwrap();

        assert_eq!(
            candidates,
            vec![JournalEntryExtractionCandidate {
                journal_entry_id: second,
                raw_text: "second".to_string()
            }]
        );
    }

    #[tokio::test]
    async fn mark_completed_stores_valid_json_metadata_and_status() {
        let (repo, pool) = setup().await;
        let entry_id = insert_entry(&pool).await;
        repo.insert_pending_if_absent(entry_id, "model-a", "entry_extraction_v1")
            .await
            .unwrap();

        repo.mark_completed(
            entry_id,
            r#"{"summary":"Saved","domains":[],"emotions":[],"behaviors":[],"needs":[],"possible_patterns":[]}"#,
            "model-b",
            "entry_extraction_v2",
        )
        .await
        .unwrap();

        let extraction = repo
            .find_by_journal_entry_id(entry_id)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(extraction.status, JournalEntryExtractionStatus::Completed);
        assert_eq!(extraction.model, "model-b");
        assert_eq!(extraction.prompt_version, "entry_extraction_v2");
        assert!(extraction.extraction_json.unwrap().contains("\"summary\""));
        assert_eq!(extraction.error_message, None);
    }

    #[tokio::test]
    async fn mark_failed_records_error_without_json() {
        let (repo, pool) = setup().await;
        let entry_id = insert_entry(&pool).await;
        repo.insert_pending_if_absent(entry_id, "model-a", "entry_extraction_v1")
            .await
            .unwrap();

        repo.mark_failed(entry_id, "model-a", "entry_extraction_v1", "provider down")
            .await
            .unwrap();

        let extraction = repo
            .find_by_journal_entry_id(entry_id)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(extraction.status, JournalEntryExtractionStatus::Failed);
        assert_eq!(extraction.extraction_json, None);
        assert_eq!(extraction.error_message, Some("provider down".to_string()));
    }

    #[tokio::test]
    async fn deleting_journal_entry_cascades_to_extraction() {
        let (repo, pool) = setup().await;
        let entry_id = insert_entry(&pool).await;
        repo.insert_pending_if_absent(entry_id, "model-a", "entry_extraction_v1")
            .await
            .unwrap();

        sqlx::query("DELETE FROM journal_entries WHERE id = ?")
            .bind(entry_id)
            .execute(&pool)
            .await
            .unwrap();

        let extraction = repo.find_by_journal_entry_id(entry_id).await.unwrap();
        assert!(extraction.is_none());
    }
}
