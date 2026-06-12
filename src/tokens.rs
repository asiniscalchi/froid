//! Self-serve bearer tokens issued through the Telegram `/token` command.
//!
//! Telegram is the authenticated channel: when a message arrives, the chat id
//! is known with certainty, so the bot can safely mint HTTP credentials for
//! that identity. Only the SHA-256 hash of a token is persisted (in the
//! central database); the plaintext exists once, in the bot's reply.

use rand::RngCore;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;

const TOKEN_BYTES: usize = 32;
const TOKEN_PREFIX: &str = "froid_";

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Generate a fresh random bearer token (256 bits, hex-encoded).
pub fn generate_token() -> String {
    let mut bytes = [0u8; TOKEN_BYTES];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    format!("{TOKEN_PREFIX}{}", hex_encode(&bytes))
}

/// Hash a token for storage and lookup.
pub fn hash_token(token: &str) -> String {
    hex_encode(&Sha256::digest(token.as_bytes()))
}

/// Persistence for issued tokens, keyed by Telegram chat id. One token per
/// user: issuing again rotates (replaces) the previous one.
#[derive(Clone)]
pub struct UserTokenStore {
    pool: SqlitePool,
}

impl UserTokenStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Store the token hash for `chat_id`, replacing any previous token.
    pub async fn upsert(&self, chat_id: &str, token_hash: &str) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"
            INSERT INTO user_tokens (chat_id, token_hash)
            VALUES (?, ?)
            ON CONFLICT (chat_id) DO UPDATE SET
                token_hash = excluded.token_hash,
                updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
            "#,
        )
        .bind(chat_id)
        .bind(token_hash)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Delete the token for `chat_id`. Returns whether one existed.
    pub async fn revoke(&self, chat_id: &str) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM user_tokens WHERE chat_id = ?")
            .bind(chat_id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() != 0)
    }

    /// Resolve a token hash to the owning chat id.
    pub async fn find_chat_id_by_hash(
        &self,
        token_hash: &str,
    ) -> Result<Option<String>, sqlx::Error> {
        sqlx::query_scalar("SELECT chat_id FROM user_tokens WHERE token_hash = ?")
            .bind(token_hash)
            .fetch_optional(&self.pool)
            .await
    }
}

/// Issues and revokes tokens on behalf of the Telegram adapter.
#[derive(Clone)]
pub struct TokenIssuer {
    store: UserTokenStore,
}

impl TokenIssuer {
    pub fn new(store: UserTokenStore) -> Self {
        Self { store }
    }

    /// Mint a fresh token for `chat_id`, replacing any previous one, and
    /// return the plaintext (the only time it is available).
    pub async fn issue(&self, chat_id: &str) -> Result<String, sqlx::Error> {
        let token = generate_token();
        self.store.upsert(chat_id, &hash_token(&token)).await?;
        Ok(token)
    }

    /// Revoke the token for `chat_id`. Returns whether one existed.
    pub async fn revoke(&self, chat_id: &str) -> Result<bool, sqlx::Error> {
        self.store.revoke(chat_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn store() -> UserTokenStore {
        let pool = crate::database::test_pool().await;
        UserTokenStore::new(pool)
    }

    #[test]
    fn generated_tokens_are_long_random_and_prefixed() {
        let a = generate_token();
        let b = generate_token();

        assert!(a.starts_with(TOKEN_PREFIX));
        assert_eq!(a.len(), TOKEN_PREFIX.len() + TOKEN_BYTES * 2);
        assert_ne!(a, b);
    }

    #[test]
    fn hashing_is_deterministic_and_not_identity() {
        let token = "froid_abc";

        assert_eq!(hash_token(token), hash_token(token));
        assert_ne!(hash_token(token), token);
        assert_eq!(hash_token(token).len(), 64);
    }

    #[tokio::test]
    async fn issue_stores_hash_and_resolves_to_chat_id() {
        let store = store().await;
        let issuer = TokenIssuer::new(store.clone());

        let token = issuer.issue("111").await.unwrap();

        assert_eq!(
            store
                .find_chat_id_by_hash(&hash_token(&token))
                .await
                .unwrap(),
            Some("111".to_string())
        );
        // The plaintext itself is never stored.
        assert_eq!(store.find_chat_id_by_hash(&token).await.unwrap(), None);
    }

    #[tokio::test]
    async fn issuing_again_rotates_the_token() {
        let store = store().await;
        let issuer = TokenIssuer::new(store.clone());

        let first = issuer.issue("111").await.unwrap();
        let second = issuer.issue("111").await.unwrap();

        assert_ne!(first, second);
        assert_eq!(
            store
                .find_chat_id_by_hash(&hash_token(&first))
                .await
                .unwrap(),
            None,
            "rotated token must stop working"
        );
        assert_eq!(
            store
                .find_chat_id_by_hash(&hash_token(&second))
                .await
                .unwrap(),
            Some("111".to_string())
        );
    }

    #[tokio::test]
    async fn revoke_removes_the_token() {
        let store = store().await;
        let issuer = TokenIssuer::new(store.clone());
        let token = issuer.issue("111").await.unwrap();

        assert!(issuer.revoke("111").await.unwrap());
        assert!(
            !issuer.revoke("111").await.unwrap(),
            "second revoke is a no-op"
        );
        assert_eq!(
            store
                .find_chat_id_by_hash(&hash_token(&token))
                .await
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn tokens_of_different_users_are_independent() {
        let store = store().await;
        let issuer = TokenIssuer::new(store.clone());

        let alice = issuer.issue("111").await.unwrap();
        let bob = issuer.issue("222").await.unwrap();
        issuer.revoke("111").await.unwrap();

        assert_eq!(
            store
                .find_chat_id_by_hash(&hash_token(&alice))
                .await
                .unwrap(),
            None
        );
        assert_eq!(
            store.find_chat_id_by_hash(&hash_token(&bob)).await.unwrap(),
            Some("222".to_string())
        );
    }
}
