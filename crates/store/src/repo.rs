use rusqlite::{OptionalExtension, params};

use crate::db::{Db, DbError};

#[derive(Debug, Clone, PartialEq)]
pub struct ProjectRow {
    pub id: i32,
    pub path: String,
    pub host: Option<String>,
    pub owner: Option<String>,
    pub repo: Option<String>,
    pub remote: Option<String>,
}

pub struct NewProject<'a> {
    pub path: &'a str,
    pub host: Option<&'a str>,
    pub owner: Option<&'a str>,
    pub repo: Option<&'a str>,
    pub remote: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteRow {
    pub remote: String,
    pub host: String,
    pub owner: String,
    pub repo: String,
}

pub struct PrRow<'a> {
    pub host: &'a str,
    pub owner: &'a str,
    pub repo: &'a str,
    pub number: u32,
    pub title: &'a str,
    pub state: &'a str,
    pub author: Option<&'a str>,
    pub head_sha: &'a str,
    pub base_sha: &'a str,
    pub detail_json: &'a str,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CachedPr {
    pub id: i64,
    pub head_sha: String,
    pub merge_base: Option<String>,
    pub detail_json: String,
}

impl Db {
    /// Adds a project, or refreshes its remote if the path is already known.
    pub fn add_project(&self, p: &NewProject<'_>) -> Result<i32, DbError> {
        Ok(self.conn().query_row(
            "INSERT INTO projects (path, host, owner, repo, default_remote) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (path) DO UPDATE SET host = ?2, owner = ?3, repo = ?4, default_remote = ?5
             RETURNING id",
            params![p.path, p.host, p.owner, p.repo, p.remote],
            |r| r.get(0),
        )?)
    }

