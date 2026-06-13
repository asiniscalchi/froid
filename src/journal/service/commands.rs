use chrono::{Duration, NaiveDate};
use tracing::error;

use crate::{
    journal::{
        command::{JournalCommand, JournalCommandRequest},
        responses::{
            daily_review_not_available_for_date_response, daily_review_unavailable_response,
            deleted_last_entry_response, format_daily_review_for_date,
            format_weekly_review_for_week, no_entry_to_delete_response, search_usage_response,
            start_response, weekly_review_not_available_response,
            weekly_review_unavailable_response,
        },
        review::DailyReview,
        search::{
            format_search_results, search_empty_response, search_error_response,
            search_unavailable_response,
        },
    },
    messages::OutgoingMessage,
};

fn previous_iso_week_monday(today: NaiveDate) -> NaiveDate {
    let days_since_monday = today.weekday().num_days_from_monday() as i64;
    let this_monday = today - Duration::days(days_since_monday);
    this_monday - Duration::days(7)
}

use chrono::Datelike;

use super::JournalService;

impl JournalService {
    pub async fn command(
        &self,
        request: &JournalCommandRequest,
    ) -> Result<OutgoingMessage, sqlx::Error> {
        match &request.command {
            JournalCommand::Start => Ok(OutgoingMessage {
                text: start_response(),
            }),
            JournalCommand::Undo => self.undo(request).await,
            JournalCommand::DayReviewLast => {
                Ok(self.day_review_last(request.received_at.date_naive()).await)
            }
            JournalCommand::WeekReviewLast => Ok(self
                .week_review_last(request.received_at.date_naive())
                .await),
            JournalCommand::Search { query } => Ok(self.search_command(query).await),
            JournalCommand::SearchUsage => Ok(OutgoingMessage {
                text: search_usage_response(),
            }),
        }
    }

    async fn day_review_last(&self, today: chrono::NaiveDate) -> OutgoingMessage {
        let yesterday = today - Duration::days(1);
        self.run_review(
            yesterday,
            |r| format_daily_review_for_date(r, yesterday),
            daily_review_not_available_for_date_response(yesterday),
        )
        .await
    }

    async fn run_review(
        &self,
        date: chrono::NaiveDate,
        format_review: impl Fn(&DailyReview) -> String,
        not_found_text: String,
    ) -> OutgoingMessage {
        let Some(daily_review) = &self.daily_review else {
            return OutgoingMessage {
                text: daily_review_unavailable_response(),
            };
        };

        match daily_review.fetch_review(date).await {
            Ok(Some(review)) => OutgoingMessage {
                text: format_review(&review),
            },
            Ok(None) => OutgoingMessage {
                text: not_found_text,
            },
            Err(error) => {
                error!(%error, "failed to fetch daily review");
                OutgoingMessage {
                    text: not_found_text,
                }
            }
        }
    }

    async fn week_review_last(&self, today: NaiveDate) -> OutgoingMessage {
        let Some(weekly_review) = &self.weekly_review else {
            return OutgoingMessage {
                text: weekly_review_unavailable_response(),
            };
        };

        let week_start = previous_iso_week_monday(today);

        match weekly_review.fetch_review(week_start).await {
            Ok(Some(review)) => OutgoingMessage {
                text: format_weekly_review_for_week(&review, week_start),
            },
            Ok(None) => OutgoingMessage {
                text: weekly_review_not_available_response(week_start),
            },
            Err(error) => {
                error!(%error, "failed to fetch weekly review");
                OutgoingMessage {
                    text: weekly_review_not_available_response(week_start),
                }
            }
        }
    }

    async fn search_command(&self, query: &str) -> OutgoingMessage {
        let Some(search) = &self.search else {
            return OutgoingMessage {
                text: search_unavailable_response(),
            };
        };

        match search.search(query).await {
            Ok(results) if results.is_empty() => OutgoingMessage {
                text: search_empty_response(),
            },
            Ok(results) => OutgoingMessage {
                text: format_search_results(query, &results),
            },
            Err(e) => OutgoingMessage {
                text: search_error_response(&e),
            },
        }
    }

    async fn undo(&self, request: &JournalCommandRequest) -> Result<OutgoingMessage, sqlx::Error> {
        let Some(_) = self
            .store
            .delete_last_for_conversation(&request.source, &request.source_conversation_id)
            .await?
        else {
            return Ok(OutgoingMessage {
                text: no_entry_to_delete_response(),
            });
        };

        Ok(OutgoingMessage {
            text: deleted_last_entry_response(),
        })
    }
}
