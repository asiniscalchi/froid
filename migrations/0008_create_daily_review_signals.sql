CREATE TABLE daily_review_signals (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    daily_review_id INTEGER NOT NULL REFERENCES daily_reviews(id) ON DELETE CASCADE,
    user_id         TEXT    NOT NULL,
    review_date     TEXT    NOT NULL,
    signal_type     TEXT    NOT NULL,
    label           TEXT    NOT NULL,
    status          TEXT,
    valence         TEXT,
    strength        REAL    NOT NULL,
    confidence      REAL    NOT NULL,
    evidence        TEXT    NOT NULL,
    model           TEXT    NOT NULL,
    prompt_version  TEXT    NOT NULL,
    created_at      TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at      TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    CHECK (signal_type IN ('theme', 'emotion', 'behavior', 'need', 'tension', 'pattern', 'tomorrow_attention')),
    CHECK (status IS NULL OR status IN ('activated', 'unmet', 'fulfilled', 'unclear')),
    CHECK (valence IS NULL OR valence IN ('positive', 'negative', 'ambiguous', 'neutral', 'unclear')),
    CHECK (strength >= 0.0 AND strength <= 1.0),
    CHECK (confidence >= 0.0 AND confidence <= 1.0),
    CHECK (length(label) > 0),
    CHECK (length(evidence) > 0)
);

CREATE INDEX idx_daily_review_signals_daily_review_id
    ON daily_review_signals(daily_review_id);

CREATE INDEX idx_daily_review_signals_user_date
    ON daily_review_signals(user_id, review_date);

CREATE INDEX idx_daily_review_signals_user_type_label
    ON daily_review_signals(user_id, signal_type, label);

CREATE INDEX idx_daily_review_signals_user_date_type
    ON daily_review_signals(user_id, review_date, signal_type);
