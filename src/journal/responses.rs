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

pub(super) fn reviews_status_response(daily_enabled: bool, weekly_enabled: bool) -> String {
    format!(
        "Daily review delivery is {}.\nWeekly review delivery is {}.\n\n\
         Change them individually with /reviews daily on|off and /reviews weekly on|off, \
         or both at once with /reviews on|off.",
        if daily_enabled { "ON" } else { "OFF" },
        if weekly_enabled { "ON" } else { "OFF" }
    )
}

pub(super) fn reviews_enabled_response() -> String {
    "Daily and weekly review delivery is now ON.".to_string()
}

pub(super) fn reviews_disabled_response() -> String {
    "Daily and weekly review delivery is now OFF. You can still request them on demand \
     with /day_review and /week_review."
        .to_string()
}

pub(super) fn reviews_daily_enabled_response() -> String {
    "Daily review delivery is now ON.".to_string()
}

pub(super) fn reviews_daily_disabled_response() -> String {
    "Daily review delivery is now OFF. You can still request it on demand with /day_review."
        .to_string()
}

pub(super) fn reviews_weekly_enabled_response() -> String {
    "Weekly review delivery is now ON.".to_string()
}

pub(super) fn reviews_weekly_disabled_response() -> String {
    "Weekly review delivery is now OFF. You can still request it on demand with /week_review."
        .to_string()
}

pub(super) fn reviews_usage_response() -> String {
    "Usage: /reviews to show your current settings, /reviews on or /reviews off to change \
     both at once, or /reviews daily on|off and /reviews weekly on|off to change them \
     individually."
        .to_string()
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::*;
    use crate::journal::review::DailyReviewStatus;

    fn daily_review(review_text: &str) -> DailyReview {
        DailyReview {
            id: 1,
            review_date: NaiveDate::from_ymd_opt(2026, 6, 15).unwrap(),
            review_text: Some(review_text.to_string()),
            model: "test-model".to_string(),
            prompt_version: "daily_review_v3".to_string(),
            status: DailyReviewStatus::Completed,
            error_message: None,
            delivered_at: None,
            delivery_error: None,
            signals_status: None,
            signals_error: None,
            signals_model: None,
            signals_prompt_version: None,
            signals_updated_at: None,
            created_at: Utc.with_ymd_and_hms(2026, 6, 15, 22, 0, 0).unwrap(),
            updated_at: Utc.with_ymd_and_hms(2026, 6, 15, 22, 0, 0).unwrap(),
        }
    }

    #[test]
    fn renders_application_owned_date_header_above_model_body() {
        let date = NaiveDate::from_ymd_opt(2026, 6, 15).unwrap();
        let body = "Summary:\nGiornata stanca.\n\nMain signals:\n- Energia bassa.";

        let rendered = format_daily_review_for_date(&daily_review(body), date);

        assert!(rendered.starts_with("Daily review for 2026-06-15\n\n"));
        assert!(rendered.ends_with(body));
    }

    #[test]
    fn rendered_review_contains_exactly_one_date_header() {
        let date = NaiveDate::from_ymd_opt(2026, 6, 15).unwrap();
        // Model body starts at Summary and must not carry its own date header.
        let body = "Summary:\nGiornata stanca.";

        let rendered = format_daily_review_for_date(&daily_review(body), date);

        assert_eq!(rendered.matches("Daily review for").count(), 1);
    }
}
