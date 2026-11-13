//! Questions asked about a pull request, and the answers.

use rusqlite::params;

use crate::db::{Db, DbError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatRow {
    pub id: i64,
    pub role: String,
    pub body: String,
}

impl Db {
    pub fn add_chat(&self, pr_id: i64, role: &str, body: &str) -> Result<i64, DbError> {
        Ok(self.conn().query_row(
            "INSERT INTO chat_messages (pr_id, role, body) VALUES (?1, ?2, ?3) RETURNING id",
            params![pr_id, role, body],
            |r| r.get(0),
        )?)
    }

    /// Oldest first.
    pub fn chat_messages(&self, pr_id: i64) -> Result<Vec<ChatRow>, DbError> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT id, role, body FROM chat_messages WHERE pr_id = ?1 ORDER BY id")?;
        let rows = stmt.query_map([pr_id], |r| Ok(ChatRow { id: r.get(0)?, role: r.get(1)?, body: r.get(2)? }))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PrRow;

    #[test]
    fn messages_come_back_oldest_first() {
        let db = Db::open_in_memory().unwrap();
        let pr = db
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
        db.add_chat(pr, "user", "Why 404?").unwrap();
        db.add_chat(pr, "assistant", "Empty id.").unwrap();
        let rows = db.chat_messages(pr).unwrap();
        assert_eq!(
            rows.iter().map(|r| (r.role.as_str(), r.body.as_str())).collect::<Vec<_>>(),
            vec![("user", "Why 404?"), ("assistant", "Empty id.")]
        );
    }
}
