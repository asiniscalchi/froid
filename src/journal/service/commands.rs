use tracing::{error, warn};

use crate::{
    journal::{
        command::{JournalCommand, JournalCommandRequest, MAX_RECENT_LIMIT},
        responses::{
            daily_review_failure_response, daily_review_unavailable_response,
            daily_review_usage_response, deleted_last_entry_response, format_daily_review,
            format_daily_review_for_date, format_entries, format_last_entry, help_response,
            no_entries_for_date_response, no_entries_response, no_entries_today_response,
            no_entry_to_delete_response, no_last_entry_response, recent_usage_response,
            start_response, stats_response, status_response, unknown_command_response,
        },
        review::{DailyReview, DailyReviewResult},
        search::{
            format_search_results, search_empty_response, search_error_response,
            search_unavailable_response, search_usage_response,
        },
        status::{
            DailyReviewDeliveryStatus, DailyReviewGenerationStatus, DailyReviewStatus,
            EmbeddingStatus, SemanticSearchStatus, StatusReport,
        },
    },
    messages::OutgoingMessage,
};

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
            JournalCommand::ReviewToday => Ok(self
                .review_today(&request.user_id, request.received_at.date_naive())
                .await),
            JournalCommand::ReviewDate { date } => {
                Ok(self.review_date(&request.user_id, *date).await)
            }
            JournalCommand::ReviewUsage => Ok(OutgoingMessage {
                text: daily_review_usage_response(),
            }),
            JournalCommand::ReviewError { message } => Ok(OutgoingMessage {
                text: message.clone(),
            }),
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

    async fn review_today(&self, user_id: &str, date: chrono::NaiveDate) -> OutgoingMessage {
        self.run_review(
            user_id,
            date,
            format_daily_review,
            no_entries_today_response(),
        )
        .await
    }

    async fn review_date(&self, user_id: &str, date: chrono::NaiveDate) -> OutgoingMessage {
        self.run_review(
            user_id,
            date,
            |review| format_daily_review_for_date(review, date),
            no_entries_for_date_response(date),
        )
        .await
    }

    async fn run_review(
        &self,
        user_id: &str,
        date: chrono::NaiveDate,
        format_review: impl Fn(&DailyReview) -> String,
        empty_text: String,
    ) -> OutgoingMessage {
        let Some(daily_review) = &self.daily_review else {
            return OutgoingMessage {
                text: daily_review_unavailable_response(),
            };
        };

        match daily_review.review_day(user_id, date).await {
            Ok(DailyReviewResult::Existing(review) | DailyReviewResult::Generated(review)) => {
                OutgoingMessage {
                    text: format_review(&review),
                }
            }
            Ok(DailyReviewResult::EmptyDay) => OutgoingMessage { text: empty_text },
            Ok(DailyReviewResult::GenerationFailed(failure)) => {
                warn!(
                    user_id = %failure.user_id,
                    review_date = %failure.review_date,
                    model = %failure.model,
                    prompt_version = %failure.prompt_version,
                    error = %failure.error_message,
                    "failed to generate daily review"
                );
                OutgoingMessage {
                    text: daily_review_failure_response(),
                }
            }
            Err(error) => {
                error!(%error, "failed to process daily review command");
                OutgoingMessage {
                    text: daily_review_failure_response(),
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
