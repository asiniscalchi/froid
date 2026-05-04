use chrono::{Duration, NaiveDate};
use tracing::{error, warn};

use crate::{
    journal::{
        command::{JournalCommand, JournalCommandRequest, MAX_RECENT_LIMIT},
        responses::{
            daily_review_not_available_for_date_response, daily_review_unavailable_response,
            deleted_last_entry_response, format_daily_review_for_date, format_entries,
            format_last_entry, format_weekly_review_for_week, help_response, no_entries_response,
            no_entries_today_response, no_entry_to_delete_response, no_last_entry_response,
            recent_usage_response, search_usage_response, start_response, stats_response,
            status_response, unknown_command_response, weekly_review_not_available_response,
            weekly_review_unavailable_response,
        },
        review::DailyReview,
        search::{
            format_search_results, search_empty_response, search_error_response,
            search_unavailable_response,
        },
        status::{
            DailyReviewDeliveryStatus, DailyReviewGenerationStatus, DailyReviewStatus,
            EmbeddingStatus, SemanticSearchStatus, StatusReport,
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
            JournalCommand::Help => Ok(OutgoingMessage {
                text: help_response(),
            }),
            JournalCommand::Last => self.last(request).await,
            JournalCommand::Undo => self.undo(request).await,
            JournalCommand::Recent { requested_limit } => {
                self.recent(&request.user_id, *requested_limit).await
            }
            JournalCommand::RecentUsage => Ok(OutgoingMessage {
                text: recent_usage_response(),
            }),
            JournalCommand::Today => {
                self.today(&request.user_id, request.received_at.date_naive())
                    .await
            }
            JournalCommand::Stats => {
                self.stats(&request.user_id, request.received_at.date_naive())
                    .await
            }
            JournalCommand::Status => {
                self.status(&request.user_id, request.received_at.date_naive())
                    .await
            }
            JournalCommand::DayReviewLast => Ok(self
                .day_review_last(&request.user_id, request.received_at.date_naive())
                .await),
            JournalCommand::WeekReviewLast => Ok(self
                .week_review_last(&request.user_id, request.received_at.date_naive())
                .await),
            JournalCommand::Search { query } => {
                Ok(self.search_command(&request.user_id, query).await)
            }
            JournalCommand::SearchUsage => Ok(OutgoingMessage {
                text: search_usage_response(),
            }),
            JournalCommand::Unknown { command } => Ok(OutgoingMessage {
                text: unknown_command_response(command),
            }),
        }
    }

    async fn day_review_last(&self, user_id: &str, today: chrono::NaiveDate) -> OutgoingMessage {
        let yesterday = today - Duration::days(1);
        self.run_review(
            user_id,
            yesterday,
            |r| format_daily_review_for_date(r, yesterday),
            daily_review_not_available_for_date_response(yesterday),
        )
        .await
    }

    async fn run_review(
        &self,
        user_id: &str,
        date: chrono::NaiveDate,
        format_review: impl Fn(&DailyReview) -> String,
        not_found_text: String,
    ) -> OutgoingMessage {
        let Some(daily_review) = &self.daily_review else {
            return OutgoingMessage {
                text: daily_review_unavailable_response(),
            };
        };

        match daily_review.fetch_review(user_id, date).await {
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

    async fn week_review_last(&self, user_id: &str, today: NaiveDate) -> OutgoingMessage {
        let Some(weekly_review) = &self.weekly_review else {
            return OutgoingMessage {
                text: weekly_review_unavailable_response(),
            };
        };

        let week_start = previous_iso_week_monday(today);

        match weekly_review.fetch_review(user_id, week_start).await {
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

    async fn search_command(&self, user_id: &str, query: &str) -> OutgoingMessage {
        let Some(search) = &self.search else {
            return OutgoingMessage {
                text: search_unavailable_response(),
            };
        };

        match search.search(user_id, query).await {
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

    async fn last(&self, request: &JournalCommandRequest) -> Result<OutgoingMessage, sqlx::Error> {
        let Some(entry) = self
            .repository
            .fetch_last_for_conversation(
                &request.user_id,
                &request.source,
                &request.source_conversation_id,
            )
            .await?
        else {
            return Ok(OutgoingMessage {
                text: no_last_entry_response(),
            });
        };

        Ok(OutgoingMessage {
            text: format_last_entry(&entry.entry),
        })
    }

    async fn undo(&self, request: &JournalCommandRequest) -> Result<OutgoingMessage, sqlx::Error> {
        let Some(_) = self
            .store
            .delete_last_for_conversation(
                &request.user_id,
                &request.source,
                &request.source_conversation_id,
            )
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

    async fn recent(&self, user_id: &str, limit: u32) -> Result<OutgoingMessage, sqlx::Error> {
        let limit = limit.min(MAX_RECENT_LIMIT);
        let entries = self.repository.fetch_recent(user_id, limit).await?;

        if entries.is_empty() {
            return Ok(OutgoingMessage {
                text: no_entries_response(),
            });
        }

        Ok(OutgoingMessage {
            text: format_entries(&entries),
        })
    }

    async fn today(
        &self,
        user_id: &str,
        date: chrono::NaiveDate,
    ) -> Result<OutgoingMessage, sqlx::Error> {
        let entries = self.repository.fetch_today(user_id, date).await?;

        if entries.is_empty() {
            return Ok(OutgoingMessage {
                text: no_entries_today_response(),
            });
        }

        Ok(OutgoingMessage {
            text: format_entries(&entries),
        })
    }

    async fn stats(
        &self,
        user_id: &str,
        today: chrono::NaiveDate,
    ) -> Result<OutgoingMessage, sqlx::Error> {
        let stats = self.repository.stats(user_id, today).await?;

        Ok(OutgoingMessage {
            text: stats_response(&stats),
        })
    }

    async fn status(
        &self,
        user_id: &str,
        today: chrono::NaiveDate,
    ) -> Result<OutgoingMessage, sqlx::Error> {
        let journal = self.repository.stats(user_id, today).await?;
        let embeddings = self.embedding_status(user_id).await;
        let daily_review = self.daily_review_status();

        Ok(OutgoingMessage {
            text: status_response(&StatusReport {
                journal,
                embeddings,
                daily_review,
            }),
        })
    }

    async fn embedding_status(&self, user_id: &str) -> EmbeddingStatus {
        let semantic_search = if self.search.is_some() && self.embedding_status_config.is_some() {
            SemanticSearchStatus::Enabled
        } else {
            SemanticSearchStatus::Unavailable
        };

        let pending_embeddings = match (
            self.embedding_status_config.as_ref(),
            self.pending_embedding_counter.as_ref(),
        ) {
            (Some(config), Some(counter)) => {
                match counter
                    .count_entries_missing_embedding_for_user(user_id, &config.model)
                    .await
                {
                    Ok(count) => Some(count),
                    Err(error) => {
                        warn!(%error, "failed to count pending embeddings for status");
                        None
                    }
                }
            }
            _ => None,
        };

        EmbeddingStatus {
            semantic_search,
            config: self.embedding_status_config.clone(),
            pending_embeddings,
        }
    }

    fn daily_review_status(&self) -> DailyReviewStatus {
        let generation = if self.daily_review.is_some() {
            DailyReviewGenerationStatus::Configured
        } else {
            DailyReviewGenerationStatus::NotConfigured
        };

        let delivery = if self.daily_review_delivery_configured {
            DailyReviewDeliveryStatus::Configured
        } else {
            DailyReviewDeliveryStatus::NotConfigured
        };

        DailyReviewStatus {
            generation,
            prompt_version: self.daily_review_prompt_version.clone(),
            delivery,
        }
    }
}
