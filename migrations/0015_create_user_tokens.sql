-- Bearer tokens issued via the Telegram /token command. Only the SHA-256
-- hash is stored; the plaintext is shown once in the bot reply. This table
-- is only consulted in the central/default database — per-tenant databases
-- receive it through the shared migration set but leave it empty.
CREATE TABLE user_tokens (
    chat_id     TEXT PRIMARY KEY,
    token_hash  TEXT NOT NULL UNIQUE,
    created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
