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

use crate::journal::repository::{BulkImportError, JournalEntryRecord, JournalRepository};

pub const EXPORT_FORMAT_VERSION: u32 = 1;

pub fn router(repo: JournalRepository) -> Router {
    Router::new()
        .route("/messages/export", get(export_messages))
        .route("/messages/import", post(import_messages))
        .with_state(repo)
}

#[derive(Serialize)]
struct ExportedMessage {
    source: String,
    source_conversation_id: String,
    source_message_id: String,
    text: String,
    received_at: DateTime<Utc>,
}

#[derive(Serialize)]
struct ExportEnvelope {
    version: u32,
    exported_at: DateTime<Utc>,
    messages: Vec<ExportedMessage>,
}

async fn export_messages(State(repo): State<JournalRepository>) -> Response {
    let records = match repo.fetch_all_for_export().await {
        Ok(records) => records,
        Err(err) => {
            error!(error = %err, "failed to fetch journal entries for export");
            return (StatusCode::INTERNAL_SERVER_ERROR, "failed to load messages").into_response();
        }
    };

    let envelope = ExportEnvelope {
        version: EXPORT_FORMAT_VERSION,
        exported_at: Utc::now(),
        messages: records
            .into_iter()
            .map(|r| ExportedMessage {
                source: r.source,
                source_conversation_id: r.source_conversation_id,
                source_message_id: r.source_message_id,
                text: r.text,
                received_at: r.received_at,
            })
            .collect(),
    };

    let body = match serde_json::to_vec(&envelope) {
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

    let filename = format!(
        "froid-messages-{}.json",
        envelope.exported_at.format("%Y-%m-%d")
    );

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

#[derive(Deserialize)]
struct ImportEnvelope {
    version: u32,
    #[serde(default)]
    #[allow(dead_code)]
    exported_at: Option<DateTime<Utc>>,
    messages: Vec<ImportedMessage>,
}

#[derive(Serialize)]
struct ImportResult {
    imported: usize,
}

#[derive(Serialize)]
struct ConflictDetails {
    source: String,
    source_conversation_id: String,
    source_message_id: String,
}

#[derive(Serialize)]
struct ImportError {
    error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    conflict: Option<ConflictDetails>,
}

async fn import_messages(
    State(repo): State<JournalRepository>,
    Json(envelope): Json<ImportEnvelope>,
) -> Response {
    if envelope.version != EXPORT_FORMAT_VERSION {
        return (
            StatusCode::BAD_REQUEST,
            Json(ImportError {
                error: format!(
                    "unsupported export version {} (expected {})",
                    envelope.version, EXPORT_FORMAT_VERSION
                ),
                conflict: None,
            }),
        )
            .into_response();
    }

    if envelope.messages.is_empty() {
        return (StatusCode::OK, Json(ImportResult { imported: 0 })).into_response();
    }

    let records: Vec<JournalEntryRecord> = envelope
        .messages
        .into_iter()
        .map(|m| JournalEntryRecord {
            source: m.source,
            source_conversation_id: m.source_conversation_id,
            source_message_id: m.source_message_id,
            text: m.text,
            received_at: m.received_at,
        })
        .collect();

    match repo.bulk_import(&records).await {
        Ok(imported) => (StatusCode::OK, Json(ImportResult { imported })).into_response(),
        Err(BulkImportError::Conflict {
            source,
            source_conversation_id,
            source_message_id,
        }) => (
            StatusCode::CONFLICT,
            Json(ImportError {
                error: format!(
                    "import aborted: entry ({source}, {source_conversation_id}, {source_message_id}) collides with an existing message"
                ),
                conflict: Some(ConflictDetails {
                    source,
                    source_conversation_id,
                    source_message_id,
                }),
            }),
        )
            .into_response(),
        Err(BulkImportError::Database(err)) => {
            error!(error = %err, "failed to import journal entries");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ImportError {
                    error: "failed to import messages".to_string(),
                    conflict: None,
                }),
            )
                .into_response()
        }
    }
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
    async fn export_returns_envelope_with_empty_messages() {
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
        assert_eq!(parsed["version"], EXPORT_FORMAT_VERSION);
        assert!(parsed["exported_at"].is_string());
        assert_eq!(parsed["messages"], serde_json::json!([]));
    }

    #[tokio::test]
    async fn export_envelope_contains_all_fields_newest_first() {
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
        let parsed: Value = serde_json::from_slice(&body).unwrap();

        let messages = parsed["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["text"], "newer");
        assert_eq!(messages[0]["source"], "telegram");
        assert_eq!(messages[0]["source_conversation_id"], "42");
        assert_eq!(messages[0]["source_message_id"], "2");
        assert!(messages[0].get("id").is_none(), "id must not be exported");
        assert_eq!(messages[0]["received_at"], "2026-04-28T12:00:00Z");
        assert_eq!(messages[1]["text"], "older");
    }

    fn envelope(messages: Value) -> Value {
        json!({ "version": EXPORT_FORMAT_VERSION, "messages": messages })
    }

    #[tokio::test]
    async fn import_inserts_messages_and_returns_count() {
        let (router, repo) = setup().await;

        let payload = envelope(json!([
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
        ]));

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
    async fn import_fails_atomically_on_collision_and_names_the_row() {
        let (router, repo) = setup().await;
        repo.store(&incoming(
            "dup",
            "existing",
            Utc.with_ymd_and_hms(2026, 4, 28, 9, 0, 0).unwrap(),
        ))
        .await
        .unwrap();

        let payload = envelope(json!([
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
        ]));

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
        assert!(parsed["error"].as_str().unwrap().contains("dup"));
        assert_eq!(parsed["conflict"]["source"], "telegram");
        assert_eq!(parsed["conflict"]["source_conversation_id"], "42");
        assert_eq!(parsed["conflict"]["source_message_id"], "dup");

        let entries = repo.fetch_recent("7", 10).await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].entry.text, "existing");
    }

    #[tokio::test]
    async fn import_rejects_unknown_version() {
        let (router, _) = setup().await;

        let payload = json!({ "version": 999, "messages": [] });

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

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let parsed: Value = serde_json::from_slice(&body).unwrap();
        assert!(parsed["error"].as_str().unwrap().contains("999"));
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
