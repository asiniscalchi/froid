use axum::{
    Json, Router,
    extract::{Path, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::error;

use crate::{
    journal::repository::{BulkImportError, JournalEntryRecord, JournalRepository},
    prompts::{PromptKey, PromptRepository, load_default, registry::version_for},
};

pub const EXPORT_FORMAT_VERSION: u32 = 2;
pub const MIN_SUPPORTED_IMPORT_VERSION: u32 = 1;

pub fn router(journal: JournalRepository, prompts: PromptRepository) -> Router {
    let messages = Router::new()
        .route("/messages/export", get(export_messages))
        .route("/messages/import", post(import_messages))
        .with_state(journal);

    let prompts = Router::new()
        .route("/prompts", get(list_prompts))
        .route(
            "/prompts/{key}",
            get(get_prompt).put(update_prompt).delete(reset_prompt),
        )
        .with_state(prompts);

    Router::new().merge(messages).merge(prompts)
}

#[derive(Serialize)]
struct ExportedMessage {
    id: String,
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
                id: r.id,
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
    #[serde(default)]
    id: Option<String>,
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
    if envelope.version < MIN_SUPPORTED_IMPORT_VERSION || envelope.version > EXPORT_FORMAT_VERSION {
        return (
            StatusCode::BAD_REQUEST,
            Json(ImportError {
                error: format!(
                    "unsupported export version {} (supported {}..={})",
                    envelope.version, MIN_SUPPORTED_IMPORT_VERSION, EXPORT_FORMAT_VERSION
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
            id: m.id.unwrap_or_default(),
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

#[derive(Serialize)]
struct PromptListItem {
    key: &'static str,
    label: &'static str,
    default_version: String,
    is_customized: bool,
    updated_at: Option<DateTime<Utc>>,
}

#[derive(Serialize)]
struct PromptDetail {
    key: &'static str,
    label: &'static str,
    default_version: String,
    current_version: String,
    default_text: String,
    current_text: String,
    is_customized: bool,
    updated_at: Option<DateTime<Utc>>,
}

#[derive(Deserialize)]
struct UpdatePromptBody {
    content: String,
}

#[derive(Serialize)]
struct ApiError {
    error: String,
}

fn api_error(status: StatusCode, message: impl Into<String>) -> Response {
    (
        status,
        Json(ApiError {
            error: message.into(),
        }),
    )
        .into_response()
}

#[allow(clippy::result_large_err)]
fn parse_prompt_key(raw: &str) -> Result<PromptKey, Response> {
    PromptKey::parse(raw)
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, format!("unknown prompt key '{raw}'")))
}

async fn list_prompts(State(repo): State<PromptRepository>) -> Response {
    let rows = match repo.list_all().await {
        Ok(rows) => rows,
        Err(err) => {
            error!(error = %err, "failed to list customized prompts");
            return api_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to list prompts");
        }
    };

    let items: Vec<PromptListItem> = PromptKey::ALL
        .into_iter()
        .map(|key| {
            let row = rows.iter().find(|r| r.prompt_key == key.as_str());
            let default_path = key.default_path();
            PromptListItem {
                key: key.as_str(),
                label: key.label(),
                default_version: version_for(&default_path, false),
                is_customized: row.is_some(),
                updated_at: row.map(|r| r.updated_at),
            }
        })
        .collect();

    (StatusCode::OK, Json(items)).into_response()
}

async fn get_prompt(State(repo): State<PromptRepository>, Path(raw_key): Path<String>) -> Response {
    let key = match parse_prompt_key(&raw_key) {
        Ok(k) => k,
        Err(resp) => return resp,
    };

    let default_path = key.default_path();
    let default = match load_default(key, &default_path) {
        Ok(d) => d,
        Err(err) => {
            error!(error = %err, "failed to load default prompt");
            return api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to load default prompt: {err}"),
            );
        }
    };

    let row = match repo.get(key.as_str()).await {
        Ok(row) => row,
        Err(err) => {
            error!(error = %err, "failed to read customized prompt");
            return api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to read prompt customization",
            );
        }
    };

    let (current_text, current_version, is_customized, updated_at) = match row {
        Some(row) => (
            row.content,
            version_for(&default_path, true),
            true,
            Some(row.updated_at),
        ),
        None => (default.text.clone(), default.version.clone(), false, None),
    };

    (
        StatusCode::OK,
        Json(PromptDetail {
            key: key.as_str(),
            label: key.label(),
            default_version: default.version,
            current_version,
            default_text: default.text,
            current_text,
            is_customized,
            updated_at,
        }),
    )
        .into_response()
}

async fn update_prompt(
    State(repo): State<PromptRepository>,
    Path(raw_key): Path<String>,
    Json(body): Json<UpdatePromptBody>,
) -> Response {
    let key = match parse_prompt_key(&raw_key) {
        Ok(k) => k,
        Err(resp) => return resp,
    };

    if body.content.trim().is_empty() {
        return api_error(StatusCode::BAD_REQUEST, "prompt content must not be empty");
    }

    let row = match repo.upsert(key.as_str(), &body.content).await {
        Ok(row) => row,
        Err(err) => {
            error!(error = %err, "failed to upsert customized prompt");
            return api_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to save prompt");
        }
    };

    let default_path = key.default_path();
    (
        StatusCode::OK,
        Json(PromptDetail {
            key: key.as_str(),
            label: key.label(),
            default_version: version_for(&default_path, false),
            current_version: version_for(&default_path, true),
            default_text: String::new(),
            current_text: row.content,
            is_customized: true,
            updated_at: Some(row.updated_at),
        }),
    )
        .into_response()
}

async fn reset_prompt(
    State(repo): State<PromptRepository>,
    Path(raw_key): Path<String>,
) -> Response {
    let key = match parse_prompt_key(&raw_key) {
        Ok(k) => k,
        Err(resp) => return resp,
    };

    if let Err(err) = repo.delete(key.as_str()).await {
        error!(error = %err, "failed to delete customized prompt");
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to reset prompt");
    }

    StatusCode::NO_CONTENT.into_response()
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
        let journal = JournalRepository::new(pool.clone());
        let prompts = PromptRepository::new(pool);
        (router(journal.clone(), prompts), journal)
    }

    async fn setup_with_prompts() -> (Router, PromptRepository) {
        database::register_sqlite_vec_extension();
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        let journal = JournalRepository::new(pool.clone());
        let prompts = PromptRepository::new(pool);
        (router(journal, prompts.clone()), prompts)
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
        assert!(
            messages[0]
                .get("id")
                .and_then(|v| v.as_str())
                .is_some_and(|s| !s.is_empty()),
            "id must be exported as a non-empty string"
        );
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

    #[tokio::test]
    async fn export_uses_v2_envelope_and_round_trips_ids() {
        let (router, repo) = setup().await;
        repo.store(&incoming(
            "1",
            "round trip",
            Utc.with_ymd_and_hms(2026, 4, 28, 10, 0, 0).unwrap(),
        ))
        .await
        .unwrap();

        let export_response = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/messages/export")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(export_response.into_body(), 64 * 1024)
            .await
            .unwrap();
        let payload: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(payload["version"], 2);
        let exported_id = payload["messages"][0]["id"].as_str().unwrap().to_string();
        assert!(!exported_id.is_empty());

        let (router2, repo2) = setup().await;
        let response = router2
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

        let round_tripped: String = sqlx::query_scalar("SELECT id FROM journal_entries")
            .fetch_one(repo2.pool())
            .await
            .unwrap();
        assert_eq!(round_tripped, exported_id);
    }

    #[tokio::test]
    async fn import_accepts_v1_payload_without_id_and_generates_one() {
        let (router, repo) = setup().await;
        let payload = json!({
            "version": 1,
            "messages": [{
                "source": "telegram",
                "source_conversation_id": "42",
                "source_message_id": "v1-msg",
                "text": "from v1 export",
                "received_at": "2026-04-28T10:00:00Z"
            }]
        });

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
        let id: String = sqlx::query_scalar("SELECT id FROM journal_entries")
            .fetch_one(repo.pool())
            .await
            .unwrap();
        assert!(!id.is_empty(), "v1 import must assign a fresh id");
    }

    #[tokio::test]
    async fn list_prompts_returns_one_entry_per_known_key() {
        let (router, _) = setup_with_prompts().await;

        let response = router
            .oneshot(
                Request::builder()
                    .uri("/prompts")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let parsed: Value = serde_json::from_slice(&body).unwrap();
        let items = parsed.as_array().unwrap();
        assert_eq!(items.len(), PromptKey::ALL.len());
        for item in items {
            assert!(item["key"].is_string());
            assert!(item["label"].is_string());
            assert!(item["default_version"].is_string());
            assert_eq!(item["is_customized"], false);
            assert!(item["updated_at"].is_null());
        }
    }

    #[tokio::test]
    async fn list_prompts_reflects_customization_state_after_upsert() {
        let (router, prompts) = setup_with_prompts().await;
        prompts
            .upsert(PromptKey::DailyReview.as_str(), "custom body")
            .await
            .unwrap();

        let response = router
            .oneshot(
                Request::builder()
                    .uri("/prompts")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let parsed: Value = serde_json::from_slice(&body).unwrap();
        let item = parsed
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["key"] == "daily_review")
            .unwrap();
        assert_eq!(item["is_customized"], true);
        assert!(item["updated_at"].is_string());
    }

    #[tokio::test]
    async fn get_prompt_returns_default_text_when_uncustomized() {
        let (router, _) = setup_with_prompts().await;

        let response = router
            .oneshot(
                Request::builder()
                    .uri("/prompts/daily_review")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        let parsed: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed["key"], "daily_review");
        assert_eq!(parsed["is_customized"], false);
        assert_eq!(parsed["default_text"], parsed["current_text"]);
        assert_eq!(parsed["default_version"], parsed["current_version"]);
        assert!(!parsed["default_text"].as_str().unwrap().is_empty());
    }

    #[tokio::test]
    async fn get_prompt_returns_404_for_unknown_key() {
        let (router, _) = setup_with_prompts().await;

        let response = router
            .oneshot(
                Request::builder()
                    .uri("/prompts/unknown_key")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn update_prompt_persists_content_and_marks_as_customized() {
        let (router, prompts) = setup_with_prompts().await;

        let response = router
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/prompts/daily_review")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        json!({ "content": "new prompt body" }).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let stored = prompts.get(PromptKey::DailyReview.as_str()).await.unwrap();
        let stored = stored.expect("upsert should persist");
        assert_eq!(stored.content, "new prompt body");
    }

    #[tokio::test]
    async fn update_prompt_rejects_empty_content() {
        let (router, _) = setup_with_prompts().await;

        let response = router
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/prompts/daily_review")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(json!({ "content": "   \n" }).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn delete_prompt_removes_customization_and_is_idempotent() {
        let (router, prompts) = setup_with_prompts().await;
        prompts
            .upsert(PromptKey::DailyReview.as_str(), "custom body")
            .await
            .unwrap();

        let first = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/prompts/daily_review")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::NO_CONTENT);
        assert!(
            prompts
                .get(PromptKey::DailyReview.as_str())
                .await
                .unwrap()
                .is_none()
        );

        let second = router
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/prompts/daily_review")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(second.status(), StatusCode::NO_CONTENT);
    }
}
