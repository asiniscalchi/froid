use axum::{
    Json, Router,
    extract::State,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::error;

use crate::journal::repository::{JournalEntryRecord, JournalRepository};

pub fn router(repo: JournalRepository) -> Router {
    Router::new()
        .route("/messages/export", get(export_messages))
        .route("/messages/import", post(import_messages))
        .with_state(repo)
}

#[derive(Serialize)]
struct ExportedMessage {
    id: i64,
    source: String,
    source_conversation_id: String,
    source_message_id: String,
    text: String,
    received_at: DateTime<Utc>,
}

async fn export_messages(State(repo): State<JournalRepository>) -> Response {
    let records = match repo.fetch_all_for_export().await {
        Ok(records) => records,
        Err(err) => {
            error!(error = %err, "failed to fetch journal entries for export");
            return (StatusCode::INTERNAL_SERVER_ERROR, "failed to load messages").into_response();
        }
    };

    let exported: Vec<ExportedMessage> = records
        .into_iter()
        .filter_map(|r| {
            r.id.map(|id| ExportedMessage {
                id,
                source: r.source,
                source_conversation_id: r.source_conversation_id,
                source_message_id: r.source_message_id,
                text: r.text,
                received_at: r.received_at,
            })
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

#[derive(Deserialize)]
struct ImportedMessage {
    source: String,
    source_conversation_id: String,
    source_message_id: String,
    text: String,
    received_at: DateTime<Utc>,
}

#[derive(Serialize)]
struct ImportResult {
    imported: usize,
}

#[derive(Serialize)]
struct ImportError {
    error: String,
}

async fn import_messages(
    State(repo): State<JournalRepository>,
    Json(payload): Json<Vec<ImportedMessage>>,
) -> Response {
    if payload.is_empty() {
        return (StatusCode::OK, Json(ImportResult { imported: 0 })).into_response();
    }

    let records: Vec<JournalEntryRecord> = payload
        .into_iter()
        .map(|m| JournalEntryRecord {
            id: None,
            source: m.source,
            source_conversation_id: m.source_conversation_id,
            source_message_id: m.source_message_id,
            text: m.text,
            received_at: m.received_at,
        })
        .collect();

    match repo.bulk_import(&records).await {
        Ok(imported) => (StatusCode::OK, Json(ImportResult { imported })).into_response(),
        Err(sqlx::Error::Database(db_err)) if is_unique_violation(db_err.as_ref()) => {
            error!(error = %db_err, "import aborted by unique constraint");
            (
                StatusCode::CONFLICT,
                Json(ImportError {
                    error: "import aborted: one or more entries collide with existing messages"
                        .to_string(),
                }),
            )
                .into_response()
        }
        Err(err) => {
            error!(error = %err, "failed to import journal entries");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ImportError {
                    error: "failed to import messages".to_string(),
                }),
            )
                .into_response()
        }
    }
}

fn is_unique_violation(err: &dyn sqlx::error::DatabaseError) -> bool {
    err.code().as_deref() == Some("2067") // SQLITE_CONSTRAINT_UNIQUE
        || err.message().to_lowercase().contains("unique constraint")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database;
    use crate::messages::{IncomingMessage, MessageSource};
    use axum::body::{Body, to_bytes};
    use axum::http::Request;
    use chrono::TimeZone;
    use serde_json::{Value, json};
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
    async fn export_returns_all_fields_newest_first() {
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
        assert_eq!(parsed[0]["source"], "telegram");
        assert_eq!(parsed[0]["source_conversation_id"], "42");
        assert_eq!(parsed[0]["source_message_id"], "2");
        assert!(parsed[0]["id"].is_i64());
        assert_eq!(parsed[0]["received_at"], "2026-04-28T12:00:00Z");
        assert_eq!(parsed[1]["text"], "older");
    }

    #[tokio::test]
    async fn import_inserts_messages_and_returns_count() {
        let (router, repo) = setup().await;

        let payload = json!([
            {
                "source": "telegram",
                "source_conversation_id": "42",
                "source_message_id": "i-1",
                "text": "hello",
                "received_at": "2026-04-28T10:00:00Z"
            },
            {
                "source": "telegram",
                "source_conversation_id": "42",
                "source_message_id": "i-2",
                "text": "world",
                "received_at": "2026-04-28T11:00:00Z"
            }
        ]);

        let response = router
            .oneshot(
                Request::builder()
                    .uri("/messages/import")
                    .method("POST")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_vec(&payload).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let parsed: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed["imported"], 2);

        let entries = repo.fetch_recent("7", 10).await.unwrap();
        assert_eq!(entries.len(), 2);
    }

    #[tokio::test]
    async fn import_fails_atomically_on_collision() {
        let (router, repo) = setup().await;
        repo.store(&incoming(
            "dup",
            "existing",
            Utc.with_ymd_and_hms(2026, 4, 28, 9, 0, 0).unwrap(),
        ))
        .await
        .unwrap();

        let payload = json!([
            {
                "source": "telegram",
                "source_conversation_id": "42",
                "source_message_id": "fresh",
                "text": "fresh",
                "received_at": "2026-04-28T10:00:00Z"
            },
            {
                "source": "telegram",
                "source_conversation_id": "42",
                "source_message_id": "dup",
                "text": "collides",
                "received_at": "2026-04-28T11:00:00Z"
            }
        ]);

        let response = router
            .oneshot(
                Request::builder()
                    .uri("/messages/import")
                    .method("POST")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_vec(&payload).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CONFLICT);
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let parsed: Value = serde_json::from_slice(&body).unwrap();
        assert!(parsed["error"].as_str().unwrap().contains("collide"));

        let entries = repo.fetch_recent("7", 10).await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].entry.text, "existing");
    }

    #[tokio::test]
    async fn import_rejects_malformed_json() {
        let (router, _) = setup().await;

        let response = router
            .oneshot(
                Request::builder()
                    .uri("/messages/import")
                    .method("POST")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from("not json"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert!(response.status().is_client_error());
    }
}
