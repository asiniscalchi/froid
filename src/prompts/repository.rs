use chrono::{DateTime, Utc};
use sqlx::{Row, SqlitePool};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomizedPrompt {
    pub prompt_key: String,
    pub content: String,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct PromptRepository {
    pool: SqlitePool,
}

impl PromptRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn get(&self, prompt_key: &str) -> Result<Option<CustomizedPrompt>, sqlx::Error> {
        let row = sqlx::query(
            r#"
            SELECT prompt_key, content, updated_at
            FROM customized_prompts
            WHERE prompt_key = ?
            "#,
        )
        .bind(prompt_key)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|row| CustomizedPrompt {
            prompt_key: row.get("prompt_key"),
            content: row.get("content"),
            updated_at: row.get("updated_at"),
        }))
    }

    pub async fn list_all(&self) -> Result<Vec<CustomizedPrompt>, sqlx::Error> {
        let rows = sqlx::query(
            r#"
            SELECT prompt_key, content, updated_at
            FROM customized_prompts
            ORDER BY prompt_key ASC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| CustomizedPrompt {
                prompt_key: row.get("prompt_key"),
                content: row.get("content"),
                updated_at: row.get("updated_at"),
            })
            .collect())
    }

    pub async fn upsert(
        &self,
        prompt_key: &str,
        content: &str,
    ) -> Result<CustomizedPrompt, sqlx::Error> {
        sqlx::query(
            r#"
            INSERT INTO customized_prompts (prompt_key, content)
            VALUES (?, ?)
            ON CONFLICT(prompt_key) DO UPDATE SET
                content = excluded.content,
                updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
            "#,
        )
        .bind(prompt_key)
        .bind(content)
        .execute(&self.pool)
        .await?;

        self.get(prompt_key)
            .await?
            .ok_or_else(|| sqlx::Error::RowNotFound)
    }

    pub async fn delete(&self, prompt_key: &str) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            r#"
            DELETE FROM customized_prompts
            WHERE prompt_key = ?
            "#,
        )
        .bind(prompt_key)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected() > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn setup() -> PromptRepository {
        let pool = crate::database::test_pool().await;
        PromptRepository::new(pool)
    }

    #[tokio::test]
    async fn get_returns_none_for_unknown_key() {
        let repo = setup().await;
        assert!(repo.get("missing").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn upsert_inserts_then_updates_content() {
        let repo = setup().await;

        let first = repo.upsert("daily_review", "first text").await.unwrap();
        assert_eq!(first.prompt_key, "daily_review");
        assert_eq!(first.content, "first text");

        let second = repo.upsert("daily_review", "second text").await.unwrap();
        assert_eq!(second.content, "second text");
        assert!(second.updated_at >= first.updated_at);

        let fetched = repo.get("daily_review").await.unwrap().unwrap();
        assert_eq!(fetched.content, "second text");
    }

    #[tokio::test]
    async fn list_all_returns_rows_sorted_by_key() {
        let repo = setup().await;
        repo.upsert("weekly_review", "w").await.unwrap();
        repo.upsert("daily_review", "d").await.unwrap();
        repo.upsert("entry_extraction", "e").await.unwrap();

        let rows = repo.list_all().await.unwrap();
        let keys: Vec<_> = rows.iter().map(|r| r.prompt_key.as_str()).collect();
        assert_eq!(
            keys,
            vec!["daily_review", "entry_extraction", "weekly_review"]
        );
    }

    #[tokio::test]
    async fn delete_removes_row_and_reports_whether_present() {
        let repo = setup().await;
        repo.upsert("daily_review", "hello").await.unwrap();

        assert!(repo.delete("daily_review").await.unwrap());
        assert!(repo.get("daily_review").await.unwrap().is_none());
        assert!(!repo.delete("daily_review").await.unwrap());
    }
}
