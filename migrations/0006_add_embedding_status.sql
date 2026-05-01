ALTER TABLE journal_entry_embedding_metadata
    ADD COLUMN status TEXT NOT NULL DEFAULT 'completed';

ALTER TABLE journal_entry_embedding_metadata
    ADD COLUMN error_message TEXT;
