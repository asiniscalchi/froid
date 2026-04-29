use super::entry::{JournalEntry, JournalStats};
use super::review::DailyReview;

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
    "Commands:\n/recent [number] - show recent entries\n/today - show today's entries\n/review today - generate today's review\n/stats - show journal stats\n/search <query> - search entries by meaning\n/help - show commands".to_string()
}

pub(super) fn recent_usage_response() -> String {
    "Usage: /recent [number]\n\nExamples:\n/recent\n/recent 5".to_string()
}

pub(super) fn no_entries_response() -> String {
    "No journal entries found.".to_string()
}

pub(super) fn no_entries_today_response() -> String {
    "No journal entries found for today.".to_string()
}

pub(super) fn daily_review_usage_response() -> String {
    "Usage: /review today".to_string()
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

pub(super) fn format_entries(entries: &[JournalEntry]) -> String {
    entries
        .iter()
        .map(|e| format!("{} - {}", e.received_at.format("%Y-%m-%d %H:%M"), e.text))
        .collect::<Vec<_>>()
        .join("\n")
}
