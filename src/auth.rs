//! Bearer-token authentication for the shared HTTP listener (MCP and dashboard).
//!
//! Two modes are supported. With `FROID_AUTH_TOKEN`, a single token guards the
//! whole listener and every request is served from one database. With
//! `FROID_AUTH_TOKENS`, each token belongs to a user (Telegram chat id) and a
//! matching request is tagged with an [`AuthenticatedTenant`] extension so the
//! router can serve it from that user's isolated database.

use std::sync::Arc;

use axum::{
    extract::{Request, State},
    http::{StatusCode, header::AUTHORIZATION},
    middleware::Next,
    response::Response,
};

/// A bearer token bound to one user's tenant (Telegram chat id).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserToken {
    pub chat_id: String,
    pub token: String,
}

/// Token table for per-user authentication.
#[derive(Debug, Clone)]
pub struct UserTokens(Vec<UserToken>);

impl UserTokens {
    pub fn new(tokens: Vec<UserToken>) -> Self {
        Self(tokens)
    }

    /// Returns the chat id owning `provided`, if any. Every entry is compared
    /// in constant time and the scan never exits early, so the response time
    /// does not depend on which (or whether a) token matched.
    fn resolve(&self, provided: &str) -> Option<&str> {
        let mut matched = None;
        for entry in &self.0 {
            if tokens_match(provided, &entry.token) {
                matched = Some(entry.chat_id.as_str());
            }
        }
        matched
    }
}

/// Request extension identifying the tenant resolved from the bearer token.
#[derive(Debug, Clone)]
pub struct AuthenticatedTenant(pub Arc<str>);

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

/// Axum middleware for per-user tokens: rejects requests whose bearer token is
/// not in the table, and tags accepted requests with the owning tenant.
pub async fn require_user_bearer(
    State(tokens): State<Arc<UserTokens>>,
    mut request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let header_value = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok());

    let chat_id = extract_bearer(header_value)
        .and_then(|provided| tokens.resolve(provided))
        .ok_or(StatusCode::UNAUTHORIZED)?;

    request
        .extensions_mut()
        .insert(AuthenticatedTenant(Arc::from(chat_id)));
    Ok(next.run(request).await)
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

    mod per_user {
        use super::super::{AuthenticatedTenant, UserToken, UserTokens, require_user_bearer};

        use axum::{
            Extension, Router,
            body::{Body, to_bytes},
            http::{Request, StatusCode, header::AUTHORIZATION},
            middleware::from_fn_with_state,
            routing::get,
        };
        use std::sync::Arc;
        use tower::ServiceExt;

        fn protected_router() -> Router {
            let tokens = Arc::new(UserTokens::new(vec![
                UserToken {
                    chat_id: "111".into(),
                    token: "alice-secret".into(),
                },
                UserToken {
                    chat_id: "222".into(),
                    token: "bob-secret".into(),
                },
            ]));
            Router::new()
                .route(
                    "/whoami",
                    get(
                        |Extension(tenant): Extension<AuthenticatedTenant>| async move {
                            tenant.0.to_string()
                        },
                    ),
                )
                .layer(from_fn_with_state(tokens, require_user_bearer))
        }

        async fn body_for_token(token: &str) -> (StatusCode, String) {
            let response = protected_router()
                .oneshot(
                    Request::builder()
                        .uri("/whoami")
                        .header(AUTHORIZATION, format!("Bearer {token}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            let status = response.status();
            let body = to_bytes(response.into_body(), 1024).await.unwrap();
            (status, String::from_utf8_lossy(&body).to_string())
        }

        #[tokio::test]
        async fn resolves_each_token_to_its_tenant() {
            assert_eq!(
                body_for_token("alice-secret").await,
                (StatusCode::OK, "111".to_string())
            );
            assert_eq!(
                body_for_token("bob-secret").await,
                (StatusCode::OK, "222".to_string())
            );
        }

        #[tokio::test]
        async fn rejects_unknown_token() {
            let (status, _) = body_for_token("mallory-secret").await;
            assert_eq!(status, StatusCode::UNAUTHORIZED);
        }

        #[tokio::test]
        async fn rejects_missing_header() {
            let response = protected_router()
                .oneshot(
                    Request::builder()
                        .uri("/whoami")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }
    }
}
