//! Per-user opt-out for daily/weekly review delivery, stored independently.
//!
//! Backed by a singleton row in the tenant's isolated database. Absence of a
//! row means both daily and weekly reviews are enabled — the default for
//! every user.

use sqlx::{Row, SqlitePool};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReviewPreferences {
    pub daily_enabled: bool,
    pub weekly_enabled: bool,
}

impl Default for ReviewPreferences {
    fn default() -> Self {
        Self {
            daily_enabled: true,
            weekly_enabled: true,
        }
    }
}

#[derive(Clone)]
pub struct ReviewPreferenceRepository {
    pool: SqlitePool,
}

impl ReviewPreferenceRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn get(&self) -> Result<ReviewPreferences, sqlx::Error> {
        let row = sqlx::query(
            "SELECT daily_enabled, weekly_enabled FROM review_preferences WHERE id = 1",
        )
        .fetch_optional(&self.pool)
        .await?;

        Ok(match row {
            Some(row) => ReviewPreferences {
                daily_enabled: row.get::<i64, _>("daily_enabled") != 0,
                weekly_enabled: row.get::<i64, _>("weekly_enabled") != 0,
            },
            None => ReviewPreferences::default(),
        })
    }

    pub async fn is_daily_enabled(&self) -> Result<bool, sqlx::Error> {
        Ok(self.get().await?.daily_enabled)
    }

    pub async fn is_weekly_enabled(&self) -> Result<bool, sqlx::Error> {
        Ok(self.get().await?.weekly_enabled)
    }

    pub async fn set_daily_enabled(&self, enabled: bool) -> Result<(), sqlx::Error> {
        self.set_column("daily_enabled", enabled).await
    }

    pub async fn set_weekly_enabled(&self, enabled: bool) -> Result<(), sqlx::Error> {
        self.set_column("weekly_enabled", enabled).await
    }

    async fn set_column(&self, column: &str, enabled: bool) -> Result<(), sqlx::Error> {
        let sql = format!(
            r#"
            INSERT INTO review_preferences (id, {column})
            VALUES (1, ?)
            ON CONFLICT(id) DO UPDATE SET
                {column} = excluded.{column},
                updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
            "#
        );

        sqlx::query(&sql).bind(enabled).execute(&self.pool).await?;

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

        assert_eq!(
            repo.get().await.unwrap(),
            ReviewPreferences {
                daily_enabled: true,
                weekly_enabled: true
            }
        );
    }

    #[tokio::test]
    async fn set_daily_enabled_false_persists_and_is_read_back() {
        let repo = setup().await;

        repo.set_daily_enabled(false).await.unwrap();

        let prefs = repo.get().await.unwrap();
        assert!(!prefs.daily_enabled);
        assert!(prefs.weekly_enabled);
    }

    #[tokio::test]
    async fn set_weekly_enabled_false_persists_and_is_read_back() {
        let repo = setup().await;

        repo.set_weekly_enabled(false).await.unwrap();

        let prefs = repo.get().await.unwrap();
        assert!(prefs.daily_enabled);
        assert!(!prefs.weekly_enabled);
    }

    #[tokio::test]
    async fn disabling_one_review_keeps_the_other_enabled() {
        let repo = setup().await;

        repo.set_daily_enabled(false).await.unwrap();
        assert!(repo.is_weekly_enabled().await.unwrap());

        repo.set_weekly_enabled(false).await.unwrap();
        assert!(!repo.is_daily_enabled().await.unwrap());
    }

    #[tokio::test]
    async fn set_daily_enabled_true_after_disabling_re_enables() {
        let repo = setup().await;
        repo.set_daily_enabled(false).await.unwrap();

        repo.set_daily_enabled(true).await.unwrap();

        assert!(repo.is_daily_enabled().await.unwrap());
    }

    #[tokio::test]
    async fn set_weekly_enabled_true_after_disabling_re_enables() {
        let repo = setup().await;
        repo.set_weekly_enabled(false).await.unwrap();

        repo.set_weekly_enabled(true).await.unwrap();

        assert!(repo.is_weekly_enabled().await.unwrap());
    }

    #[tokio::test]
    async fn setting_one_preference_is_idempotent_and_preserves_the_other() {
        let repo = setup().await;

        repo.set_daily_enabled(false).await.unwrap();
        repo.set_daily_enabled(false).await.unwrap();

        let prefs = repo.get().await.unwrap();
        assert!(!prefs.daily_enabled);
        assert!(prefs.weekly_enabled);
    }

    #[tokio::test]
    async fn turning_one_off_after_the_other_keeps_both_off() {
        let repo = setup().await;

        repo.set_daily_enabled(false).await.unwrap();
        repo.set_weekly_enabled(false).await.unwrap();

        let prefs = repo.get().await.unwrap();
        assert!(!prefs.daily_enabled);
        assert!(!prefs.weekly_enabled);
    }
}
