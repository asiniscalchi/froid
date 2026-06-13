use chrono::NaiveDate;

use super::review::DailyReview;
use super::week_review::WeeklyReview;

pub(super) fn message_saved_response() -> String {
    "Message saved.".to_string()
}

pub(super) fn start_response() -> String {
    "Froid is your private journal. Send me any text message and I will store it for you.\n\nI use AI to help you find meaning in your entries and provide daily and weekly reviews of your thoughts.\n\nUse /help to see all available commands.".to_string()
}

pub(super) fn search_usage_response() -> String {
    "Usage: /search <query>\n\nExamples:\n/search anxiety before meetings".to_string()
}

pub(super) fn no_entry_to_delete_response() -> String {
    "No journal entry to delete.".to_string()
}

pub(super) fn deleted_last_entry_response() -> String {
    "Deleted last entry.".to_string()
}

pub(super) fn daily_review_unavailable_response() -> String {
    "Daily review generation is not configured yet.".to_string()
}

pub(crate) fn daily_review_not_available_for_date_response(date: NaiveDate) -> String {
    format!(
        "No daily review available for {} yet.",
        date.format("%Y-%m-%d")
    )
}

pub(crate) fn format_daily_review_for_date(review: &DailyReview, date: NaiveDate) -> String {
    format!(
        "Daily review for {}\n\n{}",
        date.format("%Y-%m-%d"),
        review.review_text.as_deref().unwrap_or_default()
    )
}

pub(crate) fn format_weekly_review_for_week(
    review: &WeeklyReview,
    week_start: NaiveDate,
) -> String {
    format!(
        "Weekly review for week of {}\n\n{}",
        week_start.format("%Y-%m-%d"),
        review.review_text.as_deref().unwrap_or_default()
    )
}

pub(super) fn weekly_review_unavailable_response() -> String {
    "Weekly review generation is not configured yet.".to_string()
}

pub(super) fn weekly_review_not_available_response(week_start: NaiveDate) -> String {
    format!(
        "No weekly review available for the week of {} yet.",
        week_start.format("%Y-%m-%d")
    )
}
