//! Skill runs and the findings they produce.

use rusqlite::params;

use crate::db::{Db, DbError};
use crate::review::DraftTarget;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunRow {
    pub id: i64,
    pub skill: String,
    pub skill_name: String,
    pub agent: String,
    pub head_sha: String,
    /// running, done, failed or cancelled.
    pub status: String,
    pub error: Option<String>,
    pub started_at: i64,
    pub ended_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewFinding<'a> {
    pub path: &'a str,
    pub line: Option<u32>,
    pub end_line: Option<u32>,
    pub severity: &'a str,
    pub title: &'a str,
    pub body: &'a str,
    pub suggestion: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FindingRow {
    pub id: i64,
    pub run_id: i64,
    /// Head the run reviewed.
    pub head_sha: String,
    pub path: String,
    pub line: Option<u32>,
    pub end_line: Option<u32>,
    pub severity: String,
    pub title: String,
    pub body: String,
    pub suggestion: Option<String>,
    /// open, accepted or dismissed.
    pub status: String,
    pub dismiss_reason: Option<String>,
}

impl Db {
    pub fn start_run(&self, pr_id: i64, skill: &str, skill_name: &str, agent: &str, head_sha: &str) -> Result<i64, DbError> {
        Ok(self.conn().query_row(
            "INSERT INTO skill_runs (pr_id, skill, skill_name, agent, head_sha, status) VALUES (?1, ?2, ?3, ?4, ?5, 'running')
             RETURNING id",
            params![pr_id, skill, skill_name, agent, head_sha],
            |r| r.get(0),
        )?)
    }

    /// Records the outcome and, for successful runs, the findings, together.
    pub fn finish_run(&self, id: i64, status: &str, error: Option<&str>, findings: &[NewFinding<'_>]) -> Result<(), DbError> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        for f in findings {
            tx.execute(
                "INSERT INTO findings (run_id, path, line, end_line, severity, title, body, suggestion)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![id, f.path, f.line, f.end_line, f.severity, f.title, f.body, f.suggestion],
            )?;
        }
        tx.execute(
            "UPDATE skill_runs SET status = ?2, error = ?3, ended_at = unixepoch() WHERE id = ?1",
            params![id, status, error],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Runs left `running` by a previous launch can't finish any more.
    pub fn fail_unfinished_runs(&self) -> Result<usize, DbError> {
        Ok(self.conn().execute(
            "UPDATE skill_runs SET status = 'failed', error = 'Prism quit during the run', ended_at = unixepoch()
             WHERE status = 'running'",
            [],
        )?)
    }

    /// Newest first.
    pub fn runs(&self, pr_id: i64) -> Result<Vec<RunRow>, DbError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, skill, skill_name, agent, head_sha, status, error, started_at, ended_at
             FROM skill_runs WHERE pr_id = ?1 ORDER BY id DESC",
        )?;
        let rows = stmt.query_map([pr_id], |r| {
            Ok(RunRow {
                id: r.get(0)?,
                skill: r.get(1)?,
                skill_name: r.get(2)?,
                agent: r.get(3)?,
                head_sha: r.get(4)?,
                status: r.get(5)?,
                error: r.get(6)?,
                started_at: r.get(7)?,
                ended_at: r.get(8)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn findings(&self, pr_id: i64) -> Result<Vec<FindingRow>, DbError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT f.id, f.run_id, r.head_sha, f.path, f.line, f.end_line, f.severity, f.title, f.body, f.suggestion,
                    f.status, f.dismiss_reason
             FROM findings f JOIN skill_runs r ON r.id = f.run_id
             WHERE r.pr_id = ?1 ORDER BY f.run_id, f.id",
        )?;
        let rows = stmt.query_map([pr_id], |r| {
            Ok(FindingRow {
                id: r.get(0)?,
                run_id: r.get(1)?,
                head_sha: r.get(2)?,
                path: r.get(3)?,
                line: r.get(4)?,
                end_line: r.get(5)?,
                severity: r.get(6)?,
                title: r.get(7)?,
                body: r.get(8)?,
                suggestion: r.get(9)?,
                status: r.get(10)?,
                dismiss_reason: r.get(11)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Turns a finding into a draft and marks it accepted, atomically.
    pub fn accept_finding(
        &self,
        finding_id: i64,
        pr_id: i64,
        target: &DraftTarget,
        body: &str,
        label: Option<&str>,
        head_sha: &str,
    ) -> Result<i64, DbError> {
        let DraftTarget::Line { path, line, side, start_line, start_side } = target else {
            return Err(DbError::Invalid("findings become line comments".into()));
        };
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let id = tx.query_row(
            "INSERT INTO drafts (pr_id, path, line, side, start_line, start_side, body, label, source, finding_id, head_sha)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'finding', ?9, ?10) RETURNING id",
            params![pr_id, path, line, side, start_line, start_side, body, label, finding_id, head_sha],
            |r| r.get(0),
        )?;
        tx.execute("UPDATE findings SET status = 'accepted', dismiss_reason = NULL WHERE id = ?1", [finding_id])?;
        tx.commit()?;
        Ok(id)
    }

    /// `reason: None` reopens a dismissed finding.
    pub fn dismiss_finding(&self, id: i64, reason: Option<&str>) -> Result<(), DbError> {
        let status = if reason.is_some() { "dismissed" } else { "open" };
        self.conn().execute("UPDATE findings SET status = ?2, dismiss_reason = ?3 WHERE id = ?1", params![id, status, reason])?;
        Ok(())
    }

    /// Deletes a draft you discarded; a finding it came from becomes open again.
    pub fn discard_draft(&self, id: i64) -> Result<(), DbError> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE findings SET status = 'open' WHERE id = (SELECT finding_id FROM drafts WHERE id = ?1)",
            [id],
        )?;
        tx.execute("DELETE FROM drafts WHERE id = ?1", [id])?;
        tx.commit()?;
        Ok(())
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
    fn runs_findings_and_accepting() {
        let (db, pr) = db_with_pr();
        let run = db.start_run(pr, "personal:review", "review", "cursor", "h1").unwrap();
        assert_eq!(db.runs(pr).unwrap()[0].status, "running");
        let f = NewFinding {
            path: "a.rs",
            line: Some(3),
            end_line: Some(5),
            severity: "blocker",
            title: "Off by one",
            body: "b",
            suggestion: Some("x"),
        };
        db.finish_run(run, "done", None, &[f.clone(), NewFinding { severity: "nit", line: None, end_line: None, ..f }]).unwrap();
        let runs = db.runs(pr).unwrap();
        assert_eq!((runs[0].status.as_str(), runs[0].ended_at.is_some()), ("done", true));
        let found = db.findings(pr).unwrap();
        assert_eq!(found.len(), 2);
        assert_eq!((found[0].head_sha.as_str(), found[0].end_line, found[0].status.as_str()), ("h1", Some(5), "open"));

        let target = DraftTarget::Line { path: "a.rs".into(), line: 5, side: "RIGHT".into(), start_line: Some(3), start_side: Some("RIGHT".into()) };
        let draft = db.accept_finding(found[0].id, pr, &target, "Off by one", None, "h1").unwrap();
        assert_eq!(db.findings(pr).unwrap()[0].status, "accepted");
        assert_eq!(db.drafts(pr).unwrap()[0].target, target);
        db.discard_draft(draft).unwrap();
        assert_eq!(db.findings(pr).unwrap()[0].status, "open");
        assert!(db.drafts(pr).unwrap().is_empty());

        db.dismiss_finding(found[1].id, Some("not a bug")).unwrap();
        assert_eq!(db.findings(pr).unwrap()[1].dismiss_reason.as_deref(), Some("not a bug"));
        db.dismiss_finding(found[1].id, None).unwrap();
        assert_eq!(db.findings(pr).unwrap()[1].status, "open");
    }

    #[test]
    fn unfinished_runs_fail_on_launch() {
        let (db, pr) = db_with_pr();
        db.start_run(pr, "s", "s", "cursor", "h1").unwrap();
        assert_eq!(db.fail_unfinished_runs().unwrap(), 1);
        assert_eq!(db.runs(pr).unwrap()[0].error.as_deref(), Some("Prism quit during the run"));
    }
}
