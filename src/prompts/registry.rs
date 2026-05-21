use std::path::{Path, PathBuf};

/// A stable identifier for each prompt that can be customized by the user.
///
/// The string returned by [`PromptKey::as_str`] is the lookup key used in the
/// `customized_prompts` table and in dashboard API URLs. It must remain stable
/// across releases even when the bundled markdown filename changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PromptKey {
    DailyReview,
    SignalExtraction,
    WeeklyReview,
    EntryExtraction,
}

impl PromptKey {
    pub const ALL: [PromptKey; 4] = [
        PromptKey::DailyReview,
        PromptKey::SignalExtraction,
        PromptKey::WeeklyReview,
        PromptKey::EntryExtraction,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            PromptKey::DailyReview => "daily_review",
            PromptKey::SignalExtraction => "signal_extraction",
            PromptKey::WeeklyReview => "weekly_review",
            PromptKey::EntryExtraction => "entry_extraction",
        }
    }

    /// Human-readable label shown in the dashboard UI.
    pub fn label(self) -> &'static str {
        match self {
            PromptKey::DailyReview => "Daily review",
            PromptKey::SignalExtraction => "Daily review signal extraction",
            PromptKey::WeeklyReview => "Weekly review",
            PromptKey::EntryExtraction => "Journal entry extraction",
        }
    }

    /// Default on-disk path for the bundled prompt file.
    pub fn default_path(self) -> PathBuf {
        let relative = match self {
            PromptKey::DailyReview => crate::journal::review::prompt::DEFAULT_REVIEW_PROMPT_PATH,
            PromptKey::SignalExtraction => {
                crate::journal::review::signals::prompt::DEFAULT_SIGNAL_EXTRACTION_PROMPT_PATH
            }
            PromptKey::WeeklyReview => {
                crate::journal::week_review::prompt::DEFAULT_WEEK_REVIEW_PROMPT_PATH
            }
            PromptKey::EntryExtraction => {
                crate::journal::extraction::prompt::DEFAULT_JOURNAL_ENTRY_EXTRACTION_PROMPT_PATH
            }
        };
        PathBuf::from(relative)
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|key| key.as_str() == value)
    }
}

/// Computes the `version` string reported alongside resolved prompt text.
///
/// For the bundled default the version is the file stem of the markdown file
/// (e.g. `daily_review_with_entry_extractions_v1`). When a customization is in
/// effect we append `-custom` so downstream rows that record `prompt_version`
/// (signals, daily reviews, ...) clearly distinguish customized output.
pub fn version_for(path: &Path, customized: bool) -> String {
    let stem = path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    if customized {
        format!("{stem}-custom")
    } else {
        stem
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_round_trip_through_str() {
        for key in PromptKey::ALL {
            assert_eq!(PromptKey::parse(key.as_str()), Some(key));
        }
    }

    #[test]
    fn unknown_key_returns_none() {
        assert_eq!(PromptKey::parse("nope"), None);
    }

    #[test]
    fn default_paths_point_to_existing_files() {
        for key in PromptKey::ALL {
            let path = key.default_path();
            assert!(
                path.exists(),
                "default prompt {:?} missing on disk at {}",
                key,
                path.display()
            );
        }
    }

    #[test]
    fn version_appends_custom_suffix_when_customized() {
        let path = PathBuf::from("prompts/daily_review_v1.md");
        assert_eq!(version_for(&path, false), "daily_review_v1");
        assert_eq!(version_for(&path, true), "daily_review_v1-custom");
    }
}
