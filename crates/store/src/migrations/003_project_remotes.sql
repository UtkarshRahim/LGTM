-- Every forge repo a project's remotes point at (upstream, origin/fork, ...).
-- `projects.host/owner/repo/default_remote` stays the primary one.
CREATE TABLE project_remotes (
    project_id INTEGER NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    remote     TEXT    NOT NULL,
    host       TEXT    NOT NULL,
    owner      TEXT    NOT NULL,
    repo       TEXT    NOT NULL,
    position   INTEGER NOT NULL,
    PRIMARY KEY (project_id, remote)
);

INSERT INTO project_remotes (project_id, remote, host, owner, repo, position)
SELECT id, default_remote, host, owner, repo, 0 FROM projects
WHERE default_remote IS NOT NULL AND host IS NOT NULL AND owner IS NOT NULL AND repo IS NOT NULL;
