use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::prompts::{
    registry::{PromptKey, version_for},
    repository::PromptRepository,
};

/// Effective prompt resolved at invocation time: customized DB row if present,
/// otherwise the on-disk bundled default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPrompt {
    pub version: String,
    pub text: String,
}

/// Resolves prompts by consulting the customization DB first, then falling back
/// to a configured on-disk default path. Cloning is cheap (handle + key + path).
#[derive(Debug, Clone)]
pub struct PromptSource {
    repo: PromptRepository,
    key: PromptKey,
    default_path: PathBuf,
}

impl PromptSource {
    pub fn new(repo: PromptRepository, key: PromptKey, default_path: PathBuf) -> Self {
        Self {
            repo,
            key,
            default_path,
        }
    }

    pub fn key(&self) -> PromptKey {
        self.key
    }

    pub fn default_path(&self) -> &Path {
        &self.default_path
    }

    pub async fn resolve(&self) -> Result<ResolvedPrompt, PromptSourceError> {
        let customized = self.repo.get(self.key.as_str()).await.map_err(|source| {
            PromptSourceError::Database {
                key: self.key.as_str(),
                message: source.to_string(),
            }
        })?;

        if let Some(row) = customized
            && !row.content.trim().is_empty()
        {
            return Ok(ResolvedPrompt {
                version: version_for(&self.default_path, true),
                text: row.content,
            });
        }

        load_default(self.key, &self.default_path)
    }
}

/// Reads the bundled default file at `path` and returns it as a [`ResolvedPrompt`].
/// Returns the bundled default text regardless of any stored customization.
pub fn load_default(key: PromptKey, path: &Path) -> Result<ResolvedPrompt, PromptSourceError> {
    let text = fs::read_to_string(path).map_err(|source| PromptSourceError::ReadFailed {
        key: key.as_str(),
        path: path.to_path_buf(),
        message: source.to_string(),
    })?;

    if text.trim().is_empty() {
        return Err(PromptSourceError::Empty {
            key: key.as_str(),
            path: path.to_path_buf(),
        });
    }

    Ok(ResolvedPrompt {
        version: version_for(path, false),
        text,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PromptSourceError {
    #[error("failed to read customized prompt '{key}': {message}")]
    Database {
        key: &'static str,
        message: String,
    },
    #[error("failed to load default prompt '{key}' from {}: {message}", path.display())]
    ReadFailed {
        key: &'static str,
        path: PathBuf,
        message: String,
    },
    #[error("default prompt '{key}' file is empty: {}", path.display())]
    Empty {
        key: &'static str,
        path: PathBuf,
    },
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    use sqlx::SqlitePool;

    use super::*;
    use crate::database;

    async fn setup() -> PromptRepository {
        database::register_sqlite_vec_extension();
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        PromptRepository::new(pool)
    }

    fn temp_prompt(stem: &str, body: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "froid-source-{stem}-{}.md",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::write(&path, body).unwrap();
        path
    }

    #[tokio::test]
    async fn resolves_to_disk_default_when_no_row_exists() {
        let repo = setup().await;
        let path = temp_prompt("plain", "Default text body.");
        let source = PromptSource::new(repo, PromptKey::DailyReview, path.clone());

        let resolved = source.resolve().await.unwrap();

        let expected_version = path.file_stem().unwrap().to_string_lossy().into_owned();
        assert_eq!(resolved.version, expected_version);
        assert_eq!(resolved.text, "Default text body.");

        fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn resolves_to_db_content_with_custom_version_when_row_present() {
        let repo = setup().await;
        repo.upsert(PromptKey::DailyReview.as_str(), "Custom body.")
            .await
            .unwrap();
        let path = temp_prompt("with-custom", "Default body that should be ignored.");
        let source = PromptSource::new(repo, PromptKey::DailyReview, path.clone());

        let resolved = source.resolve().await.unwrap();

        let expected_version = format!("{}-custom", path.file_stem().unwrap().to_string_lossy());
        assert_eq!(resolved.version, expected_version);
        assert_eq!(resolved.text, "Custom body.");

        fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn falls_back_to_disk_when_db_row_content_is_blank() {
        let repo = setup().await;
        repo.upsert(PromptKey::DailyReview.as_str(), "  \n")
            .await
            .unwrap();
        let path = temp_prompt("blank-row", "Default body.");
        let source = PromptSource::new(repo, PromptKey::DailyReview, path.clone());

        let resolved = source.resolve().await.unwrap();

        assert_eq!(resolved.text, "Default body.");
        assert!(!resolved.version.ends_with("-custom"));

        fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn errors_when_no_row_and_default_file_missing() {
        let repo = setup().await;
        let source = PromptSource::new(
            repo,
            PromptKey::DailyReview,
            PathBuf::from("/nonexistent/froid-source-missing.md"),
        );

        let err = source.resolve().await.unwrap_err();
        assert!(matches!(err, PromptSourceError::ReadFailed { .. }));
    }
}
