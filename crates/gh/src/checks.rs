//! CI checks on a pull request's head commit.

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::client::GitHub;
use crate::error::{GhError, GhResult};
use crate::query::PrRef;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Pending,
    Success,
    Failure,
    Neutral,
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct Check {
    pub name: String,
    pub status: CheckStatus,
    pub url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct PrChecks {
    /// GitHub's rollup for the head commit, when any checks exist.
    pub conclusion: Option<CheckStatus>,
    pub checks: Vec<Check>,
    /// More checks exist than were fetched.
    pub truncated: bool,
}

pub async fn pull_request_checks(gh: &GitHub, pr: &PrRef) -> GhResult<PrChecks> {
    let query = "query($owner: String!, $repo: String!, $n: Int!) { repository(owner: $owner, name: $repo) { \
        pullRequest(number: $n) { commits(last: 1) { nodes { commit { statusCheckRollup { state \
        contexts(first: 100) { pageInfo { hasNextPage } nodes { __typename \
        ... on CheckRun { name status conclusion detailsUrl } \
        ... on StatusContext { context state targetUrl } } } } } } } } } }";
    let what = format!("{}/{}#{}", pr.owner, pr.repo, pr.number);
    let data: Data = gh
        .graphql(
            query,
            json!({ "owner": pr.owner, "repo": pr.repo, "n": pr.number }),
            &what,
        )
        .await?;
    let pr_node = data
        .repository
        .and_then(|r| r.pull_request)
        .ok_or_else(|| GhError::NotVisible {
            what,
            login: gh.login().into(),
        })?;
    Ok(map(pr_node))
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
    commits: Connection,
}

#[derive(Deserialize)]
struct Connection {
    nodes: Vec<Option<CommitNode>>,
}

#[derive(Deserialize)]
struct CommitNode {
    commit: Commit,
}

#[derive(Deserialize)]
struct Commit {
    #[serde(rename = "statusCheckRollup")]
    status_check_rollup: Option<Rollup>,
}

#[derive(Deserialize)]
struct Rollup {
    state: Option<String>,
    contexts: Contexts,
}

#[derive(Deserialize)]
struct Contexts {
    #[serde(rename = "pageInfo")]
    page_info: PageInfo,
    nodes: Vec<Option<RawNode>>,
}

#[derive(Deserialize)]
struct PageInfo {
    #[serde(rename = "hasNextPage")]
    has_next_page: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CheckRun {
    name: String,
    status: String,
    conclusion: Option<String>,
    details_url: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StatusContext {
    context: String,
    state: String,
    target_url: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawNode {
    Run(CheckRun),
    Status(StatusContext),
    Other(serde_json::Value),
}

fn map(pr: Pull) -> PrChecks {
    let Some(rollup) = pr
        .commits
        .nodes
        .into_iter()
        .flatten()
        .next()
        .and_then(|n| n.commit.status_check_rollup)
    else {
        return PrChecks {
            conclusion: None,
            checks: Vec::new(),
            truncated: false,
        };
    };
    let mut checks = Vec::new();
    for node in rollup.contexts.nodes.into_iter().flatten() {
        let check = match node {
            RawNode::Run(r) => Check {
                status: run_status(&r.status, r.conclusion.as_deref()),
                name: r.name,
                url: r.details_url,
            },
            RawNode::Status(s) => Check {
                status: context_status(&s.state),
                name: s.context,
                url: s.target_url,
            },
            RawNode::Other(v) => {
                let _ = v;
                continue;
            }
        };
        checks.push(check);
    }
    checks.sort_by(|a, b| {
        rank(a.status)
            .cmp(&rank(b.status))
            .then_with(|| a.name.cmp(&b.name))
    });
    PrChecks {
        conclusion: rollup.state.as_deref().map(context_status),
        truncated: rollup.contexts.page_info.has_next_page,
        checks,
    }
}

fn rank(s: CheckStatus) -> u8 {
    match s {
        CheckStatus::Failure => 0,
        CheckStatus::Pending => 1,
        CheckStatus::Success => 2,
        CheckStatus::Neutral => 3,
        CheckStatus::Skipped => 4,
    }
}

fn run_status(status: &str, conclusion: Option<&str>) -> CheckStatus {
    if !status.eq_ignore_ascii_case("COMPLETED") {
        return CheckStatus::Pending;
    }
    match conclusion.unwrap_or("").to_ascii_uppercase().as_str() {
        "SUCCESS" => CheckStatus::Success,
        "NEUTRAL" | "STALE" => CheckStatus::Neutral,
        "SKIPPED" => CheckStatus::Skipped,
        _ => CheckStatus::Failure,
    }
}

fn context_status(state: &str) -> CheckStatus {
    match state.to_ascii_uppercase().as_str() {
        "SUCCESS" => CheckStatus::Success,
        "PENDING" | "EXPECTED" => CheckStatus::Pending,
        _ => CheckStatus::Failure,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{
      "repository": { "pullRequest": { "commits": { "nodes": [{ "commit": { "statusCheckRollup": {
        "state": "FAILURE",
        "contexts": { "pageInfo": { "hasNextPage": false }, "nodes": [
          { "__typename": "CheckRun", "name": "ci", "status": "COMPLETED", "conclusion": "SUCCESS", "detailsUrl": "https://example.com/ci" },
          { "__typename": "CheckRun", "name": "lint", "status": "COMPLETED", "conclusion": "FAILURE", "detailsUrl": null },
          { "__typename": "CheckRun", "name": "build", "status": "IN_PROGRESS", "conclusion": null, "detailsUrl": null },
          { "__typename": "CheckRun", "name": "docs", "status": "COMPLETED", "conclusion": "SKIPPED", "detailsUrl": null },
          { "__typename": "StatusContext", "context": "coverage", "state": "PENDING", "targetUrl": "https://example.com/cov" },
          { "__typename": "CheckSuite", "status": "QUEUED" }
        ] }
      } } }] } } }
    }"#;

    #[test]
    fn maps_a_mixed_rollup() {
        let data: Data = serde_json::from_str(FIXTURE).unwrap();
        let checks = map(data.repository.unwrap().pull_request.unwrap());
        assert_eq!(checks.conclusion, Some(CheckStatus::Failure));
        assert!(!checks.truncated);
        let names: Vec<_> = checks
            .checks
            .iter()
            .map(|c| (c.name.as_str(), c.status))
            .collect();
        assert_eq!(
            names,
            vec![
                ("lint", CheckStatus::Failure),
                ("build", CheckStatus::Pending),
                ("coverage", CheckStatus::Pending),
                ("ci", CheckStatus::Success),
                ("docs", CheckStatus::Skipped),
            ]
        );
        assert_eq!(checks.checks[0].url, None);
        assert_eq!(
            checks.checks[3].url.as_deref(),
            Some("https://example.com/ci")
        );
    }

    #[test]
    fn no_checks_is_empty() {
        let data: Data = serde_json::from_str(r#"{"repository":{"pullRequest":{"commits":{"nodes":[{"commit":{"statusCheckRollup":null}}]}}}}"#).unwrap();
        let checks = map(data.repository.unwrap().pull_request.unwrap());
        assert_eq!(
            checks,
            PrChecks {
                conclusion: None,
                checks: vec![],
                truncated: false
            }
        );
    }
}
