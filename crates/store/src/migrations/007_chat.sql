CREATE TABLE chat_messages (
    id         INTEGER PRIMARY KEY,
    pr_id      INTEGER NOT NULL REFERENCES prs (id) ON DELETE CASCADE,
    role       TEXT    NOT NULL CHECK (role IN ('user', 'assistant')),
    body       TEXT    NOT NULL,
    created_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE INDEX idx_chat_pr ON chat_messages (pr_id, id);
