//! Writing reviews: submit with inline comments and replies, resolve threads,
//! and find the commit you last reviewed.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::client::GitHub;
use crate::error::GhResult;
use crate::query::PrRef;
use crate::threads::DiffSide;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ReviewEvent {
    Comment,
    Approve,
    RequestChanges,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewComment {
    pub path: String,
    pub line: u32,
    pub side: DiffSide,
    pub start_line: Option<u32>,
    pub start_side: Option<DiffSide>,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    pub thread_id: String,
    pub body: String,
}

fn what(pr: &PrRef) -> String {
    format!("{}/{}#{}", pr.owner, pr.repo, pr.number)
}

async fn pr_node_id(gh: &GitHub, pr: &PrRef) -> GhResult<String> {
    #[derive(Deserialize)]
    struct Data {
        repository: Repo,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Repo {
        pull_request: Node,
    }
    #[derive(Deserialize)]
    struct Node {
        id: String,
    }
    let q = "query($o: String!, $r: String!, $n: Int!) { repository(owner: $o, name: $r) { pullRequest(number: $n) { id } } }";
    let d: Data = gh.graphql(q, json!({ "o": pr.owner, "r": pr.repo, "n": pr.number }), &what(pr)).await?;
    Ok(d.repository.pull_request.id)
}

fn thread_input(c: &NewComment) -> Value {
    let mut v = json!({ "path": c.path, "line": c.line, "side": c.side, "body": c.body });
    if let Some(start) = c.start_line.filter(|&s| s != c.line) {
        v["startLine"] = json!(start);
        v["startSide"] = json!(c.start_side.unwrap_or(c.side));
    }
    v
}

/// Submits a review and returns its URL. With no replies this is one
/// `addPullRequestReview` call; replies to existing threads need a pending
/// review, which is deleted again if any later step fails.
pub async fn submit_review(
    gh: &GitHub,
    pr: &PrRef,
    commit: &str,
    event: ReviewEvent,
    body: &str,
    comments: &[NewComment],
    replies: &[Reply],
) -> GhResult<String> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Added {
        add_pull_request_review: Payload,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Submitted {
        submit_pull_request_review: Payload,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Payload {
        pull_request_review: Review,
    }
    #[derive(Deserialize)]
    struct Review {
        id: String,
        url: String,
    }

    let what = what(pr);
    let pr_id = pr_node_id(gh, pr).await?;
    let threads: Vec<Value> = comments.iter().map(thread_input).collect();
    let add = "mutation($input: AddPullRequestReviewInput!) { addPullRequestReview(input: $input) { pullRequestReview { id url } } }";
    let mut input = json!({ "pullRequestId": pr_id, "commitOID": commit, "threads": threads });

    if replies.is_empty() {
        input["event"] = json!(event);
        input["body"] = json!(body);
        let added: Added = gh.mutate(add, json!({ "input": input }), &what).await?;
        return Ok(added.add_pull_request_review.pull_request_review.url);
    }

    let pending: Added = gh.mutate(add, json!({ "input": input }), &what).await?;
    let review_id = pending.add_pull_request_review.pull_request_review.id;
    let finish = async {
        let reply = "mutation($input: AddPullRequestReviewThreadReplyInput!) { addPullRequestReviewThreadReply(input: $input) { comment { id } } }";
        for r in replies {
            let input = json!({ "pullRequestReviewId": review_id, "pullRequestReviewThreadId": r.thread_id, "body": r.body });
            gh.mutate::<Value>(reply, json!({ "input": input }), &what).await?;
        }
        let submit = "mutation($input: SubmitPullRequestReviewInput!) { submitPullRequestReview(input: $input) { pullRequestReview { id url } } }";
        let input = json!({ "pullRequestReviewId": review_id, "event": event, "body": body });
        let done: Submitted = gh.mutate(submit, json!({ "input": input }), &what).await?;
        Ok(done.submit_pull_request_review.pull_request_review.url)
    };
    match finish.await {
        Ok(url) => Ok(url),
        Err(e) => {
            let delete = "mutation($id: ID!) { deletePullRequestReview(input: { pullRequestReviewId: $id }) { clientMutationId } }";
            if let Err(cleanup) = gh.mutate::<Value>(delete, json!({ "id": review_id }), &what).await {
                tracing::warn!(error = %cleanup, "couldn't delete pending review after a failed submit");
            }
            Err(e)
        }
    }
}

pub async fn set_thread_resolved(gh: &GitHub, pr: &PrRef, thread_id: &str, resolved: bool) -> GhResult<()> {
    let m = if resolved {
        "mutation($id: ID!) { resolveReviewThread(input: { threadId: $id }) { thread { id } } }"
    } else {
        "mutation($id: ID!) { unresolveReviewThread(input: { threadId: $id }) { thread { id } } }"
    };
    gh.mutate::<Value>(m, json!({ "id": thread_id }), &what(pr)).await?;
    Ok(())
}

/// The commit your most recent submitted review was made against.
pub async fn last_reviewed_commit(gh: &GitHub, pr: &PrRef) -> GhResult<Option<String>> {
    #[derive(Deserialize)]
    struct Data {
        repository: Repo,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Repo {
        pull_request: Pr,
    }
    #[derive(Deserialize)]
    struct Pr {
        reviews: Nodes,
    }
    #[derive(Deserialize)]
    struct Nodes {
        nodes: Vec<Rev>,
    }
    #[derive(Deserialize)]
    struct Rev {
        commit: Option<Oid>,
    }
    #[derive(Deserialize)]
    struct Oid {
        oid: String,
    }
    let q = "query($o: String!, $r: String!, $n: Int!, $login: String!) { repository(owner: $o, name: $r) { pullRequest(number: $n) {
      reviews(author: $login, states: [APPROVED, CHANGES_REQUESTED, COMMENTED, DISMISSED], last: 1) { nodes { commit { oid } } } } } }";
    let vars = json!({ "o": pr.owner, "r": pr.repo, "n": pr.number, "login": gh.login() });
    let d: Data = gh.graphql(q, vars, &what(pr)).await?;
    Ok(d.repository.pull_request.reviews.nodes.into_iter().last().and_then(|r| r.commit).map(|c| c.oid))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thread_inputs() {
        let single = NewComment {
            path: "a.rs".into(),
            line: 4,
            side: DiffSide::Right,
            start_line: None,
            start_side: None,
            body: "x".into(),
        };
        assert_eq!(thread_input(&single), json!({ "path": "a.rs", "line": 4, "side": "RIGHT", "body": "x" }));
        let range = NewComment { start_line: Some(2), ..single.clone() };
        assert_eq!(
            thread_input(&range),
            json!({ "path": "a.rs", "line": 4, "side": "RIGHT", "body": "x", "startLine": 2, "startSide": "RIGHT" })
        );
        let same = NewComment { start_line: Some(4), ..single };
        assert!(thread_input(&same).get("startLine").is_none());
    }

    #[test]
    fn events_serialize_for_graphql() {
        assert_eq!(json!(ReviewEvent::RequestChanges), json!("REQUEST_CHANGES"));
    }
}
