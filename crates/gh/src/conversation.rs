//! Pull request conversation: issue comments and submitted reviews, plus
//! posting a new comment on the pull request itself.

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::client::GitHub;
use crate::error::{GhError, GhResult};
use crate::query::PrRef;
use crate::review::pr_node_id;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub struct ConversationItem {
    pub id: String,
    pub author: Option<String>,
    pub body_html: String,
    pub created_at: String,
    pub url: String,
    /// Set for a submitted review: APPROVED, CHANGES_REQUESTED, COMMENTED, DISMISSED.
    pub review_state: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub struct Conversation {
    pub items: Vec<ConversationItem>,
    /// More comments or reviews exist than were fetched.
    pub truncated: bool,
}

pub async fn conversation(gh: &GitHub, pr: &PrRef) -> GhResult<Conversation> {
    let query = "query($o: String!, $r: String!, $n: Int!) { repository(owner: $o, name: $r) { \
        pullRequest(number: $n) { \
        comments(first: 100) { pageInfo { hasNextPage } nodes { id author { login } bodyHTML createdAt url } } \
        reviews(first: 100) { pageInfo { hasNextPage } nodes { id author { login } bodyHTML submittedAt url state } } } } }";
    let what = format!("{}/{}#{}", pr.owner, pr.repo, pr.number);
    let data: Data = gh
        .graphql(
            query,
            json!({ "o": pr.owner, "r": pr.repo, "n": pr.number }),
            &what,
        )
        .await?;
    let pull = data
        .repository
        .and_then(|r| r.pull_request)
        .ok_or_else(|| GhError::NotVisible {
            what,
            login: gh.login().into(),
        })?;
    Ok(assemble(pull))
}

pub async fn add_comment(gh: &GitHub, pr: &PrRef, body: &str) -> GhResult<ConversationItem> {
    let id = pr_node_id(gh, pr).await?;
    let query = "mutation($id: ID!, $body: String!) { addComment(input: { subjectId: $id, body: $body }) { \
        commentEdge { node { id author { login } bodyHTML createdAt url } } } }";
    let what = format!("{}/{}#{}", pr.owner, pr.repo, pr.number);
    let data: AddData = gh
        .mutate(query, json!({ "id": id, "body": body }), &what)
        .await?;
    let node = data
        .add_comment
        .and_then(|c| c.comment_edge)
        .and_then(|e| e.node)
        .ok_or_else(|| GhError::api("GitHub didn't return the comment"))?;
    Ok(item_from_comment(node))
}

fn assemble(pull: Pull) -> Conversation {
    let mut items = Vec::new();
    for node in pull.comments.nodes.into_iter().flatten() {
        items.push(item_from_comment(node));
    }
    for node in pull.reviews.nodes.into_iter().flatten() {
        if node.state == "PENDING" {
            continue;
        }
        let Some(created_at) = node.submitted_at else {
            continue;
        };
        items.push(ConversationItem {
            id: node.id,
            author: node.author.map(|a| a.login),
            body_html: node.body_html,
            created_at,
            url: node.url,
            review_state: Some(node.state),
        });
    }
    items.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
    Conversation {
        truncated: pull.comments.page_info.has_next_page || pull.reviews.page_info.has_next_page,
        items,
    }
}

fn item_from_comment(node: CommentNode) -> ConversationItem {
    ConversationItem {
        id: node.id,
        author: node.author.map(|a| a.login),
        body_html: node.body_html,
        created_at: node.created_at,
        url: node.url,
        review_state: None,
    }
}

#[derive(Deserialize)]
struct Data {
    repository: Option<Repo>,
}

#[derive(Deserialize)]
struct Repo {
    #[serde(rename = "pullRequest")]
    pull_request: Option<Pull>,
}

#[derive(Deserialize)]
struct Pull {
    comments: Connection<CommentNode>,
    reviews: Connection<ReviewNode>,
}

#[derive(Deserialize)]
struct Connection<T> {
    #[serde(rename = "pageInfo")]
    page_info: PageInfo,
    nodes: Vec<Option<T>>,
}

#[derive(Deserialize)]
struct PageInfo {
    #[serde(rename = "hasNextPage")]
    has_next_page: bool,
}

#[derive(Deserialize)]
struct Actor {
    login: String,
}

#[derive(Deserialize)]
struct CommentNode {
    id: String,
    author: Option<Actor>,
    #[serde(rename = "bodyHTML")]
    body_html: String,
    #[serde(rename = "createdAt")]
    created_at: String,
    url: String,
}

#[derive(Deserialize)]
struct ReviewNode {
    id: String,
    author: Option<Actor>,
    #[serde(rename = "bodyHTML")]
    body_html: String,
    #[serde(rename = "submittedAt")]
    submitted_at: Option<String>,
    url: String,
    state: String,
}

#[derive(Deserialize)]
struct AddData {
    #[serde(rename = "addComment")]
    add_comment: Option<AddPayload>,
}

#[derive(Deserialize)]
struct AddPayload {
    #[serde(rename = "commentEdge")]
    comment_edge: Option<CommentEdge>,
}

#[derive(Deserialize)]
struct CommentEdge {
    node: Option<CommentNode>,
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{
      "repository": { "pullRequest": {
        "comments": { "pageInfo": { "hasNextPage": false }, "nodes": [
          { "id": "c2", "author": { "login": "octocat" }, "bodyHTML": "<p>Later</p>", "createdAt": "2024-06-02T00:00:00Z", "url": "https://github.com/c2" },
          { "id": "c1", "author": null, "bodyHTML": "<p>First</p>", "createdAt": "2024-06-01T00:00:00Z", "url": "https://github.com/c1" }
        ] },
        "reviews": { "pageInfo": { "hasNextPage": true }, "nodes": [
          { "id": "r0", "author": { "login": "me" }, "bodyHTML": "", "submittedAt": null, "url": "https://github.com/r0", "state": "PENDING" },
          { "id": "r1", "author": { "login": "hubot" }, "bodyHTML": "", "submittedAt": "2024-06-01T12:00:00Z", "url": "https://github.com/r1", "state": "APPROVED" }
        ] }
      } }
    }"#;

    #[test]
    fn merges_comments_and_submitted_reviews_in_order() {
        let data: Data = serde_json::from_str(FIXTURE).unwrap();
        let conv = assemble(data.repository.unwrap().pull_request.unwrap());
        assert!(conv.truncated);
        let ids: Vec<_> = conv.items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, vec!["c1", "r1", "c2"]);
        assert_eq!(conv.items[1].review_state.as_deref(), Some("APPROVED"));
        assert!(conv.items[1].body_html.is_empty());
        assert_eq!(conv.items[0].author, None);
    }
}
