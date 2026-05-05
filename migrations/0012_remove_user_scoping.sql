PRAGMA foreign_keys = OFF;

ALTER TABLE journal_entries RENAME TO journal_entries_old;

CREATE TABLE journal_entries (
    id                     INTEGER PRIMARY KEY AUTOINCREMENT,
    source                 TEXT    NOT NULL,
    source_conversation_id TEXT    NOT NULL,
    source_message_id      TEXT    NOT NULL,
    raw_text               TEXT    NOT NULL,
    received_at            TEXT    NOT NULL,
    created_at             TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (source, source_conversation_id, source_message_id)
);

INSERT INTO journal_entries
    (id, source, source_conversation_id, source_message_id, raw_text, received_at, created_at)
SELECT id, source, source_conversation_id, source_message_id, raw_text, received_at, created_at
FROM journal_entries_old;

DROP TABLE journal_entries_old;

CREATE INDEX idx_journal_entries_received
    ON journal_entries (received_at DESC, id DESC);

ALTER TABLE journal_entry_embedding_metadata RENAME TO journal_entry_embedding_metadata_old;

CREATE TABLE journal_entry_embedding_metadata (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    journal_entry_id INTEGER NOT NULL REFERENCES journal_entries(id) ON DELETE CASCADE,
    embedding_model  TEXT    NOT NULL,
    embedding_dim    INTEGER NOT NULL,
    created_at       TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    status           TEXT    NOT NULL DEFAULT 'completed',
    error_message    TEXT,
    UNIQUE (journal_entry_id, embedding_model)
);

INSERT INTO journal_entry_embedding_metadata
    (id, journal_entry_id, embedding_model, embedding_dim, created_at, status, error_message)
SELECT id, journal_entry_id, embedding_model, embedding_dim, created_at, status, error_message
FROM journal_entry_embedding_metadata_old;

DROP TABLE journal_entry_embedding_metadata_old;

CREATE INDEX idx_journal_entry_embedding_metadata_model
    ON journal_entry_embedding_metadata (embedding_model, journal_entry_id);

ALTER TABLE journal_entry_extractions RENAME TO journal_entry_extractions_old;

CREATE TABLE journal_entry_extractions (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    journal_entry_id INTEGER NOT NULL REFERENCES journal_entries(id) ON DELETE CASCADE,
    extraction_json  TEXT,
    model            TEXT    NOT NULL,
    prompt_version   TEXT    NOT NULL,
    status           TEXT    NOT NULL,
    error_message    TEXT,
    created_at       TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at       TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (journal_entry_id),
    CHECK (status IN ('pending', 'completed', 'failed')),
    CHECK (
        (status = 'pending' AND extraction_json IS NULL AND error_message IS NULL)
        OR
        (status = 'completed' AND extraction_json IS NOT NULL AND error_message IS NULL)
        OR
        (status = 'failed' AND extraction_json IS NULL AND error_message IS NOT NULL)
    )
);

INSERT INTO journal_entry_extractions
    (id, journal_entry_id, extraction_json, model, prompt_version, status,
     error_message, created_at, updated_at)
SELECT id, journal_entry_id, extraction_json, model, prompt_version, status,
       error_message, created_at, updated_at
FROM journal_entry_extractions_old;

DROP TABLE journal_entry_extractions_old;

ALTER TABLE daily_reviews RENAME TO daily_reviews_old;

CREATE TABLE daily_reviews (
    id                     INTEGER PRIMARY KEY AUTOINCREMENT,
    review_date            TEXT    NOT NULL,
    review_text            TEXT,
    model                  TEXT    NOT NULL,
    prompt_version         TEXT    NOT NULL,
    status                 TEXT    NOT NULL,
    error_message          TEXT,
    created_at             TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at             TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    delivered_at           TEXT,
    delivery_error         TEXT,
    signals_status         TEXT,
    signals_error          TEXT,
    signals_model          TEXT,
    signals_prompt_version TEXT,
    signals_updated_at     TEXT,
    UNIQUE (review_date),
    CHECK (status IN ('completed', 'failed')),
    CHECK (
        (status = 'completed' AND review_text IS NOT NULL AND error_message IS NULL)
        OR
        (status = 'failed' AND review_text IS NULL AND error_message IS NOT NULL)
    )
);

