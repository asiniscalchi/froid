//! Runtime model selection for the daily and weekly review generators,
//! controlled through the Telegram `/model` command.
//!
//! The bot's environment decides the *default* model for each review kind
//! (`FROID_REVIEW_MODEL` / `FROID_WEEK_REVIEW_MODEL`). An override set via
//! `/model` takes precedence at generation time, survives restarts (it is
//! persisted in the central database), and resetting removes the override so
//! the env-configured default applies again. Overrides live in a shared,
//! cloneable handle so every generator built for any tenant resolves the
//! current model on each generation — no restart required.

use std::sync::{Arc, RwLock};

use sqlx::SqlitePool;

/// Which review a model override applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewKind {
    Daily,
    Weekly,
}

impl ReviewKind {
    /// Parse the `daily`/`weekly` keyword used in `/model` arguments.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "daily" => Some(Self::Daily),
            "weekly" => Some(Self::Weekly),
            _ => None,
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::Daily => "daily",
            Self::Weekly => "weekly",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Daily => "Daily review model",
            Self::Weekly => "Weekly review model",
        }
    }
}

/// Shared handle holding the current model override (if any) for one review
/// kind. Clones share state; generators resolve the model per call so a new
/// override takes effect without restarting.
#[derive(Clone, Default)]
pub struct ModelOverride(Arc<RwLock<Option<String>>>);

impl ModelOverride {
    /// The effective model: the override when set, otherwise `default`.
    pub fn resolve(&self, default: &str) -> String {
        self.0
            .read()
            .unwrap()
            .clone()
            .unwrap_or_else(|| default.to_string())
    }

    pub fn get(&self) -> Option<String> {
        self.0.read().unwrap().clone()
    }

    pub fn set(&self, model: impl Into<String>) {
        *self.0.write().unwrap() = Some(model.into());
    }

    pub fn reset(&self) {
        *self.0.write().unwrap() = None;
    }
}

impl std::fmt::Debug for ModelOverride {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.get() {
            Some(model) => write!(f, "ModelOverride({model})"),
            None => write!(f, "ModelOverride(unset)"),
        }
    }
}

impl PartialEq for ModelOverride {
    fn eq(&self, other: &Self) -> bool {
        self.get() == other.get()
    }
}

impl Eq for ModelOverride {}

/// Persistence and shared state for both review model overrides. The handles
/// are distributed to the generators; this store keeps the central database in
/// sync so overrides survive a restart.
#[derive(Clone)]
pub struct ReviewModelSettings {
    pool: SqlitePool,
    daily: ModelOverride,
    weekly: ModelOverride,
}

impl ReviewModelSettings {
    pub fn new(pool: SqlitePool) -> Self {
        Self {
            pool,
            daily: ModelOverride::default(),
            weekly: ModelOverride::default(),
        }
    }

    /// Load persisted overrides into the in-memory handles. Called once at
    /// startup so an override survives a restart.
    pub async fn load(&self) -> Result<(), sqlx::Error> {
        let rows: Vec<(String, String)> =
            sqlx::query_as("SELECT review_kind, model FROM review_model_settings")
                .fetch_all(&self.pool)
                .await?;

        for (kind, model) in rows {
            match ReviewKind::parse(&kind) {
                Some(ReviewKind::Daily) => self.daily.set(model),
                Some(ReviewKind::Weekly) => self.weekly.set(model),
                None => continue,
            }
        }
        Ok(())
    }

    pub fn handle(&self, kind: ReviewKind) -> ModelOverride {
        match kind {
            ReviewKind::Daily => self.daily.clone(),
            ReviewKind::Weekly => self.weekly.clone(),
        }
    }

    /// Persist a model override and apply it immediately.
    pub async fn set(&self, kind: ReviewKind, model: &str) -> Result<(), sqlx::Error> {
        let model = model.trim();
        sqlx::query(
            r#"
            INSERT INTO review_model_settings (review_kind, model)
            VALUES (?, ?)
            ON CONFLICT (review_kind) DO UPDATE SET
                model = excluded.model,
                updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
            "#,
        )
        .bind(kind.key())
        .bind(model)
        .execute(&self.pool)
        .await?;

        self.handle(kind).set(model);
        Ok(())
    }

