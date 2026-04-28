CREATE TABLE journal_entries (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id          TEXT    NOT NULL,
    source           TEXT    NOT NULL,
    source_conversation_id TEXT NOT NULL,
    source_message_id TEXT   NOT NULL,
    raw_text         TEXT    NOT NULL,
    received_at      TEXT    NOT NULL,
    created_at       TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (source, source_conversation_id, source_message_id)
);

CREATE INDEX idx_journal_entries_user_received
    ON journal_entries (user_id, received_at DESC, id DESC);
