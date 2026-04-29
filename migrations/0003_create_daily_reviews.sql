CREATE TABLE daily_reviews (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id        TEXT    NOT NULL,
    review_date    TEXT    NOT NULL,
    review_text    TEXT,
    model          TEXT    NOT NULL,
    prompt_version TEXT    NOT NULL,
    status         TEXT    NOT NULL,
    error_message  TEXT,
    created_at     TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at     TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (user_id, review_date),
    CHECK (status IN ('completed', 'failed')),
    CHECK (
        (status = 'completed' AND review_text IS NOT NULL AND error_message IS NULL)
        OR
        (status = 'failed' AND review_text IS NULL AND error_message IS NOT NULL)
    )
);

CREATE INDEX idx_daily_reviews_user_date
    ON daily_reviews (user_id, review_date);
