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
