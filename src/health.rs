//! Liveness endpoint for the shared HTTP listener.
//!
//! `GET /health` answers `200 OK` with the service name and version. It is
//! merged into the listener *after* the bearer-auth layer is applied, so it
//! stays reachable without credentials — supervisors and container
//! healthchecks must be able to probe it without holding a token.

use axum::{Json, Router, routing::get};
use serde_json::{Value, json};

use crate::version;

/// Router exposing `GET /health`. Merge after any auth layer so the probe
/// stays unauthenticated.
pub fn router() -> Router {
    Router::new().route("/health", get(health))
}

async fn health() -> Json<Value> {
    Json(json!({
        "status": "ok",
        "name": env!("CARGO_PKG_NAME"),
        "version": version::VERSION,
    }))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::{
        Router,
        body::{Body, to_bytes},
        http::{Request, StatusCode},
        middleware::from_fn_with_state,
        routing::get,
    };
    use tower::ServiceExt;

    use super::router;

    async fn response_for(app: Router, uri: &str) -> axum::response::Response {
        app.oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn health_returns_ok_with_version() {
        let response = response_for(router(), "/health").await;

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(payload["status"], "ok");
        assert_eq!(payload["version"], crate::version::VERSION);
    }

    #[tokio::test]
    async fn health_bypasses_bearer_auth_when_merged_after_layer() {
        // Mirror the listener assembly in `http::build_per_user_app`:
        // protected routes first, auth layer, then the health router merged
        // on top.
        let pool = crate::database::test_pool().await;
        let resolver = Arc::new(crate::auth::TokenResolver::new(
            crate::tokens::UserTokenStore::new(pool),
        ));

        let app = Router::new()
            .route("/protected", get(|| async { "ok" }))
            .layer(from_fn_with_state(
                resolver,
                crate::auth::require_user_bearer,
            ))
            .merge(router());

        let health = response_for(app.clone(), "/health").await;
        assert_eq!(health.status(), StatusCode::OK);

        let protected = response_for(app, "/protected").await;
        assert_eq!(protected.status(), StatusCode::UNAUTHORIZED);
    }
}
