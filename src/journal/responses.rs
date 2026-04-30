use chrono::NaiveDate;

use super::entry::{JournalEntry, JournalStats};
use super::review::DailyReview;
use super::status::{
    DailyReviewDeliveryStatus, DailyReviewGenerationStatus, DailyReviewStatus, EmbeddingStatus,
    SemanticSearchStatus, StatusReport,
};

pub(super) fn message_saved_response() -> String {
    "Message saved.".to_string()
}

pub(super) fn start_response() -> String {
    format!(
        "Froid is running.\n\nSend me any text message and I will store it as a journal entry.\n\n{}",
        help_response()
    )
}

pub(super) fn help_response() -> String {
    "Commands:\n/last - show latest entry\n/undo - delete latest entry\n/recent [number] - show recent entries\n/today - show today's entries\n/review [today|YYYY-MM-DD|-N] - generate daily review\n/stats - show journal stats\n/status - show bot status\n/search <query> - search entries by meaning\n/help - show commands".to_string()
}

pub(super) fn unknown_command_response(command: &str) -> String {
    format!("Unknown command: {command}\n\n{}", help_response())
}

pub(super) fn recent_usage_response() -> String {
    "Usage: /recent [number]\n\nExamples:\n/recent\n/recent 5".to_string()
}

pub(super) fn no_entries_response() -> String {
    "No journal entries found.".to_string()
}

pub(super) fn no_last_entry_response() -> String {
    "No journal entry found.".to_string()
}

pub(super) fn no_entry_to_delete_response() -> String {
    "No journal entry to delete.".to_string()
}

pub(super) fn deleted_last_entry_response() -> String {
    "Deleted last entry.".to_string()
}

pub(super) fn no_entries_today_response() -> String {
    "No journal entries found for today.".to_string()
}

pub(super) fn daily_review_usage_response() -> String {
    "Usage: /review [today|YYYY-MM-DD|-N]\n\nExamples:\n/review\n/review today\n/review 2026-04-29\n/review -1\n/review -7".to_string()
}

pub(super) fn daily_review_unavailable_response() -> String {
    "Daily review generation is not configured yet.".to_string()
}

pub(super) fn daily_review_failure_response() -> String {
    "I could not generate today's review right now. Please try again later.".to_string()
}

pub(super) fn format_daily_review(review: &DailyReview) -> String {
    format!(
        "Today's review\n\n{}",
        review.review_text.as_deref().unwrap_or_default()
    )
}

pub(super) fn format_daily_review_for_date(review: &DailyReview, date: NaiveDate) -> String {
    format!(
        "Daily review for {}\n\n{}",
        date.format("%Y-%m-%d"),
        review.review_text.as_deref().unwrap_or_default()
    )
}

pub(super) fn no_entries_for_date_response(date: NaiveDate) -> String {
    format!("No journal entries found for {}.", date.format("%Y-%m-%d"))
}

pub(super) fn stats_response(stats: &JournalStats) -> String {
    let latest = stats
        .latest_received_at
        .map(|timestamp| timestamp.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| "none".to_string());

    format!(
        "Journal stats:\nTotal entries: {}\nEntries today: {}\nLatest entry: {}",
        stats.total_entries, stats.entries_today, latest
    )
}

pub(super) fn status_response(report: &StatusReport) -> String {
    format!(
        "Froid status\n\nJournal:\n- Total entries: {}\n- Entries today UTC: {}\n\nEmbeddings:\n{}\n\nDaily review:\n{}",
        report.journal.total_entries,
        report.journal.entries_today,
        format_embedding_status(&report.embeddings),
        format_daily_review_status(&report.daily_review)
    )
}

fn format_embedding_status(status: &EmbeddingStatus) -> String {
    let semantic_search = match status.semantic_search {
        SemanticSearchStatus::Enabled => "enabled",
        SemanticSearchStatus::Unavailable => "unavailable",
    };
    let model = status
        .config
        .as_ref()
        .map(|config| config.model.as_str())
        .unwrap_or("unavailable");
    let dimensions = status
        .config
        .as_ref()
        .map(|config| config.dimensions.to_string())
        .unwrap_or_else(|| "unavailable".to_string());
    let pending_embeddings = status
        .pending_embeddings
        .map(|count| count.to_string())
        .unwrap_or_else(|| "unavailable".to_string());

    format!(
        "- Semantic search: {semantic_search}\n- Model: {model}\n- Dimensions: {dimensions}\n- Pending embeddings: {pending_embeddings}"
    )
}

fn format_daily_review_status(status: &DailyReviewStatus) -> String {
    let generation = match status.generation {
        DailyReviewGenerationStatus::Configured => "configured",
        DailyReviewGenerationStatus::NotConfigured => "not configured",
    };
    let delivery = match status.delivery {
        DailyReviewDeliveryStatus::NotImplemented => "not implemented",
    };

    let mut lines = vec![format!("- Generation: {generation}")];
    if let Some(prompt_version) = &status.prompt_version {
        lines.push(format!("- Prompt: {prompt_version}"));
    }
    lines.push(format!("- Delivery: {delivery}"));
    lines.push("- Date mode: UTC".to_string());

    lines.join("\n")
}

pub(super) fn format_entries(entries: &[JournalEntry]) -> String {
    entries
        .iter()
        .map(|e| format!("{} - {}", e.received_at.format("%Y-%m-%d %H:%M"), e.text))
        .collect::<Vec<_>>()
        .join("\n")
}

pub(super) fn format_last_entry(entry: &JournalEntry) -> String {
    format!(
        "Last entry:\n\n\"{}\"\n\nReceived at: {}\n\nUse /undo to delete it.",
        entry.text,
        entry.received_at.format("%Y-%m-%d %H:%M")
    )
}
