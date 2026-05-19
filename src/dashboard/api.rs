use axum::{
    Router,
    extract::State,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use chrono::{DateTime, Utc};
use serde::Serialize;
use tracing::error;

use crate::journal::repository::JournalRepository;

pub fn router(repo: JournalRepository) -> Router {
    Router::new()
        .route("/messages/export", get(export_messages))
        .with_state(repo)
}

#[derive(Serialize)]
struct ExportedMessage {
    id: i64,
    text: String,
    received_at: DateTime<Utc>,
}

async fn export_messages(State(repo): State<JournalRepository>) -> Response {
    let entries = match repo.fetch_all().await {
        Ok(entries) => entries,
        Err(err) => {
            error!(error = %err, "failed to fetch journal entries for export");
            return (StatusCode::INTERNAL_SERVER_ERROR, "failed to load messages").into_response();
        }
    };

    let exported: Vec<ExportedMessage> = entries
        .into_iter()
        .map(|stored| ExportedMessage {
            id: stored.id,
            text: stored.entry.text,
            received_at: stored.entry.received_at,
        })
        .collect();

    let body = match serde_json::to_vec(&exported) {
        Ok(body) => body,
        Err(err) => {
            error!(error = %err, "failed to serialize journal entries for export");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to serialize messages",
            )
                .into_response();
        }
    };

    let filename = format!("froid-messages-{}.json", Utc::now().format("%Y-%m-%d"));

    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/json".to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{filename}\""),
            ),
        ],
        body,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database;
    use crate::messages::{IncomingMessage, MessageSource};
    use axum::body::{Body, to_bytes};
    use axum::http::Request;
    use chrono::TimeZone;
    use serde_json::Value;
    use sqlx::SqlitePool;
    use tower::ServiceExt;

    async fn setup() -> (Router, JournalRepository) {
        database::register_sqlite_vec_extension();
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        let repo = JournalRepository::new(pool);
        (router(repo.clone()), repo)
    }

    fn incoming(message_id: &str, text: &str, received_at: DateTime<Utc>) -> IncomingMessage {
        IncomingMessage {
            source: MessageSource::Telegram,
            source_conversation_id: "42".to_string(),
            source_message_id: message_id.to_string(),
            user_id: "7".to_string(),
            text: text.to_string(),
            received_at,
        }
    }

    #[tokio::test]
    async fn export_returns_empty_array_when_no_entries() {
        let (router, _) = setup().await;

        let response = router
            .oneshot(
                Request::builder()
                    .uri("/messages/export")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .unwrap()
                .to_str()
                .unwrap(),
            "application/json"
        );
        let disposition = response
            .headers()
            .get(header::CONTENT_DISPOSITION)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert!(disposition.starts_with("attachment; filename=\"froid-messages-"));
        assert!(disposition.ends_with(".json\""));

        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let parsed: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed, serde_json::json!([]));
    }

    #[tokio::test]
    async fn export_returns_all_entries_newest_first() {
        let (router, repo) = setup().await;

        let earlier = Utc.with_ymd_and_hms(2026, 4, 28, 10, 0, 0).unwrap();
        let later = Utc.with_ymd_and_hms(2026, 4, 28, 12, 0, 0).unwrap();
        repo.store(&incoming("1", "older", earlier)).await.unwrap();
        repo.store(&incoming("2", "newer", later)).await.unwrap();

        let response = router
            .oneshot(
                Request::builder()
                    .uri("/messages/export")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let parsed: Vec<Value> = serde_json::from_slice(&body).unwrap();

        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0]["text"], "newer");
        assert_eq!(parsed[1]["text"], "older");
        assert!(parsed[0]["id"].is_i64());
        assert_eq!(parsed[0]["received_at"], "2026-04-28T12:00:00Z");
    }
}
