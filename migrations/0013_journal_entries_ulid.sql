PRAGMA foreign_keys = OFF;

CREATE TEMP TABLE _je_id_map (
    old_id INTEGER PRIMARY KEY,
    new_id TEXT    NOT NULL UNIQUE
);

INSERT INTO _je_id_map (old_id, new_id)
SELECT id, lower(hex(randomblob(16))) FROM journal_entries;

ALTER TABLE journal_entries RENAME TO journal_entries_old;

CREATE TABLE journal_entries (
    id                     TEXT    PRIMARY KEY,
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
SELECT m.new_id, j.source, j.source_conversation_id, j.source_message_id,
       j.raw_text, j.received_at, j.created_at
FROM journal_entries_old j
JOIN _je_id_map m ON m.old_id = j.id;

DROP TABLE journal_entries_old;

CREATE INDEX idx_journal_entries_received
    ON journal_entries (received_at DESC, id DESC);

ALTER TABLE journal_entry_embedding_metadata RENAME TO journal_entry_embedding_metadata_old;

CREATE TABLE journal_entry_embedding_metadata (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    journal_entry_id TEXT    NOT NULL REFERENCES journal_entries(id) ON DELETE CASCADE,
    embedding_model  TEXT    NOT NULL,
    embedding_dim    INTEGER NOT NULL,
    created_at       TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    status           TEXT    NOT NULL DEFAULT 'completed',
    error_message    TEXT,
    UNIQUE (journal_entry_id, embedding_model)
);

INSERT INTO journal_entry_embedding_metadata
    (id, journal_entry_id, embedding_model, embedding_dim, created_at, status, error_message)
SELECT em.id, m.new_id, em.embedding_model, em.embedding_dim, em.created_at, em.status, em.error_message
FROM journal_entry_embedding_metadata_old em
JOIN _je_id_map m ON m.old_id = em.journal_entry_id;

DROP TABLE journal_entry_embedding_metadata_old;

CREATE INDEX idx_journal_entry_embedding_metadata_model
    ON journal_entry_embedding_metadata (embedding_model, journal_entry_id);

ALTER TABLE journal_entry_extractions RENAME TO journal_entry_extractions_old;

CREATE TABLE journal_entry_extractions (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    journal_entry_id TEXT    NOT NULL REFERENCES journal_entries(id) ON DELETE CASCADE,
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
SELECT ex.id, m.new_id, ex.extraction_json, ex.model, ex.prompt_version, ex.status,
       ex.error_message, ex.created_at, ex.updated_at
FROM journal_entry_extractions_old ex
JOIN _je_id_map m ON m.old_id = ex.journal_entry_id;

DROP TABLE journal_entry_extractions_old;

DROP TABLE _je_id_map;

PRAGMA foreign_keys = ON;
