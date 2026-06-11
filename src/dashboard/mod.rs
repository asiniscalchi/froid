use axum::{
    Router,
    body::Body,
    http::{StatusCode, Uri, header},
    response::{IntoResponse, Response},
};
use rust_embed::RustEmbed;

mod api;

#[derive(RustEmbed)]
#[folder = "web/dist/"]
struct Assets;

/// Data endpoints under `/api`. These carry journal content and must sit
/// behind the bearer-auth layer. `capture_conversation_id` is the
/// conversation entries captured via `POST /api/messages` are filed under —
/// the owning user's chat id, so captured entries flow through the same
/// review pipeline as Telegram messages.
pub fn api_router(pool: &sqlx::SqlitePool, capture_conversation_id: &str) -> Router {
    Router::new().nest("/api", api::router(pool, capture_conversation_id))
}

/// Static SPA shell (embedded assets + index.html fallback). Contains no user
/// data, so it is served without authentication — the webapp itself prompts
/// for the bearer token and sends it on every `/api` call.
pub fn spa_router() -> Router {
    Router::new().fallback(spa_handler)
}

async fn spa_handler(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };

    match Assets::get(path) {
        Some(file) => respond_with(path, file.data.into_owned()),
        None => match Assets::get("index.html") {
            Some(file) => respond_with("index.html", file.data.into_owned()),
            None => (StatusCode::NOT_FOUND, "not found").into_response(),
        },
    }
}

fn respond_with(path: &str, bytes: Vec<u8>) -> Response {
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime.as_ref())
        .body(Body::from(bytes))
        .expect("build dashboard response")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database;
    use axum::body::to_bytes;
    use axum::http::Request;
    use sqlx::SqlitePool;
    use tower::ServiceExt;

    async fn test_router() -> Router {
        database::register_sqlite_vec_extension();
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        api_router(&pool, "42").merge(spa_router())
    }

    #[tokio::test]
    async fn root_serves_index_html() {
        let response = test_router()
            .await
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
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
            "text/html"
        );
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        assert!(String::from_utf8_lossy(&body).contains("<div id=\"root\">"));
    }

    #[tokio::test]
    async fn unknown_path_falls_back_to_index_html() {
        let response = test_router()
            .await
            .oneshot(
                Request::builder()
                    .uri("/some/spa/route")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        assert!(String::from_utf8_lossy(&body).contains("<div id=\"root\">"));
    }
}
