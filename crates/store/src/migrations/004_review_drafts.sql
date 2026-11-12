-- Drafts now cover replies to existing threads and multi-line comments, and
-- remember the head they were written against. No drafts existed before.
DROP TABLE drafts;

CREATE TABLE drafts (
    id         INTEGER PRIMARY KEY,
    pr_id      INTEGER NOT NULL REFERENCES prs (id) ON DELETE CASCADE,
    -- Set for a reply to an existing GitHub review thread.
    thread_id  TEXT,
    path       TEXT,
    line       INTEGER,
    side       TEXT CHECK (side IN ('LEFT', 'RIGHT')),
    start_line INTEGER,
    start_side TEXT CHECK (start_side IN ('LEFT', 'RIGHT')),
    body       TEXT    NOT NULL,
    label      TEXT CHECK (label IN ('praise', 'nit', 'suggestion', 'question')),
    source     TEXT    NOT NULL DEFAULT 'you' CHECK (source IN ('you', 'finding')),
    finding_id INTEGER REFERENCES findings (id) ON DELETE SET NULL,
    head_sha   TEXT    NOT NULL,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    CHECK ((thread_id IS NULL) = (path IS NOT NULL AND line IS NOT NULL AND side IS NOT NULL))
);

CREATE INDEX idx_drafts_pr ON drafts (pr_id);
