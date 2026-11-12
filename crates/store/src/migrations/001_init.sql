CREATE TABLE projects (
    id             INTEGER PRIMARY KEY,
    path           TEXT    NOT NULL UNIQUE,
    host           TEXT,
    owner          TEXT,
    repo           TEXT,
    default_remote TEXT,
    added_at       INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE TABLE prs (
    id        INTEGER PRIMARY KEY,
    host      TEXT    NOT NULL,
    owner     TEXT    NOT NULL,
    repo      TEXT    NOT NULL,
    number    INTEGER NOT NULL,
    title     TEXT    NOT NULL,
    state     TEXT    NOT NULL,
    author    TEXT,
    head_sha  TEXT,
    base_sha  TEXT,
    cached_at INTEGER NOT NULL DEFAULT (unixepoch()),
    UNIQUE (host, owner, repo, number)
);

CREATE TABLE changesets (
    id         INTEGER PRIMARY KEY,
    pr_id      INTEGER NOT NULL REFERENCES prs (id) ON DELETE CASCADE,
    head_sha   TEXT    NOT NULL,
    base_sha   TEXT    NOT NULL,
    seen_at    INTEGER NOT NULL DEFAULT (unixepoch()),
    force_push INTEGER NOT NULL DEFAULT 0,
    UNIQUE (pr_id, head_sha)
);

CREATE TABLE viewed (
    pr_id        INTEGER NOT NULL REFERENCES prs (id) ON DELETE CASCADE,
    path         TEXT    NOT NULL,
    content_hash TEXT    NOT NULL,
    PRIMARY KEY (pr_id, path)
);

CREATE TABLE drafts (
    id         INTEGER PRIMARY KEY,
    pr_id      INTEGER NOT NULL REFERENCES prs (id) ON DELETE CASCADE,
    path       TEXT    NOT NULL,
    line       INTEGER NOT NULL,
    side       TEXT    NOT NULL CHECK (side IN ('LEFT', 'RIGHT')),
    body       TEXT    NOT NULL,
    label      TEXT,
    source     TEXT    NOT NULL CHECK (source IN ('you', 'finding')),
    finding_id INTEGER REFERENCES findings (id) ON DELETE SET NULL,
    created_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE TABLE skill_runs (
    id         INTEGER PRIMARY KEY,
    pr_id      INTEGER NOT NULL REFERENCES prs (id) ON DELETE CASCADE,
    skill      TEXT    NOT NULL,
    agent      TEXT    NOT NULL,
    head_sha   TEXT    NOT NULL,
    status     TEXT    NOT NULL,
    started_at INTEGER NOT NULL DEFAULT (unixepoch()),
    ended_at   INTEGER
);

CREATE TABLE findings (
    id             INTEGER PRIMARY KEY,
    run_id         INTEGER NOT NULL REFERENCES skill_runs (id) ON DELETE CASCADE,
    path           TEXT    NOT NULL,
    line           INTEGER,
    severity       TEXT    NOT NULL CHECK (severity IN ('blocker', 'warning', 'nit')),
    title          TEXT    NOT NULL,
    body           TEXT    NOT NULL,
    suggestion     TEXT,
    status         TEXT    NOT NULL DEFAULT 'open' CHECK (status IN ('open', 'accepted', 'dismissed', 'stale')),
    dismiss_reason TEXT
);

CREATE TABLE agent_sessions (
    id             INTEGER PRIMARY KEY,
    pr_id          INTEGER NOT NULL REFERENCES prs (id) ON DELETE CASCADE,
    agent          TEXT    NOT NULL,
    acp_session_id TEXT,
    seed           TEXT,
    created_at     INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE INDEX idx_findings_run ON findings (run_id);
CREATE INDEX idx_drafts_pr ON drafts (pr_id);
CREATE INDEX idx_skill_runs_pr ON skill_runs (pr_id);
