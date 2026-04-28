CREATE TABLE journal_entries (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id          TEXT    NOT NULL,
    source           TEXT    NOT NULL,
    source_chat_id   TEXT    NOT NULL,
    source_message_id TEXT   NOT NULL,
    raw_text         TEXT    NOT NULL,
    received_at      TEXT    NOT NULL,
    created_at       TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (source, source_chat_id, source_message_id)
);
