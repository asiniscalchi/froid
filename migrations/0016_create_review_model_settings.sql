-- Model overrides set through the Telegram /model command for the daily and
-- weekly review generators. Overrides live in the central/default database —
-- per-tenant databases receive the table through the shared migration set but
-- leave it empty. A missing row means "use the env-configured default model".
CREATE TABLE review_model_settings (
    review_kind TEXT PRIMARY KEY CHECK (review_kind IN ('daily', 'weekly')),
    model       TEXT NOT NULL,
    updated_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