    /// Replaces a project's remotes; the first is also stored as its primary.
    pub fn set_project_remotes(&self, project_id: i32, remotes: &[RemoteRow]) -> Result<(), DbError> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM project_remotes WHERE project_id = ?1", [project_id])?;
        for (i, r) in remotes.iter().enumerate() {
            tx.execute(
                "INSERT INTO project_remotes (project_id, remote, host, owner, repo, position) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![project_id, r.remote, r.host, r.owner, r.repo, i as i64],
            )?;
        }
        let first = remotes.first();
        tx.execute(
            "UPDATE projects SET default_remote = ?2, host = ?3, owner = ?4, repo = ?5 WHERE id = ?1",
            params![
                project_id,
                first.map(|r| &r.remote),
                first.map(|r| &r.host),
                first.map(|r| &r.owner),
                first.map(|r| &r.repo)
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// All remotes of all projects, in each project's preference order.
    pub fn project_remotes(&self) -> Result<Vec<(i32, RemoteRow)>, DbError> {
        let conn = self.conn();
        let mut stmt = conn.prepare_cached(
            "SELECT project_id, remote, host, owner, repo FROM project_remotes ORDER BY project_id, position",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get(0)?, RemoteRow { remote: r.get(1)?, host: r.get(2)?, owner: r.get(3)?, repo: r.get(4)? }))
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn remove_project(&self, id: i32) -> Result<(), DbError> {
        self.conn().execute("DELETE FROM projects WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn projects(&self) -> Result<Vec<ProjectRow>, DbError> {
        let conn = self.conn();
        let mut stmt =
            conn.prepare_cached("SELECT id, path, host, owner, repo, default_remote FROM projects ORDER BY added_at, id")?;
        let rows = stmt.query_map([], |r| {
            Ok(ProjectRow {
                id: r.get(0)?,
                path: r.get(1)?,
                host: r.get(2)?,
                owner: r.get(3)?,
                repo: r.get(4)?,
                remote: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Caches a freshly fetched PR and records its head as a changeset.
    /// The merge base is cleared when the head or base moves.
    pub fn upsert_pr(&self, p: &PrRow<'_>) -> Result<i64, DbError> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let id: i64 = tx.query_row(
            "INSERT INTO prs (host, owner, repo, number, title, state, author, head_sha, base_sha, detail_json, cached_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, unixepoch())
             ON CONFLICT (host, owner, repo, number) DO UPDATE SET
               title = ?5, state = ?6, author = ?7, detail_json = ?10, cached_at = unixepoch(),
               merge_base = CASE WHEN head_sha = ?8 AND base_sha = ?9 THEN merge_base END,
               head_sha = ?8, base_sha = ?9
             RETURNING id",
            params![p.host, p.owner, p.repo, p.number, p.title, p.state, p.author, p.head_sha, p.base_sha, p.detail_json],
            |r| r.get(0),
        )?;
        tx.execute(
            "INSERT OR IGNORE INTO changesets (pr_id, head_sha, base_sha) VALUES (?1, ?2, ?3)",
            params![id, p.head_sha, p.base_sha],
        )?;
        tx.commit()?;
        Ok(id)
    }

    pub fn set_merge_base(&self, pr_id: i64, head_sha: &str, merge_base: &str) -> Result<(), DbError> {
        self.conn().execute(
            "UPDATE prs SET merge_base = ?3 WHERE id = ?1 AND head_sha = ?2",
            params![pr_id, head_sha, merge_base],
        )?;
        Ok(())
    }

    pub fn cached_pr(&self, host: &str, owner: &str, repo: &str, number: u32) -> Result<Option<CachedPr>, DbError> {
        Ok(self
            .conn()
            .query_row(
                "SELECT id, head_sha, merge_base, detail_json FROM prs
                 WHERE host = ?1 AND owner = ?2 AND repo = ?3 AND number = ?4 AND detail_json IS NOT NULL",
                params![host, owner, repo, number],
                |r| Ok(CachedPr { id: r.get(0)?, head_sha: r.get(1)?, merge_base: r.get(2)?, detail_json: r.get(3)? }),
            )
            .optional()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr<'a>(head: &'a str) -> PrRow<'a> {
        PrRow {
            host: "github.com",
            owner: "acme",
            repo: "api",
            number: 7,
            title: "Fix",
            state: "OPEN",
            author: Some("me"),
            head_sha: head,
            base_sha: "base",
            detail_json: "{}",
        }
    }

    #[test]
    fn projects_upsert_by_path() {
        let db = Db::open_in_memory().unwrap();
        let p = |owner| NewProject { path: "/x", host: Some("github.com"), owner: Some(owner), repo: Some("r"), remote: Some("origin") };
        let a = db.add_project(&p("a")).unwrap();
        let b = db.add_project(&p("b")).unwrap();
        assert_eq!(a, b);
        assert_eq!(db.projects().unwrap()[0].owner.as_deref(), Some("b"));
        db.remove_project(a).unwrap();
        assert!(db.projects().unwrap().is_empty());
    }

    #[test]
    fn remotes_replace_and_cascade() {
        let db = Db::open_in_memory().unwrap();
        let id = db.add_project(&NewProject { path: "/z", host: None, owner: None, repo: None, remote: None }).unwrap();
        let r = |remote: &str, owner: &str| RemoteRow { remote: remote.into(), host: "github.com".into(), owner: owner.into(), repo: "zed".into() };
        db.set_project_remotes(id, &[r("upstream", "zed-industries"), r("origin", "me")]).unwrap();
        db.set_project_remotes(id, &[r("upstream", "zed-industries"), r("origin", "me"), r("alice", "alice")]).unwrap();
        let owners: Vec<_> = db.project_remotes().unwrap().into_iter().map(|(_, r)| r.owner).collect();
        assert_eq!(owners, ["zed-industries", "me", "alice"]);
        assert_eq!(db.projects().unwrap()[0].owner.as_deref(), Some("zed-industries"));
        db.remove_project(id).unwrap();
        assert!(db.project_remotes().unwrap().is_empty());
    }

    #[test]
    fn merge_base_resets_when_head_moves() {
        let db = Db::open_in_memory().unwrap();
        let id = db.upsert_pr(&pr("h1")).unwrap();
        db.set_merge_base(id, "h1", "mb1").unwrap();
        db.upsert_pr(&pr("h1")).unwrap();
        assert_eq!(db.cached_pr("github.com", "acme", "api", 7).unwrap().unwrap().merge_base.as_deref(), Some("mb1"));

        db.upsert_pr(&pr("h2")).unwrap();
        let c = db.cached_pr("github.com", "acme", "api", 7).unwrap().unwrap();
        assert_eq!((c.head_sha.as_str(), c.merge_base), ("h2", None));
        // A stale merge base for the old head is ignored.
        db.set_merge_base(id, "h1", "mb1").unwrap();
        assert_eq!(db.cached_pr("github.com", "acme", "api", 7).unwrap().unwrap().merge_base, None);

        let seen: i64 = db.conn().query_row("SELECT count(*) FROM changesets WHERE pr_id = ?1", [id], |r| r.get(0)).unwrap();
        assert_eq!(seen, 2);
    }
}
