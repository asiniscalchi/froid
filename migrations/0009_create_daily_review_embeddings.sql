CREATE TABLE daily_review_embedding_metadata (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    daily_review_id INTEGER NOT NULL REFERENCES daily_reviews(id) ON DELETE CASCADE,
    embedding_model TEXT    NOT NULL,
    embedding_dim   INTEGER NOT NULL,
    status          TEXT    NOT NULL DEFAULT 'completed',
    error_message   TEXT,
    created_at      TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (daily_review_id, embedding_model)
);

CREATE VIRTUAL TABLE daily_review_embedding_vec USING vec0(
    embedding float[1536]
);

CREATE INDEX idx_daily_review_embedding_metadata_model
    ON daily_review_embedding_metadata (embedding_model, daily_review_id);
