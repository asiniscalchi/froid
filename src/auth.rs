//! Bearer-token authentication for the shared HTTP listener (MCP and dashboard).
//!
//! When a token is configured, every request to the HTTP endpoints must carry a
//! matching `Authorization: Bearer <token>` header. The middleware is applied as
//! a single layer over the merged router, so it protects both the MCP transport
//! and the dashboard at once.

use std::sync::Arc;

use axum::{
    extract::{Request, State},
    http::{StatusCode, header::AUTHORIZATION},
    middleware::Next,
    response::Response,
};

/// Returns the bearer token from an `Authorization` header value, if present.
fn extract_bearer(header_value: Option<&str>) -> Option<&str> {
    header_value?.strip_prefix("Bearer ")
}

/// Constant-time comparison to avoid leaking token contents via timing.
fn tokens_match(provided: &str, expected: &str) -> bool {
    let provided = provided.as_bytes();
    let expected = expected.as_bytes();
    if provided.len() != expected.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in provided.iter().zip(expected.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}

/// Axum middleware that rejects requests without a matching bearer token.
pub async fn require_bearer(
    State(expected): State<Arc<str>>,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let header_value = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok());

    match extract_bearer(header_value) {
        Some(provided) if tokens_match(provided, &expected) => Ok(next.run(request).await),
        _ => Err(StatusCode::UNAUTHORIZED),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_bearer_token() {
        assert_eq!(extract_bearer(Some("Bearer secret")), Some("secret"));
    }

    #[test]
    fn ignores_non_bearer_scheme() {
        assert_eq!(extract_bearer(Some("Basic secret")), None);
        assert_eq!(extract_bearer(Some("secret")), None);
        assert_eq!(extract_bearer(None), None);
    }

    #[test]
    fn matches_identical_tokens() {
        assert!(tokens_match("abc123", "abc123"));
    }

    #[test]
    fn rejects_different_tokens() {
        assert!(!tokens_match("abc123", "abc124"));
        assert!(!tokens_match("abc", "abc123"));
        assert!(!tokens_match("", "abc"));
    }

    mod middleware {
        use super::super::require_bearer;

        use axum::{
            Router,
            body::Body,
            http::{Request, StatusCode, header::AUTHORIZATION},
            middleware::from_fn_with_state,
            routing::get,
        };
        use std::sync::Arc;
        use tower::ServiceExt;

        fn protected_router() -> Router {
            let token: Arc<str> = Arc::from("secret");
            Router::new()
                .route("/", get(|| async { "ok" }))
                .layer(from_fn_with_state(token, require_bearer))
        }

        async fn status_for(request: Request<Body>) -> StatusCode {
            protected_router().oneshot(request).await.unwrap().status()
        }

        #[tokio::test]
        async fn rejects_request_without_authorization_header() {
            let request = Request::builder().uri("/").body(Body::empty()).unwrap();

            assert_eq!(status_for(request).await, StatusCode::UNAUTHORIZED);
        }

        #[tokio::test]
        async fn rejects_request_with_wrong_token() {
            let request = Request::builder()
                .uri("/")
                .header(AUTHORIZATION, "Bearer wrong")
                .body(Body::empty())
                .unwrap();

            assert_eq!(status_for(request).await, StatusCode::UNAUTHORIZED);
        }

        #[tokio::test]
        async fn accepts_request_with_correct_token() {
            let request = Request::builder()
                .uri("/")
                .header(AUTHORIZATION, "Bearer secret")
                .body(Body::empty())
                .unwrap();

            assert_eq!(status_for(request).await, StatusCode::OK);
        }
    }
}
