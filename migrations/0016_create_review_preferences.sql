-- Per-user opt-out for daily/weekly review delivery. This table lives in
-- each tenant's isolated database (one row max, enforced by the CHECK
-- constraint) — the absence of a row means the default, enabled state.
CREATE TABLE review_preferences (
    id               INTEGER PRIMARY KEY CHECK (id = 1),
    reviews_enabled  INTEGER NOT NULL DEFAULT 1,
    updated_at       TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