    /// Remove a model override so the env-configured default applies again.
    pub async fn reset(&self, kind: ReviewKind) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM review_model_settings WHERE review_kind = ?")
            .bind(kind.key())
            .execute(&self.pool)
            .await?;

        self.handle(kind).reset();
        Ok(())
    }
}

/// Argument parsed out of a `/model` command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelAction {
    Show,
    Set { kind: ReviewKind, model: String },
    Reset { kind: ReviewKind },
    Usage,
}

/// Renders replies for the `/model` command and applies changes.
#[derive(Clone)]
pub struct ModelCommandHandler {
    settings: ReviewModelSettings,
    default_daily: String,
    default_weekly: String,
}

impl ModelCommandHandler {
    pub fn new(
        settings: ReviewModelSettings,
        default_daily: impl Into<String>,
        default_weekly: impl Into<String>,
    ) -> Self {
        Self {
            settings,
            default_daily: default_daily.into(),
            default_weekly: default_weekly.into(),
        }
    }

    pub async fn handle(&self, action: ModelAction) -> String {
        match action {
            ModelAction::Show => self.show(),
            ModelAction::Set { kind, model } => self.set(kind, model).await,
            ModelAction::Reset { kind } => self.reset(kind).await,
            ModelAction::Usage => usage_text(),
        }
    }

    fn show(&self) -> String {
        let daily = self.settings.daily.get();
        let weekly = self.settings.weekly.get();
        format!(
            "{}: {}{}\n{}: {}{}\n\n{}",
            ReviewKind::Daily.label(),
            daily.as_deref().unwrap_or(&self.default_daily),
            if daily.is_some() {
                " (custom)"
            } else {
                " (default)"
            },
            ReviewKind::Weekly.label(),
            weekly.as_deref().unwrap_or(&self.default_weekly),
            if weekly.is_some() {
                " (custom)"
            } else {
                " (default)"
            },
            usage_text()
        )
    }

    async fn set(&self, kind: ReviewKind, model: String) -> String {
        let model = model.trim();
        if model.is_empty() {
            return usage_text();
        }

        match self.settings.set(kind, model).await {
            Ok(()) => format!(
                "{} set to {model}. The change applies to the next generated review.",
                kind.label()
            ),
            Err(err) => {
                tracing::error!(%err, "failed to persist review model override");
                "Something went wrong changing the review model. Please try again.".to_string()
            }
        }
    }

    async fn reset(&self, kind: ReviewKind) -> String {
        match self.settings.reset(kind).await {
            Ok(()) => format!(
                "{} model reset to the default ({}).",
                kind.label(),
                self.default_for(kind)
            ),
            Err(err) => {
                tracing::error!(%err, "failed to reset review model override");
                "Something went wrong resetting the review model. Please try again.".to_string()
            }
        }
    }

    fn default_for(&self, kind: ReviewKind) -> &str {
        match kind {
            ReviewKind::Daily => &self.default_daily,
            ReviewKind::Weekly => &self.default_weekly,
        }
    }
}

