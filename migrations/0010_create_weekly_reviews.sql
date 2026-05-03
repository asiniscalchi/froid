CREATE TABLE weekly_reviews (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id         TEXT    NOT NULL,
    week_start_date TEXT    NOT NULL,
    review_text     TEXT,
    model           TEXT    NOT NULL,
    prompt_version  TEXT    NOT NULL,
    status          TEXT    NOT NULL,
    error_message   TEXT,
    delivered_at    TEXT,
    delivery_error  TEXT,
    created_at      TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at      TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (user_id, week_start_date),
    CHECK (status IN ('completed', 'failed')),
    CHECK (
        (status = 'completed' AND review_text IS NOT NULL AND error_message IS NULL)
        OR
        (status = 'failed' AND review_text IS NULL AND error_message IS NOT NULL)
    )
);

CREATE INDEX idx_weekly_reviews_user_week
    ON weekly_reviews (user_id, week_start_date DESC);
