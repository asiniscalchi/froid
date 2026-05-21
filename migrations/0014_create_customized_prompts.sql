CREATE TABLE customized_prompts (
    prompt_key  TEXT PRIMARY KEY,
    content     TEXT NOT NULL,
    updated_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
