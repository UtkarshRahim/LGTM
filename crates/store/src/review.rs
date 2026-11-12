//! Local review state: draft comments and per-file viewed marks.

use rusqlite::{OptionalExtension, params};

use crate::db::{Db, DbError};

/// Where a draft goes: a new comment on the diff, or a reply to a thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DraftTarget {
    Line { path: String, line: u32, side: String, start_line: Option<u32>, start_side: Option<String> },
    Reply { thread_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftRow {
    pub id: i64,
    pub target: DraftTarget,
    pub body: String,
    pub label: Option<String>,
    pub head_sha: String,
    pub created_at: i64,
}

impl Db {
    pub fn drafts(&self, pr_id: i64) -> Result<Vec<DraftRow>, DbError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, thread_id, path, line, side, start_line, start_side, body, label, head_sha, created_at
             FROM drafts WHERE pr_id = ?1 ORDER BY id",
        )?;
        let rows = stmt.query_map([pr_id], |r| {
            let thread_id: Option<String> = r.get(1)?;
            let target = match thread_id {
                Some(thread_id) => DraftTarget::Reply { thread_id },
                None => DraftTarget::Line {
                    path: r.get(2)?,
                    line: r.get(3)?,
                    side: r.get(4)?,
                    start_line: r.get(5)?,
                    start_side: r.get(6)?,
                },
            };
            Ok(DraftRow { id: r.get(0)?, target, body: r.get(7)?, label: r.get(8)?, head_sha: r.get(9)?, created_at: r.get(10)? })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn add_draft(
        &self,
        pr_id: i64,
        target: &DraftTarget,
        body: &str,
        label: Option<&str>,
        head_sha: &str,
    ) -> Result<i64, DbError> {
        let (thread_id, path, line, side, start_line, start_side) = match target {
            DraftTarget::Reply { thread_id } => (Some(thread_id.as_str()), None, None, None, None, None),
            DraftTarget::Line { path, line, side, start_line, start_side } => {
                (None, Some(path.as_str()), Some(*line), Some(side.as_str()), *start_line, start_side.as_deref())
            }
        };
        Ok(self.conn().query_row(
            "INSERT INTO drafts (pr_id, thread_id, path, line, side, start_line, start_side, body, label, head_sha)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) RETURNING id",
            params![pr_id, thread_id, path, line, side, start_line, start_side, body, label, head_sha],
            |r| r.get(0),
        )?)
    }

    /// Returns false if the draft no longer exists.
    pub fn update_draft(&self, id: i64, body: &str, label: Option<&str>) -> Result<bool, DbError> {
        Ok(self.conn().execute("UPDATE drafts SET body = ?2, label = ?3 WHERE id = ?1", params![id, body, label])? > 0)
    }

    pub fn delete_drafts(&self, ids: &[i64]) -> Result<(), DbError> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        for id in ids {
            tx.execute("DELETE FROM drafts WHERE id = ?1", [id])?;
        }
        tx.commit()?;
        Ok(())
    }

    /// `(path, content_hash)` for every file marked viewed in this PR.
    pub fn viewed(&self, pr_id: i64) -> Result<Vec<(String, String)>, DbError> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT path, content_hash FROM viewed WHERE pr_id = ?1")?;
        let rows = stmt.query_map([pr_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Marks a file viewed at `content_hash`, or clears the mark with `None`.
    pub fn set_viewed(&self, pr_id: i64, path: &str, content_hash: Option<&str>) -> Result<(), DbError> {
        match content_hash {
            Some(h) => self.conn().execute(
                "INSERT INTO viewed (pr_id, path, content_hash) VALUES (?1, ?2, ?3)
                 ON CONFLICT (pr_id, path) DO UPDATE SET content_hash = ?3",
                params![pr_id, path, h],
            )?,
            None => self.conn().execute("DELETE FROM viewed WHERE pr_id = ?1 AND path = ?2", params![pr_id, path])?,
        };
        Ok(())
    }

    pub fn pr_id(&self, host: &str, owner: &str, repo: &str, number: u32) -> Result<Option<i64>, DbError> {
        Ok(self
            .conn()
            .query_row(
                "SELECT id FROM prs WHERE host = ?1 AND owner = ?2 AND repo = ?3 AND number = ?4",
                params![host, owner, repo, number],
                |r| r.get(0),
            )
            .optional()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PrRow;

    fn db_with_pr() -> (Db, i64) {
        let db = Db::open_in_memory().unwrap();
        let id = db
            .upsert_pr(&PrRow {
                host: "github.com",
                owner: "acme",
                repo: "api",
                number: 7,
                title: "t",
                state: "OPEN",
                author: None,
                head_sha: "h1",
                base_sha: "b1",
                detail_json: "{}",
            })
            .unwrap();
        (db, id)
    }

    #[test]
    fn drafts_round_trip() {
        let (db, pr) = db_with_pr();
        let line = DraftTarget::Line {
            path: "src/a.rs".into(),
            line: 12,
            side: "RIGHT".into(),
            start_line: Some(10),
            start_side: Some("RIGHT".into()),
        };
        let reply = DraftTarget::Reply { thread_id: "PRRT_1".into() };
        let a = db.add_draft(pr, &line, "rename", Some("nit"), "h1").unwrap();
        let b = db.add_draft(pr, &reply, "agreed", None, "h1").unwrap();

        let all = db.drafts(pr).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!((all[0].target.clone(), all[0].label.as_deref()), (line, Some("nit")));
        assert_eq!(all[1].target, reply);

        assert!(db.update_draft(a, "rename it", Some("suggestion")).unwrap());
        assert_eq!(db.drafts(pr).unwrap()[0].body, "rename it");
        db.delete_drafts(&[a, b]).unwrap();
        assert!(db.drafts(pr).unwrap().is_empty());
        assert!(!db.update_draft(a, "x", None).unwrap());
    }

    #[test]
    fn rejects_bad_labels() {
        let (db, pr) = db_with_pr();
        let reply = DraftTarget::Reply { thread_id: "PRRT_1".into() };
        assert!(db.add_draft(pr, &reply, "x", Some("shouting"), "h1").is_err());
    }

    #[test]
    fn viewed_marks() {
        let (db, pr) = db_with_pr();
        db.set_viewed(pr, "a.rs", Some("h1")).unwrap();
        db.set_viewed(pr, "a.rs", Some("h2")).unwrap();
        db.set_viewed(pr, "b.rs", Some("h3")).unwrap();
        db.set_viewed(pr, "b.rs", None).unwrap();
        assert_eq!(db.viewed(pr).unwrap(), vec![("a.rs".to_owned(), "h2".to_owned())]);
        assert_eq!(db.pr_id("github.com", "acme", "api", 7).unwrap(), Some(pr));
    }
}
