use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use chrono::{DateTime, Days, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use tracing::error;

use crate::{
    journal::repository::JournalRepository,
    journal::review::repository::{DailyReviewRepository, DailyReviewRepositoryError},
    journal::transfer::{self, TransferError},
    journal::week_review::repository::{WeeklyReviewRepository, WeeklyReviewRepositoryError},
    messages::{IncomingMessage, MessageSource},
    prompts::{PromptKey, PromptRepository, load_default, registry::version_for},
};

const DEFAULT_ENTRIES_LIMIT: u32 = 50;
const MAX_ENTRIES_LIMIT: u32 = 200;
/// Default review range when `from` is omitted: the last 30 days.
const DEFAULT_REVIEW_RANGE_DAYS: u64 = 30;

pub fn router(pool: &SqlitePool, capture_conversation_id: &str) -> Router {
    let journal = JournalRepository::new(pool.clone());

    let messages = Router::new()
        .route("/messages/export", get(export_messages))
        .route("/messages/import", post(import_messages))
        .with_state(journal.clone());

    let capture = Router::new()
        .route("/messages", post(capture_message))
        .with_state(CaptureState {
            journal: journal.clone(),
            conversation_id: Arc::from(capture_conversation_id),
        });

    let entries = Router::new()
        .route("/entries", get(list_entries))
        .with_state(journal);

    let daily_reviews = Router::new()
        .route("/reviews/daily", get(list_daily_reviews))
        .with_state(DailyReviewRepository::new(pool.clone()));

    let weekly_reviews = Router::new()
        .route("/reviews/weekly", get(list_weekly_reviews))
        .with_state(WeeklyReviewRepository::new(pool.clone()));

    let prompts = Router::new()
        .route("/prompts", get(list_prompts))
        .route(
            "/prompts/{key}",
            get(get_prompt).put(update_prompt).delete(reset_prompt),
        )
        .with_state(PromptRepository::new(pool.clone()));

    Router::new()
        .merge(messages)
        .merge(capture)
        .merge(entries)
        .merge(daily_reviews)
        .merge(weekly_reviews)
        .merge(prompts)
}

#[derive(Debug)]
enum DashboardError {
    Transfer(TransferError),
    EmptyCapture,
    CaptureRepository(sqlx::Error),
    CaptureConflict,
    InvalidLimit { max: u32 },
    EntriesRepository(sqlx::Error),
    InvalidRange { from: NaiveDate, to: NaiveDate },
    DailyReviews(DailyReviewRepositoryError),
    WeeklyReviews(WeeklyReviewRepositoryError),
}

impl IntoResponse for DashboardError {
    fn into_response(self) -> Response {
        match self {
            Self::Transfer(error) => {
                let status = match &error {
                    TransferError::InvalidPayload(_) | TransferError::UnsupportedVersion { .. } => {
                        StatusCode::BAD_REQUEST
                    }
                    TransferError::Conflict { .. } => StatusCode::CONFLICT,
                    TransferError::Storage(_) | TransferError::Serialization(_) => {
                        error!(error = %error, "journal transfer failed");
                        StatusCode::INTERNAL_SERVER_ERROR
                    }
                };
                let conflict = match &error {
                    TransferError::Conflict {
                        source,
                        source_conversation_id,
                        source_message_id,
                    } => Some(ConflictDetails {
                        source: source.clone(),
                        source_conversation_id: source_conversation_id.clone(),
                        source_message_id: source_message_id.clone(),
                    }),
                    _ => None,
                };
                (
                    status,
                    Json(ImportError {
                        error: error.to_string(),
                        conflict,
                    }),
                )
                    .into_response()
            }
            Self::EmptyCapture => {
                (StatusCode::BAD_REQUEST, "text must not be empty").into_response()
            }
            Self::CaptureRepository(err) => {
                error!(error = %err, "failed to store captured journal entry");
                (StatusCode::INTERNAL_SERVER_ERROR, "failed to store entry").into_response()
            }
            Self::CaptureConflict => {
                error!("captured journal entry collided with an existing message id");
                (StatusCode::INTERNAL_SERVER_ERROR, "failed to store entry").into_response()
            }
            Self::InvalidLimit { max } => (
                StatusCode::BAD_REQUEST,
                format!("limit must be between 1 and {max}"),
            )
                .into_response(),
            Self::EntriesRepository(err) => {
                error!(error = %err, "failed to fetch journal entries");
                (StatusCode::INTERNAL_SERVER_ERROR, "failed to load entries").into_response()
            }
            Self::InvalidRange { from, to } => (
                StatusCode::BAD_REQUEST,
                format!("from ({from}) must be strictly before to ({to})"),
            )
                .into_response(),
            Self::DailyReviews(err) => {
                error!(error = %err, "failed to fetch daily reviews");
                (StatusCode::INTERNAL_SERVER_ERROR, "failed to load reviews").into_response()
            }
            Self::WeeklyReviews(err) => {
                error!(error = %err, "failed to fetch weekly reviews");
                (StatusCode::INTERNAL_SERVER_ERROR, "failed to load reviews").into_response()
            }
        }
    }
}

#[derive(Clone)]
struct CaptureState {
    journal: JournalRepository,
    conversation_id: Arc<str>,
}

#[derive(Deserialize)]
struct CaptureRequest {
    text: String,
}

#[derive(Serialize)]
struct EntryView {
    id: String,
    text: String,
    received_at: DateTime<Utc>,
}

#[derive(Serialize)]
struct EntriesResponse {
    entries: Vec<EntryView>,
}

#[derive(Deserialize)]
struct EntriesQuery {
    limit: Option<u32>,
}

#[derive(Deserialize)]
struct ReviewRangeQuery {
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
}

#[derive(Serialize)]
struct DailyReviewView {
    review_date: NaiveDate,
    review_text: String,
    created_at: DateTime<Utc>,
}

#[derive(Serialize)]
struct DailyReviewsResponse {
    reviews: Vec<DailyReviewView>,
}

#[derive(Serialize)]
struct WeeklyReviewView {
    week_start: NaiveDate,
    week_end: NaiveDate,
    review_text: String,
    created_at: DateTime<Utc>,
}

#[derive(Serialize)]
struct WeeklyReviewsResponse {
    reviews: Vec<WeeklyReviewView>,
}

async fn capture_message(
    State(state): State<CaptureState>,
    Json(request): Json<CaptureRequest>,
) -> Result<impl IntoResponse, DashboardError> {
    let text = request.text.trim();
    if text.is_empty() {
        return Err(DashboardError::EmptyCapture);
    }

    let message = IncomingMessage {
        source: MessageSource::Web,
        source_conversation_id: state.conversation_id.to_string(),
        source_message_id: format!("web-{}", ulid::Ulid::new()),
        user_id: state.conversation_id.to_string(),
        text: text.to_string(),
        received_at: Utc::now(),
    };

    let id = state
        .journal
        .store(&message)
        .await
        .map_err(DashboardError::CaptureRepository)?
        .ok_or(DashboardError::CaptureConflict)?;

    Ok((
        StatusCode::CREATED,
        Json(EntryView {
            id,
            text: message.text,
            received_at: message.received_at,
        }),
    ))
}

async fn list_entries(
    State(repo): State<JournalRepository>,
    Query(query): Query<EntriesQuery>,
) -> Result<Json<EntriesResponse>, DashboardError> {
    let limit = query.limit.unwrap_or(DEFAULT_ENTRIES_LIMIT);
    if limit == 0 || limit > MAX_ENTRIES_LIMIT {
        return Err(DashboardError::InvalidLimit {
            max: MAX_ENTRIES_LIMIT,
        });
    }

    let entries = repo
        .fetch_recent(limit)
        .await
        .map_err(DashboardError::EntriesRepository)?
        .into_iter()
        .map(|stored| EntryView {
            id: stored.id,
            text: stored.entry.text,
            received_at: stored.entry.received_at,
        })
        .collect();

    Ok(Json(EntriesResponse { entries }))
}

/// Resolve an optional `from`/`to` pair to a concrete half-open range,
/// defaulting to the last [`DEFAULT_REVIEW_RANGE_DAYS`] days up to tomorrow.
fn resolve_review_range(
    query: &ReviewRangeQuery,
) -> Result<(NaiveDate, NaiveDate), DashboardError> {
    let today = Utc::now().date_naive();
    let to = query.to.unwrap_or_else(|| today + Days::new(1));
    let from = query
        .from
        .unwrap_or_else(|| to - Days::new(DEFAULT_REVIEW_RANGE_DAYS));
    if from >= to {
        return Err(DashboardError::InvalidRange { from, to });
    }
    Ok((from, to))
}

async fn list_daily_reviews(
    State(repo): State<DailyReviewRepository>,
    Query(query): Query<ReviewRangeQuery>,
) -> Result<Json<DailyReviewsResponse>, DashboardError> {
    let (from, to) = resolve_review_range(&query)?;

    let reviews = repo
        .fetch_completed_in_range(from, to)
        .await
        .map_err(DashboardError::DailyReviews)?
        .into_iter()
        .map(|review| DailyReviewView {
            review_date: review.review_date,
            review_text: review.review_text.unwrap_or_default(),
            created_at: review.created_at,
        })
        .collect();

    Ok(Json(DailyReviewsResponse { reviews }))
}

async fn list_weekly_reviews(
    State(repo): State<WeeklyReviewRepository>,
    Query(query): Query<ReviewRangeQuery>,
) -> Result<Json<WeeklyReviewsResponse>, DashboardError> {
    let (from, to) = resolve_review_range(&query)?;

    let reviews = repo
        .fetch_completed_in_range(from, to)
        .await
        .map_err(DashboardError::WeeklyReviews)?
        .into_iter()
        .map(|review| WeeklyReviewView {
            week_start: review.week_start_date,
            week_end: review.week_start_date + Days::new(6),
            review_text: review.review_text.unwrap_or_default(),
            created_at: review.created_at,
        })
        .collect();

    Ok(Json(WeeklyReviewsResponse { reviews }))
}

async fn export_messages(
    State(repo): State<JournalRepository>,
) -> Result<impl IntoResponse, DashboardError> {
    let export = transfer::export_json(&repo)
        .await
        .map_err(DashboardError::Transfer)?;

    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/json".to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{}\"", export.filename),
            ),
        ],
        export.bytes,
    ))
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
    body: axum::body::Bytes,
) -> Result<impl IntoResponse, DashboardError> {
    let imported = transfer::import_json(&repo, &body)
        .await
        .map_err(DashboardError::Transfer)?;

    Ok((StatusCode::OK, Json(ImportResult { imported })).into_response())
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

fn parse_prompt_key(raw: &str) -> Option<PromptKey> {
    PromptKey::parse(raw)
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
        Some(k) => k,
        None => {
            return api_error(
                StatusCode::NOT_FOUND,
                format!("unknown prompt key '{raw_key}'"),
            );
        }
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
        Some(k) => k,
        None => {
            return api_error(
                StatusCode::NOT_FOUND,
                format!("unknown prompt key '{raw_key}'"),
            );
        }
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
        Some(k) => k,
        None => {
            return api_error(
                StatusCode::NOT_FOUND,
                format!("unknown prompt key '{raw_key}'"),
            );
        }
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
    use crate::journal::transfer::EXPORT_FORMAT_VERSION;
    use crate::messages::{IncomingMessage, MessageSource};
    use axum::body::{Body, to_bytes};
    use axum::http::Request;
    use chrono::TimeZone;
    use serde_json::{Value, json};
    use sqlx::SqlitePool;
    use tower::ServiceExt;

    const TEST_CAPTURE_CONVERSATION_ID: &str = "42";

    async fn setup_with_pool() -> (Router, SqlitePool) {
        database::register_sqlite_vec_extension();
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        (router(&pool, TEST_CAPTURE_CONVERSATION_ID), pool)
    }

    async fn setup() -> (Router, JournalRepository) {
        let (router, pool) = setup_with_pool().await;
        (router, JournalRepository::new(pool))
    }

    async fn setup_with_prompts() -> (Router, PromptRepository) {
        let (router, pool) = setup_with_pool().await;
        (router, PromptRepository::new(pool))
    }

    fn at(hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 4, 28, hour, 0, 0).unwrap()
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

    async fn get_json(router: &Router, uri: &str) -> (StatusCode, Value) {
        let response = router
            .clone()
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        let value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        (status, value)
    }

    async fn post_json(router: &Router, uri: &str, payload: Value) -> (StatusCode, Value) {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(uri)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(payload.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        let value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        (status, value)
    }

    #[tokio::test]
    async fn capture_stores_entry_under_capture_conversation() {
        let (router, repo) = setup().await;

        let (status, body) =
            post_json(&router, "/messages", json!({"text": "  captured note  "})).await;

        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(body["text"], "captured note");
        assert!(body["id"].is_string());

        let records = repo.fetch_all_for_export().await.unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].source, "web");
        assert_eq!(
            records[0].source_conversation_id,
            TEST_CAPTURE_CONVERSATION_ID
        );
        assert_eq!(records[0].text, "captured note");
    }

    #[tokio::test]
    async fn capture_rejects_blank_text() {
        let (router, repo) = setup().await;

        let (status, _) = post_json(&router, "/messages", json!({"text": "   "})).await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(repo.fetch_all_for_export().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn entries_returns_recent_entries_with_default_limit() {
        let (router, repo) = setup().await;
        repo.store(&incoming("m1", "first note", at(10)))
            .await
            .unwrap();
        repo.store(&incoming("m2", "second note", at(11)))
            .await
            .unwrap();

        let (status, body) = get_json(&router, "/entries").await;

        assert_eq!(status, StatusCode::OK);
        let entries = body["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 2);
        // Most recent first, as fetch_recent orders by recency.
        assert_eq!(entries[0]["text"], "second note");
        assert_eq!(entries[1]["text"], "first note");
    }

    #[tokio::test]
    async fn entries_respects_explicit_limit() {
        let (router, repo) = setup().await;
        repo.store(&incoming("m1", "first note", at(10)))
            .await
            .unwrap();
        repo.store(&incoming("m2", "second note", at(11)))
            .await
            .unwrap();

        let (status, body) = get_json(&router, "/entries?limit=1").await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["entries"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn entries_rejects_invalid_limit() {
        let (router, _) = setup().await;

        let (status, _) = get_json(&router, "/entries?limit=0").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        let (status, _) = get_json(&router, "/entries?limit=10000").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn daily_reviews_returns_completed_reviews_in_range() {
        let (router, pool) = setup_with_pool().await;
        let reviews = crate::journal::review::repository::DailyReviewRepository::new(pool);
        reviews
            .upsert_completed(
                chrono::NaiveDate::from_ymd_opt(2026, 6, 1).unwrap(),
                "a calm day",
                "model",
                "v1",
            )
            .await
            .unwrap();

        let (status, body) =
            get_json(&router, "/reviews/daily?from=2026-06-01&to=2026-06-08").await;

        assert_eq!(status, StatusCode::OK);
        let reviews = body["reviews"].as_array().unwrap();
        assert_eq!(reviews.len(), 1);
        assert_eq!(reviews[0]["review_date"], "2026-06-01");
        assert_eq!(reviews[0]["review_text"], "a calm day");
    }

    #[tokio::test]
    async fn daily_reviews_excludes_reviews_outside_range() {
        let (router, pool) = setup_with_pool().await;
        let reviews = crate::journal::review::repository::DailyReviewRepository::new(pool);
        reviews
            .upsert_completed(
                chrono::NaiveDate::from_ymd_opt(2026, 5, 1).unwrap(),
                "out of range",
                "model",
                "v1",
            )
            .await
            .unwrap();

        let (status, body) =
            get_json(&router, "/reviews/daily?from=2026-06-01&to=2026-06-08").await;

        assert_eq!(status, StatusCode::OK);
        assert!(body["reviews"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn daily_reviews_rejects_inverted_range() {
        let (router, _) = setup().await;

        let (status, _) = get_json(&router, "/reviews/daily?from=2026-06-08&to=2026-06-01").await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn weekly_reviews_returns_completed_reviews_with_week_end() {
        let (router, pool) = setup_with_pool().await;
        let reviews = crate::journal::week_review::repository::WeeklyReviewRepository::new(pool);
        reviews
            .upsert_completed(
                chrono::NaiveDate::from_ymd_opt(2026, 6, 1).unwrap(),
                "a steady week",
                "model",
                "v1",
                "{}",
            )
            .await
            .unwrap();

        let (status, body) =
            get_json(&router, "/reviews/weekly?from=2026-06-01&to=2026-06-15").await;

        assert_eq!(status, StatusCode::OK);
        let reviews = body["reviews"].as_array().unwrap();
        assert_eq!(reviews.len(), 1);
        assert_eq!(reviews[0]["week_start"], "2026-06-01");
        assert_eq!(reviews[0]["week_end"], "2026-06-07");
        assert_eq!(reviews[0]["review_text"], "a steady week");
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

        let entries = repo.fetch_recent(10).await.unwrap();
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

        let entries = repo.fetch_recent(10).await.unwrap();
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
