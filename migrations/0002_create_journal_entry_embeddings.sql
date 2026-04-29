CREATE TABLE journal_entry_embedding_metadata (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    journal_entry_id INTEGER NOT NULL REFERENCES journal_entries(id) ON DELETE CASCADE,
    embedding_model  TEXT    NOT NULL,
    embedding_dim    INTEGER NOT NULL,
    created_at       TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (journal_entry_id, embedding_model)
);

CREATE VIRTUAL TABLE journal_entry_embedding_vec USING vec0(
    embedding float[1536]
);

CREATE INDEX idx_journal_entry_embedding_metadata_model
    ON journal_entry_embedding_metadata (embedding_model, journal_entry_id);
