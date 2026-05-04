use std::env;

use sqlx::SqlitePool;
use tracing::warn;

use crate::journal::{
    extraction::repository::JournalEntryExtractionRepository,
    repository::JournalRepository,
    review::{
        DailyReviewPromptConfig, ReviewConfig, RigOpenAiReviewGenerator,
        repository::DailyReviewRepository, service::DailyReviewService,
    },
    service::JournalService,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DailyReviewRuntimeConfig {
    pub openai_api_key: Option<String>,
    pub review: ReviewConfig,
    pub prompt: DailyReviewPromptConfig,
}

impl DailyReviewRuntimeConfig {
    pub fn from_env() -> Self {
        Self {
            openai_api_key: env::var("OPENAI_API_KEY").ok(),
            review: ReviewConfig::from_env(),
            prompt: DailyReviewPromptConfig::from_env(),
        }
    }
}

pub fn configure_daily_review(
    journal_service: JournalService,
    pool: SqlitePool,
    config: DailyReviewRuntimeConfig,
) -> Result<JournalService, Box<dyn std::error::Error>> {
    let prompt_version = config.prompt.version.clone();
    let Some(daily_review_service) = build_daily_review_service(pool, config)? else {
        return Ok(journal_service);
    };

    Ok(journal_service
        .with_daily_review_runner(daily_review_service)
        .with_daily_review_prompt_version(prompt_version))
}

pub fn build_daily_review_service(
    pool: SqlitePool,
    config: DailyReviewRuntimeConfig,
) -> Result<Option<DailyReviewService>, Box<dyn std::error::Error>> {
    let Some(openai_api_key) = config
        .openai_api_key
        .filter(|value| !value.trim().is_empty())
    else {
        warn!("daily review generation is not configured");
        return Ok(None);
    };

    let review_prompt = config.prompt.load()?;
    let review_generator = RigOpenAiReviewGenerator::from_optional_api_key(
        config.review,
        review_prompt,
        Some(openai_api_key),
    )?;
    let daily_review_service = DailyReviewService::new(
        DailyReviewRepository::new(pool.clone()),
        JournalRepository::new(pool.clone()),
        JournalEntryExtractionRepository::new(pool),
        review_generator,
    );

    Ok(Some(daily_review_service))
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    use chrono::{Duration, Utc};
    use sqlx::SqlitePool;

    use super::*;
    use crate::{
        database,
        journal::{
            command::{JournalCommand, JournalCommandRequest},
            review::prompt::DEFAULT_REVIEW_PROMPT_PATH,
        },
        messages::MessageSource,
    };

    #[tokio::test]
    async fn missing_prompt_file_does_not_break_startup_without_review_api_key() {
        let pool = setup_pool().await;
        let service = configure_daily_review(
            JournalService::new(JournalRepository::new(pool.clone())),
            pool,
            DailyReviewRuntimeConfig {
                openai_api_key: None,
                review: ReviewConfig::default(),
                prompt: DailyReviewPromptConfig {
                    path: PathBuf::from("missing-review-prompt.md"),
                    version: "v1".to_string(),
                },
            },
        )
        .unwrap();

        let response = service
            .command(&JournalCommandRequest {
                source: MessageSource::Telegram,
                source_conversation_id: "42".to_string(),
                user_id: "7".to_string(),
                received_at: Utc::now(),
                command: JournalCommand::DayReviewLast,
            })
            .await
            .unwrap();

        assert_eq!(
            response.text,
            "Daily review generation is not configured yet."
        );
    }

    #[tokio::test]
    async fn missing_prompt_file_fails_startup_when_review_api_key_is_configured() {
        let pool = setup_pool().await;
        let error = configure_daily_review(
            JournalService::new(JournalRepository::new(pool.clone())),
            pool,
            DailyReviewRuntimeConfig {
                openai_api_key: Some("test-api-key".to_string()),
                review: ReviewConfig::default(),
                prompt: DailyReviewPromptConfig {
                    path: PathBuf::from("missing-review-prompt.md"),
                    version: "v1".to_string(),
                },
            },
        )
        .err()
        .unwrap();

        assert!(
            error
                .to_string()
                .contains("failed to load daily review prompt")
        );
    }

    #[tokio::test]
    async fn default_prompt_file_allows_startup_when_review_api_key_is_configured() {
        let pool = setup_pool().await;
        configure_daily_review(
            JournalService::new(JournalRepository::new(pool.clone())),
            pool,
            DailyReviewRuntimeConfig {
                openai_api_key: Some("test-api-key".to_string()),
                review: ReviewConfig::default(),
                prompt: DailyReviewPromptConfig {
                    path: PathBuf::from(DEFAULT_REVIEW_PROMPT_PATH),
                    version: "v1".to_string(),
                },
            },
        )
        .unwrap();
    }

    #[tokio::test]
    async fn configured_prompt_version_is_used_by_review_service() {
        let prompt_path = temp_prompt_path("configured-version");
        fs::write(&prompt_path, "Prompt text").unwrap();
        let pool = setup_pool().await;

        let service = configure_daily_review(
            JournalService::new(JournalRepository::new(pool.clone())),
            pool,
            DailyReviewRuntimeConfig {
                openai_api_key: Some("test-api-key".to_string()),
                review: ReviewConfig::default(),
                prompt: DailyReviewPromptConfig {
                    path: prompt_path.clone(),
                    version: "custom-version".to_string(),
                },
            },
        )
        .unwrap();

        let response = service
            .command(&JournalCommandRequest {
                source: MessageSource::Telegram,
                source_conversation_id: "42".to_string(),
                user_id: "7".to_string(),
                received_at: Utc::now(),
                command: JournalCommand::DayReviewLast,
            })
            .await
            .unwrap();

        let yesterday = Utc::now().date_naive() - Duration::days(1);
        assert_eq!(
            response.text,
            format!(
                "No daily review available for {} yet.",
                yesterday.format("%Y-%m-%d")
            )
        );

        fs::remove_file(prompt_path).unwrap();
    }

    #[tokio::test]
    async fn configured_prompt_version_is_exposed_to_status() {
        let prompt_path = temp_prompt_path("status-configured-version");
        fs::write(&prompt_path, "Prompt text").unwrap();
        let pool = setup_pool().await;

        let service = configure_daily_review(
            JournalService::new(JournalRepository::new(pool.clone())),
            pool,
            DailyReviewRuntimeConfig {
                openai_api_key: Some("test-api-key".to_string()),
                review: ReviewConfig::default(),
                prompt: DailyReviewPromptConfig {
                    path: prompt_path.clone(),
                    version: "daily-review-v-test".to_string(),
                },
            },
        )
        .unwrap();

        let response = service
            .command(&JournalCommandRequest {
                source: MessageSource::Telegram,
                source_conversation_id: "42".to_string(),
                user_id: "7".to_string(),
                received_at: Utc::now(),
                command: JournalCommand::Status,
            })
            .await
            .unwrap();

        assert!(response.text.contains("- Generation: configured"));
        assert!(response.text.contains("- Prompt: daily-review-v-test"));

        fs::remove_file(prompt_path).unwrap();
    }

    async fn setup_pool() -> SqlitePool {
        database::register_sqlite_vec_extension();
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        pool
    }

    fn temp_prompt_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "froid-{name}-{}.md",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }
}