fn usage_text() -> String {
    "Usage:\n\
     /model — show the current daily and weekly review models\n\
     /model daily <model> — set the daily review model\n\
     /model weekly <model> — set the weekly review model\n\
     /model daily default — reset the daily review model to the default\n\
     /model weekly default — reset the weekly review model to the default"
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn settings() -> ReviewModelSettings {
        let pool = crate::database::test_pool().await;
        ReviewModelSettings::new(pool)
    }

    fn handler(settings: &ReviewModelSettings) -> ModelCommandHandler {
        ModelCommandHandler::new(settings.clone(), "env-daily", "env-weekly")
    }

    #[test]
    fn review_kind_parses_keywords() {
        assert_eq!(ReviewKind::parse("daily"), Some(ReviewKind::Daily));
        assert_eq!(ReviewKind::parse("weekly"), Some(ReviewKind::Weekly));
        assert_eq!(ReviewKind::parse("monthly"), None);
        assert_eq!(ReviewKind::parse(""), None);
    }

    #[test]
    fn override_resolves_to_default_when_unset() {
        let override_ = ModelOverride::default();

        assert_eq!(override_.resolve("gpt-5"), "gpt-5");
        assert_eq!(override_.get(), None);
    }

    #[test]
    fn override_set_and_reset_share_state_across_clones() {
        let override_ = ModelOverride::default();
        let clone = override_.clone();

        override_.set("gpt-5");
        assert_eq!(clone.resolve("fallback"), "gpt-5");

        clone.reset();
        assert_eq!(override_.resolve("fallback"), "fallback");
    }

    #[tokio::test]
    async fn set_persists_and_applies_immediately() {
        let settings = settings().await;

        settings.set(ReviewKind::Daily, "gpt-5").await.unwrap();

        assert_eq!(
            settings.handle(ReviewKind::Daily).get(),
            Some("gpt-5".to_string())
        );
        let stored: Vec<(String, String)> =
            sqlx::query_as("SELECT review_kind, model FROM review_model_settings")
                .fetch_all(&settings.pool)
                .await
                .unwrap();
        assert_eq!(stored, vec![("daily".to_string(), "gpt-5".to_string())]);
    }

    #[tokio::test]
    async fn reset_removes_override() {
        let settings = settings().await;
        settings.set(ReviewKind::Weekly, "my-model").await.unwrap();

        settings.reset(ReviewKind::Weekly).await.unwrap();

        assert_eq!(settings.handle(ReviewKind::Weekly).get(), None);
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM review_model_settings")
            .fetch_one(&settings.pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn load_restores_persisted_overrides() {
        let settings = settings().await;
        settings.set(ReviewKind::Daily, "gpt-5").await.unwrap();
        settings
            .set(ReviewKind::Weekly, "cheap-model")
            .await
            .unwrap();

        // A fresh instance simulates a restart.
        let restarted = ReviewModelSettings::new(settings.pool.clone());
        assert_eq!(restarted.handle(ReviewKind::Daily).get(), None);

        restarted.load().await.unwrap();

        assert_eq!(
            restarted.handle(ReviewKind::Daily).get(),
            Some("gpt-5".to_string())
        );
        assert_eq!(
            restarted.handle(ReviewKind::Weekly).get(),
            Some("cheap-model".to_string())
        );
    }

    #[tokio::test]
    async fn show_reports_effective_models() {
        let settings = settings().await;
        let handler = handler(&settings);

        let text = handler.handle(ModelAction::Show).await;
        assert!(text.contains("Daily review model: env-daily (default)"));
        assert!(text.contains("Weekly review model: env-weekly (default)"));

        settings.set(ReviewKind::Daily, "gpt-5").await.unwrap();
        let text = handler.handle(ModelAction::Show).await;
        assert!(text.contains("Daily review model: gpt-5 (custom)"));
        assert!(text.contains("Weekly review model: env-weekly (default)"));
    }

    #[tokio::test]
    async fn set_and_reset_reply_with_confirmation() {
        let settings = settings().await;
        let handler = handler(&settings);

        let text = handler
            .handle(ModelAction::Set {
                kind: ReviewKind::Weekly,
                model: "  gpt-5-nano  ".to_string(),
            })
            .await;
        assert!(text.contains("Weekly review model set to gpt-5-nano"));
        assert_eq!(
            settings.handle(ReviewKind::Weekly).resolve("env-weekly"),
            "gpt-5-nano"
        );

        let text = handler
            .handle(ModelAction::Reset {
                kind: ReviewKind::Weekly,
            })
            .await;
        assert!(text.contains("reset to the default (env-weekly)"));
        assert_eq!(
            handler
                .settings
                .handle(ReviewKind::Weekly)
                .resolve("env-weekly"),
            "env-weekly"
        );
    }

    #[tokio::test]
    async fn blank_model_gives_usage_text() {
        let settings = settings().await;
        let handler = handler(&settings);

        let text = handler
            .handle(ModelAction::Set {
                kind: ReviewKind::Daily,
                model: "   ".to_string(),
            })
            .await;
        assert_eq!(text, usage_text());
    }
}
