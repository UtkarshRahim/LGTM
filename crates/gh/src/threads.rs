use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::client::GitHub;
use crate::error::GhResult;
use crate::query::PrRef;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DiffSide {
    Left,
    Right,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct ReviewComment {
    pub id: String,
    pub author: Option<String>,
    /// Rendered and sanitized by GitHub.
    pub body_html: String,
    pub created_at: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct ReviewThread {
    pub id: String,
    pub path: String,
    /// Line in the current diff; `None` when the thread is outdated.
    pub line: Option<u32>,
    pub start_line: Option<u32>,
    pub original_line: Option<u32>,
    pub side: DiffSide,
    pub resolved: bool,
    pub outdated: bool,
    pub comments: Vec<ReviewComment>,
}

const QUERY: &str = "query($owner: String!, $repo: String!, $n: Int!, $after: String) {
  repository(owner: $owner, name: $repo) { pullRequest(number: $n) {
    reviewThreads(first: 100, after: $after) {
      pageInfo { hasNextPage endCursor }
      nodes { id path line startLine originalLine diffSide isResolved isOutdated
        comments(first: 100) { nodes { id author { login } bodyHTML createdAt url } } }
    } } } }";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawThread {
    id: String,
    path: String,
    line: Option<u32>,
    start_line: Option<u32>,
    original_line: Option<u32>,
    diff_side: DiffSide,
    is_resolved: bool,
    is_outdated: bool,
    comments: Nodes<RawComment>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawComment {
    id: String,
    author: Option<Login>,
    #[serde(rename = "bodyHTML")]
    body_html: String,
    created_at: String,
    url: String,
}

#[derive(Deserialize)]
struct Login {
    login: String,
}

#[derive(Deserialize)]
struct Nodes<T> {
    nodes: Vec<T>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageInfo {
    has_next_page: bool,
    end_cursor: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Threads {
    page_info: PageInfo,
    nodes: Vec<RawThread>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Pr {
    review_threads: Threads,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Repo {
    pull_request: Option<Pr>,
}

#[derive(Deserialize)]
struct Data {
    repository: Option<Repo>,
}

pub async fn review_threads(gh: &GitHub, pr: &PrRef) -> GhResult<Vec<ReviewThread>> {
    let what = format!("{}/{}#{}", pr.owner, pr.repo, pr.number);
    let mut out = Vec::new();
    let mut after: Option<String> = None;
    loop {
        let vars = json!({ "owner": pr.owner, "repo": pr.repo, "n": pr.number, "after": after });
        let data: Data = gh.graphql(QUERY, vars, &what).await?;
        let Some(threads) = data.repository.and_then(|r| r.pull_request).map(|p| p.review_threads) else {
            break;
        };
        out.extend(threads.nodes.into_iter().map(|t| ReviewThread {
            id: t.id,
            path: t.path,
            line: t.line,
            start_line: t.start_line,
            original_line: t.original_line,
            side: t.diff_side,
            resolved: t.is_resolved,
            outdated: t.is_outdated,
            comments: t
                .comments
                .nodes
                .into_iter()
                .map(|c| ReviewComment {
                    id: c.id,
                    author: c.author.map(|a| a.login),
                    body_html: c.body_html,
                    created_at: c.created_at,
                    url: c.url,
                })
                .collect(),
        }));
        match (threads.page_info.has_next_page, threads.page_info.end_cursor) {
            (true, Some(c)) => after = Some(c),
            _ => break,
        }
    }
    Ok(out)
}
