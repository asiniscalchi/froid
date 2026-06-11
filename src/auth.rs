//! Bearer-token authentication for the shared HTTP listener (MCP and dashboard).
//!
//! Tokens are minted by users themselves through the Telegram `/token`
//! command and stored hashed in the central database. A matching request is
//! tagged with an [`AuthenticatedTenant`] extension so the router can serve
//! it from that user's isolated database. When authentication is disabled
//! (`FROID_AUTH_ENABLED` unset), no middleware is installed and access must
//! be restricted at the network level.

use std::sync::Arc;

use axum::{
    extract::{Request, State},
    http::{StatusCode, header::AUTHORIZATION},
    middleware::Next,
    response::Response,
};

use crate::tokens::{UserTokenStore, hash_token};

/// Request extension identifying the tenant resolved from the bearer token.
#[derive(Debug, Clone)]
pub struct AuthenticatedTenant(pub Arc<str>);

/// Resolves a presented bearer token to the tenant that minted it via the
/// Telegram `/token` command.
#[derive(Clone)]
pub struct TokenResolver {
    issued: UserTokenStore,
}

impl TokenResolver {
    pub fn new(issued: UserTokenStore) -> Self {
        Self { issued }
    }

    async fn resolve(&self, provided: &str) -> Option<String> {
        match self
            .issued
            .find_chat_id_by_hash(&hash_token(provided))
            .await
        {
            Ok(found) => found,
            Err(err) => {
                tracing::error!(error = %err, "failed to look up issued bearer token");
                None
            }
        }
    }
}

/// Returns the bearer token from an `Authorization` header value, if present.
fn extract_bearer(header_value: Option<&str>) -> Option<&str> {
    header_value?.strip_prefix("Bearer ")
}

/// Axum middleware: rejects requests whose bearer token is not known to the
/// resolver, and tags accepted requests with the owning tenant.
pub async fn require_user_bearer(
    State(resolver): State<Arc<TokenResolver>>,
    mut request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let header_value = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok());

    let provided = extract_bearer(header_value).ok_or(StatusCode::UNAUTHORIZED)?;
    let chat_id = resolver
        .resolve(provided)
        .await
        .ok_or(StatusCode::UNAUTHORIZED)?;

    request
        .extensions_mut()
        .insert(AuthenticatedTenant(Arc::from(chat_id.as_str())));
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

    mod middleware {
        use super::super::{AuthenticatedTenant, TokenResolver, require_user_bearer};
        use crate::tokens::{TokenIssuer, UserTokenStore};

        use axum::{
            Extension, Router,
            body::{Body, to_bytes},
            http::{Request, StatusCode, header::AUTHORIZATION},
            middleware::from_fn_with_state,
            routing::get,
        };
        use std::sync::Arc;
        use tower::ServiceExt;

        async fn store() -> UserTokenStore {
            crate::database::register_sqlite_vec_extension();
            let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
            sqlx::migrate!().run(&pool).await.unwrap();
            UserTokenStore::new(pool)
        }

        fn router_with(store: UserTokenStore) -> Router {
            Router::new()
                .route(
                    "/whoami",
                    get(
                        |Extension(tenant): Extension<AuthenticatedTenant>| async move {
                            tenant.0.to_string()
                        },
                    ),
                )
                .layer(from_fn_with_state(
                    Arc::new(TokenResolver::new(store)),
                    require_user_bearer,
                ))
        }

        async fn request_with_token(app: Router, token: Option<&str>) -> (StatusCode, String) {
            let mut request = Request::builder().uri("/whoami");
            if let Some(token) = token {
                request = request.header(AUTHORIZATION, format!("Bearer {token}"));
            }
            let response = app
                .oneshot(request.body(Body::empty()).unwrap())
                .await
                .unwrap();
            let status = response.status();
            let body = to_bytes(response.into_body(), 1024).await.unwrap();
            (status, String::from_utf8_lossy(&body).to_string())
        }

        #[tokio::test]
        async fn resolves_issued_tokens_to_their_tenant() {
            let store = store().await;
            let issuer = TokenIssuer::new(store.clone());
            let alice = issuer.issue("111").await.unwrap();
            let bob = issuer.issue("222").await.unwrap();

            let app = router_with(store);

            assert_eq!(
                request_with_token(app.clone(), Some(&alice)).await,
                (StatusCode::OK, "111".to_string())
            );
            assert_eq!(
                request_with_token(app, Some(&bob)).await,
                (StatusCode::OK, "222".to_string())
            );
        }

        #[tokio::test]
        async fn rejects_unknown_token() {
            let app = router_with(store().await);

            let (status, _) = request_with_token(app, Some("froid_unknown")).await;

            assert_eq!(status, StatusCode::UNAUTHORIZED);
        }

        #[tokio::test]
        async fn rejects_missing_header() {
            let app = router_with(store().await);

            let (status, _) = request_with_token(app, None).await;

            assert_eq!(status, StatusCode::UNAUTHORIZED);
        }

        #[tokio::test]
        async fn revoked_token_stops_working() {
            let store = store().await;
            let issuer = TokenIssuer::new(store.clone());
            let token = issuer.issue("111").await.unwrap();
            issuer.revoke("111").await.unwrap();

            let app = router_with(store);

            let (status, _) = request_with_token(app, Some(&token)).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED);
        }
    }
}
