//! Loading of bundled on-disk prompt files.
//!
//! Every LLM-facing module ships a default prompt as a markdown file whose
//! file stem doubles as the prompt version. The per-module prompt configs
//! (daily review, weekly review, signal extraction, entry extraction) all
//! resolve and load their files through these helpers.

use std::path::{Path, PathBuf};

use thiserror::Error;

/// A prompt loaded from disk: version derived from the file stem plus text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptFile {
    pub version: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PromptFileError {
    #[error("failed to load {kind} prompt from {}: {message}", path.display())]
    ReadFailed {
        kind: &'static str,
        path: PathBuf,
        message: String,
    },
    #[error("{kind} prompt file is empty: {}", path.display())]
    Empty { kind: &'static str, path: PathBuf },
}

/// Resolve a prompt path override, falling back to `default_path` when the
/// override is unset or blank.
pub(crate) fn resolve_path(overridden: Option<String>, default_path: &str) -> PathBuf {
    overridden
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(default_path))
}

/// Read the prompt file at `path`, deriving its version from the file stem.
/// `kind` names the prompt in error messages (e.g. "daily review").
pub(crate) fn load(kind: &'static str, path: &Path) -> Result<PromptFile, PromptFileError> {
    let text = std::fs::read_to_string(path).map_err(|source| PromptFileError::ReadFailed {
        kind,
        path: path.to_path_buf(),
        message: source.to_string(),
    })?;

    if text.trim().is_empty() {
        return Err(PromptFileError::Empty {
            kind,
            path: path.to_path_buf(),
        });
    }

    let version = path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();

    Ok(PromptFile { version, text })
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::*;

    fn temp_prompt_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "froid-{name}-{}.md",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn resolve_path_uses_default_when_unset_or_blank() {
        assert_eq!(
            resolve_path(None, "default.md"),
            PathBuf::from("default.md")
        );
        assert_eq!(
            resolve_path(Some("  ".to_string()), "default.md"),
            PathBuf::from("default.md")
        );
    }

    #[test]
    fn resolve_path_accepts_override() {
        assert_eq!(
            resolve_path(Some("custom.md".to_string()), "default.md"),
            PathBuf::from("custom.md")
        );
    }

    #[test]
    fn loads_prompt_file_and_derives_version_from_filename() {
        let path = temp_prompt_path("prompt-file-load");
        fs::write(&path, "# Prompt\n\nUse only today's entries.").unwrap();

        let prompt = load("daily review", &path).unwrap();

        assert_eq!(prompt.version, path.file_stem().unwrap().to_string_lossy());
        assert_eq!(prompt.text, "# Prompt\n\nUse only today's entries.");

        fs::remove_file(path).unwrap();
    }

    #[test]
    fn missing_prompt_file_returns_clear_error() {
        let path = temp_prompt_path("prompt-file-missing");

        let error = load("daily review", &path).unwrap_err();

        assert!(matches!(error, PromptFileError::ReadFailed { .. }));
        assert!(error.to_string().contains(path.to_str().unwrap()));
        assert!(error.to_string().contains("daily review"));
    }

    #[test]
    fn empty_prompt_file_returns_clear_error() {
        let path = temp_prompt_path("prompt-file-empty");
        fs::write(&path, "  \n").unwrap();

        let error = load("signal extraction", &path).unwrap_err();

        assert_eq!(
            error,
            PromptFileError::Empty {
                kind: "signal extraction",
                path: path.clone(),
            }
        );

        fs::remove_file(path).unwrap();
    }
}
