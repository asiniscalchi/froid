CREATE TABLE daily_review_signal_jobs (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    daily_review_id INTEGER NOT NULL REFERENCES daily_reviews(id) ON DELETE CASCADE,
    status          TEXT    NOT NULL,
    error_message   TEXT,
    model           TEXT,
    prompt_version  TEXT,
    started_at      TEXT,
    created_at      TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at      TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    CHECK (status IN ('pending', 'completed', 'failed'))
);
