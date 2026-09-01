//! Per-user opt-out for daily/weekly review delivery.
//!
//! Backed by a singleton row in the tenant's isolated database. Absence of a
//! row means reviews are enabled — the default for every user.

use sqlx::{Row, SqlitePool};

#[derive(Clone)]
pub struct ReviewPreferenceRepository {
    pool: SqlitePool,
}

impl ReviewPreferenceRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn is_enabled(&self) -> Result<bool, sqlx::Error> {
        let row = sqlx::query("SELECT reviews_enabled FROM review_preferences WHERE id = 1")
            .fetch_optional(&self.pool)
            .await?;

        Ok(match row {
            Some(row) => row.get::<i64, _>("reviews_enabled") != 0,
            None => true,
        })
    }

    pub async fn set_enabled(&self, enabled: bool) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"
            INSERT INTO review_preferences (id, reviews_enabled)
            VALUES (1, ?)
            ON CONFLICT(id) DO UPDATE SET
                reviews_enabled = excluded.reviews_enabled,
                updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
            "#,
        )
        .bind(enabled)
        .execute(&self.pool)
        .await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn setup() -> ReviewPreferenceRepository {
        let pool = crate::database::test_pool().await;
        ReviewPreferenceRepository::new(pool)
    }

    #[tokio::test]
    async fn defaults_to_enabled_for_a_fresh_tenant_database() {
        let repo = setup().await;

        assert!(repo.is_enabled().await.unwrap());
    }

    #[tokio::test]
    async fn set_enabled_false_persists_and_is_read_back() {
        let repo = setup().await;

        repo.set_enabled(false).await.unwrap();

        assert!(!repo.is_enabled().await.unwrap());
    }

    #[tokio::test]
    async fn set_enabled_true_after_disabling_re_enables() {
        let repo = setup().await;
        repo.set_enabled(false).await.unwrap();

        repo.set_enabled(true).await.unwrap();

        assert!(repo.is_enabled().await.unwrap());
    }

    #[tokio::test]
    async fn set_enabled_is_idempotent() {
        let repo = setup().await;

        repo.set_enabled(false).await.unwrap();
        repo.set_enabled(false).await.unwrap();

        assert!(!repo.is_enabled().await.unwrap());
    }
}