INSERT OR IGNORE INTO daily_reviews
    (id, review_date, review_text, model, prompt_version, status, error_message,
     created_at, updated_at, delivered_at, delivery_error, signals_status,
     signals_error, signals_model, signals_prompt_version, signals_updated_at)
SELECT id, review_date, review_text, model, prompt_version, status, error_message,
       created_at, updated_at, delivered_at, delivery_error, signals_status,
       signals_error, signals_model, signals_prompt_version, signals_updated_at
FROM daily_reviews_old
ORDER BY review_date ASC, updated_at DESC, id DESC;

DROP TABLE daily_reviews_old;

CREATE INDEX idx_daily_reviews_signals_status ON daily_reviews(signals_status)
WHERE signals_status IS NULL OR signals_status = 'failed';

ALTER TABLE daily_review_embedding_metadata RENAME TO daily_review_embedding_metadata_old;

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

INSERT INTO daily_review_embedding_metadata
    (id, daily_review_id, embedding_model, embedding_dim, status, error_message, created_at)
SELECT m.id, m.daily_review_id, m.embedding_model, m.embedding_dim, m.status, m.error_message, m.created_at
FROM daily_review_embedding_metadata_old m
JOIN daily_reviews r ON r.id = m.daily_review_id;

DROP TABLE daily_review_embedding_metadata_old;

CREATE INDEX idx_daily_review_embedding_metadata_model
    ON daily_review_embedding_metadata (embedding_model, daily_review_id);

ALTER TABLE daily_review_signals RENAME TO daily_review_signals_old;

CREATE TABLE daily_review_signals (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    daily_review_id INTEGER NOT NULL REFERENCES daily_reviews(id) ON DELETE CASCADE,
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

INSERT INTO daily_review_signals
    (id, daily_review_id, review_date, signal_type, label, status, valence,
     strength, confidence, evidence, model, prompt_version, created_at, updated_at)
SELECT s.id, s.daily_review_id, s.review_date, s.signal_type, s.label, s.status, s.valence,
       s.strength, s.confidence, s.evidence, s.model, s.prompt_version, s.created_at, s.updated_at
FROM daily_review_signals_old s
JOIN daily_reviews r ON r.id = s.daily_review_id;

DROP TABLE daily_review_signals_old;

CREATE INDEX idx_daily_review_signals_daily_review_id
    ON daily_review_signals(daily_review_id);

CREATE INDEX idx_daily_review_signals_date
    ON daily_review_signals(review_date);

CREATE INDEX idx_daily_review_signals_type_label
    ON daily_review_signals(signal_type, label);

CREATE INDEX idx_daily_review_signals_date_type
    ON daily_review_signals(review_date, signal_type);

ALTER TABLE weekly_reviews RENAME TO weekly_reviews_old;

CREATE TABLE weekly_reviews (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
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
    inputs_snapshot TEXT,
    UNIQUE (week_start_date),
    CHECK (status IN ('completed', 'failed')),
    CHECK (
        (status = 'completed' AND review_text IS NOT NULL AND error_message IS NULL)
        OR
        (status = 'failed' AND review_text IS NULL AND error_message IS NOT NULL)
    )
);

INSERT OR IGNORE INTO weekly_reviews
    (id, week_start_date, review_text, model, prompt_version, status, error_message,
     delivered_at, delivery_error, created_at, updated_at, inputs_snapshot)
SELECT id, week_start_date, review_text, model, prompt_version, status, error_message,
       delivered_at, delivery_error, created_at, updated_at, inputs_snapshot
FROM weekly_reviews_old
ORDER BY week_start_date ASC, updated_at DESC, id DESC;

DROP TABLE weekly_reviews_old;

CREATE INDEX idx_weekly_reviews_week
    ON weekly_reviews (week_start_date DESC);

PRAGMA foreign_keys = ON;
