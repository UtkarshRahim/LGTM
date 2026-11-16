use futures_util::future::join_all;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::client::GitHub;
use crate::error::{GhError, GhResult};
use crate::query::PrRef;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PrState {
    Open,
    Closed,
    Merged,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct PrSummary {
    pub pr: PrRef,
    pub title: String,
    pub state: PrState,
    pub is_draft: bool,
    pub author: Option<String>,
    pub updated_at: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct PrDetail {
    pub summary: PrSummary,
    pub body: String,
    pub head_sha: String,
    pub base_sha: String,
    pub head_ref: String,
    pub base_ref: String,
    /// `owner/repo` the head branch lives in; `None` if the fork was deleted.
    pub head_repo: Option<String>,
    pub additions: u32,
    pub deletions: u32,
    pub changed_files: u32,
}

const PR_FIELDS: &str = "number title state isDraft url updatedAt author { login } repository { name owner { login } }";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawPr {
    number: u32,
    title: String,
    state: PrState,
    is_draft: bool,
    url: String,
    updated_at: String,
    author: Option<Login>,
    repository: RawRepo,
}

#[derive(Deserialize)]
struct Login {
    login: String,
}

#[derive(Deserialize)]
struct RawRepo {
    name: String,
    owner: Login,
}

impl RawPr {
    fn into_summary(self, host: &str) -> PrSummary {
        PrSummary {
            pr: PrRef { host: host.into(), owner: self.repository.owner.login, repo: self.repository.name, number: self.number },
            title: self.title,
            state: self.state,
            is_draft: self.is_draft,
            author: self.author.map(|a| a.login),
            updated_at: self.updated_at,
            url: self.url,
        }
    }
}

fn slug(pr: &PrRef) -> String {
    format!("{}/{}#{}", pr.owner, pr.repo, pr.number)
}

pub async fn pull_request(gh: &GitHub, pr: &PrRef) -> GhResult<PrDetail> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Detail {
        #[serde(flatten)]
        base: RawPr,
        body: String,
        head_ref_oid: String,
        base_ref_oid: String,
        head_ref_name: String,
        base_ref_name: String,
        head_repository: Option<NameWithOwner>,
        additions: u32,
        deletions: u32,
        changed_files: u32,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct NameWithOwner {
        name_with_owner: String,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Repo {
        pull_request: Option<Detail>,
    }
    #[derive(Deserialize)]
    struct Data {
        repository: Option<Repo>,
    }

    let query = format!(
        "query($owner: String!, $repo: String!, $n: Int!) {{ repository(owner: $owner, name: $repo) {{ \
           pullRequest(number: $n) {{ {PR_FIELDS} body headRefOid baseRefOid headRefName baseRefName \
             headRepository {{ nameWithOwner }} additions deletions changedFiles }} }} }}"
    );
    let what = slug(pr);
    let data: Data = gh
        .graphql(&query, json!({ "owner": pr.owner, "repo": pr.repo, "n": pr.number }), &what)
        .await?;
    let d = data
        .repository
        .and_then(|r| r.pull_request)
        .ok_or_else(|| GhError::NotVisible { what, login: gh.login().into() })?;
    Ok(PrDetail {
        summary: d.base.into_summary(gh.host()),
        body: d.body,
        head_sha: d.head_ref_oid,
        base_sha: d.base_ref_oid,
        head_ref: d.head_ref_name,
        base_ref: d.base_ref_name,
        head_repo: d.head_repository.map(|r| r.name_with_owner),
        additions: d.additions,
        deletions: d.deletions,
        changed_files: d.changed_files,
    })
}

/// The commit GitHub diffs a PR against: merge-base of base and head.
pub async fn merge_base(gh: &GitHub, pr: &PrRef, base_sha: &str, head_sha: &str) -> GhResult<String> {
    #[derive(Deserialize)]
    struct Compare {
        merge_base_commit: Sha,
    }
    #[derive(Deserialize)]
    struct Sha {
        sha: String,
    }
    let path = format!("/repos/{}/{}/compare/{base_sha}...{head_sha}?per_page=1", pr.owner, pr.repo);
    Ok(gh.rest_get::<Compare>(&path).await?.merge_base_commit.sha)
}

/// True if `old` is `new` or one of its ancestors, i.e. the branch wasn't rewritten.
pub async fn is_ancestor(gh: &GitHub, pr: &PrRef, old: &str, new: &str) -> GhResult<bool> {
    #[derive(Deserialize)]
    struct Compare {
        status: String,
    }
    let path = format!("/repos/{}/{}/compare/{old}...{new}?per_page=1", pr.owner, pr.repo);
    let status = gh.rest_get::<Compare>(&path).await?.status;
    Ok(status == "ahead" || status == "identical")
}

#[derive(Debug, Clone, Deserialize)]
pub struct ApiFile {
    pub filename: String,
    pub previous_filename: Option<String>,
    /// added, removed, modified, renamed, copied, changed, unchanged
    pub status: String,
    pub additions: u32,
    pub deletions: u32,
    /// Omitted by GitHub for binary and very large diffs.
    pub patch: Option<String>,
}

const FILES_PER_PAGE: u32 = 100;
/// GitHub stops listing after 3,000 files.
const MAX_FILE_PAGES: u32 = 30;

/// One page of `pulls/N/files`. Fetch page 1 alongside the PR query, then
/// the rest with [`pr_files_rest`] once the file count is known.
pub async fn pr_files_page(gh: &GitHub, pr: &PrRef, page: u32) -> GhResult<Vec<ApiFile>> {
    let path = format!("/repos/{}/{}/pulls/{}/files?per_page={FILES_PER_PAGE}&page={page}", pr.owner, pr.repo, pr.number);
    gh.rest_get(&path).await
}

pub async fn pr_files_rest(gh: &GitHub, pr: &PrRef, changed_files: u32) -> GhResult<Vec<ApiFile>> {
    let pages = changed_files.div_ceil(FILES_PER_PAGE).min(MAX_FILE_PAGES);
    let results = join_all((2..=pages).map(|p| pr_files_page(gh, pr, p))).await;
    let mut out = Vec::new();
    for r in results {
        out.extend(r?);
    }
    Ok(out)
}

/// Looks up `#number` in every repo with one aliased query. Repos where it
/// doesn't exist or isn't visible are skipped.
pub async fn find_number(gh: &GitHub, repos: &[(String, String)], number: u32) -> GhResult<Vec<PrSummary>> {
    if repos.is_empty() {
        return Ok(Vec::new());
    }
    let mut query = String::from("query($n: Int!) {");
    for (i, (owner, repo)) in repos.iter().enumerate() {
        query.push_str(&format!(
            " r{i}: repository(owner: {}, name: {}) {{ pullRequest(number: $n) {{ {PR_FIELDS} }} }}",
            json!(owner),
            json!(repo)
        ));
    }
    query.push_str(" }");

    let partial = gh
        .graphql_partial::<serde_json::Map<String, Value>>(&query, json!({ "n": number }), &format!("#{number}"))
        .await?;
    let mut found: Vec<(usize, PrSummary)> = partial
        .data
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(alias, repo)| {
            let idx: usize = alias.strip_prefix('r')?.parse().ok()?;
            let pr = serde_json::from_value::<RawPr>(repo.get("pullRequest")?.clone()).ok()?;
            Some((idx, pr.into_summary(gh.host())))
        })
        .collect();
    found.sort_by_key(|(i, _)| *i);
    Ok(found.into_iter().map(|(_, pr)| pr).collect())
}

/// Free-text search: PRs in the given repos plus any PR involving the viewer.
pub async fn search_text(gh: &GitHub, text: &str, repos: &[(String, String)]) -> GhResult<Vec<PrSummary>> {
    #[derive(Deserialize)]
    struct Data {
        search: Nodes,
    }
    #[derive(Deserialize)]
    struct Nodes {
        nodes: Vec<Option<RawPr>>,
    }

    let query = format!(
        "query($q: String!) {{ search(query: $q, type: ISSUE, first: 20) {{ nodes {{ ... on PullRequest {{ {PR_FIELDS} }} }} }} }}"
    );
    let text = text.replace('"', "");
    let mut searches = vec![format!("is:pr involves:@me {text}")];
    if !repos.is_empty() {
        let scope: Vec<String> = repos.iter().map(|(o, r)| format!("repo:{o}/{r}")).collect();
        searches.insert(0, format!("is:pr {} {text}", scope.join(" ")));
    }

    let results = join_all(searches.iter().map(|q| gh.graphql::<Data>(&query, json!({ "q": q }), "search"))).await;
    let mut out: Vec<PrSummary> = Vec::new();
    let mut first_err = None;
    for r in results {
        match r {
            Ok(data) => {
                for pr in data.search.nodes.into_iter().flatten() {
                    let s = pr.into_summary(gh.host());
                    if !out.iter().any(|o| o.pr == s.pr) {
                        out.push(s);
                    }
                }
            }
            Err(e) => first_err = first_err.or(Some(e)),
        }
    }
    match (out.is_empty(), first_err) {
        (true, Some(e)) => Err(e),
        _ => Ok(out),
    }
}

#[derive(Debug, Clone, Default, Serialize, specta::Type)]
pub struct NeedsYou {
    /// Your review (or your team's) is requested.
    pub blocking: Vec<PrSummary>,
    /// You reviewed, and the head moved since.
    pub rereview: Vec<PrSummary>,
}

const OPEN_READY: &str = "is:pr is:open -is:draft archived:false";

async fn search_prs(gh: &GitHub, q: &str, first: u32) -> GhResult<Vec<PrSummary>> {
    #[derive(Deserialize)]
    struct Data {
        search: Nodes,
    }
    #[derive(Deserialize)]
    struct Nodes {
        nodes: Vec<Option<RawPr>>,
    }
    let query = format!(
        "query($q: String!, $first: Int!) {{ search(query: $q, type: ISSUE, first: $first) {{ nodes {{ ... on PullRequest {{ {PR_FIELDS} }} }} }} }}"
    );
    let data: Data = gh.graphql(&query, json!({ "q": q, "first": first }), "search").await?;
    Ok(data.search.nodes.into_iter().flatten().map(|p| p.into_summary(gh.host())).collect())
}

async fn search_rereview(gh: &GitHub, q: &str) -> GhResult<Vec<PrSummary>> {
    #[derive(Deserialize)]
    struct Data {
        search: Nodes,
    }
    #[derive(Deserialize)]
    struct Nodes {
        nodes: Vec<Option<Reviewed>>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Reviewed {
        #[serde(flatten)]
        pr: RawPr,
        head_ref_oid: String,
        reviews: ReviewNodes,
    }
    #[derive(Deserialize)]
    struct ReviewNodes {
        nodes: Vec<Review>,
    }
    #[derive(Deserialize)]
    struct Review {
        commit: Option<Oid>,
    }
    #[derive(Deserialize)]
    struct Oid {
        oid: String,
    }
    let query = format!(
        "query($q: String!, $login: String!) {{ search(query: $q, type: ISSUE, first: 30) {{ nodes {{ ... on PullRequest {{ \
           {PR_FIELDS} headRefOid reviews(author: $login, last: 1) {{ nodes {{ commit {{ oid }} }} }} }} }} }} }}"
    );
    let data: Data = gh.graphql(&query, json!({ "q": q, "login": gh.login() }), "search").await?;
    Ok(data
        .search
        .nodes
        .into_iter()
        .flatten()
        .filter(|r| r.reviews.nodes.last().and_then(|rv| rv.commit.as_ref()).is_some_and(|c| c.oid != r.head_ref_oid))
        .map(|r| r.pr.into_summary(gh.host()))
        .collect())
}

pub async fn needs_you(gh: &GitHub) -> GhResult<NeedsYou> {
    let blocking_q = format!("{OPEN_READY} review-requested:@me sort:updated-desc");
    let rereview_q = format!("{OPEN_READY} reviewed-by:@me -review-requested:@me -author:@me sort:updated-desc");
    let (blocking, rereview) = tokio::join!(search_prs(gh, &blocking_q, 30), search_rereview(gh, &rereview_q));
    Ok(NeedsYou { blocking: blocking?, rereview: rereview? })
}

/// Open PRs (drafts included) across `repos`, most recently updated first.
pub async fn open_in_repos(gh: &GitHub, repos: &[(String, String)]) -> GhResult<Vec<PrSummary>> {
    if repos.is_empty() {
        return Ok(Vec::new());
    }
    let mut q = String::from("is:pr is:open archived:false sort:updated-desc");
    for (o, r) in repos {
        q.push_str(&format!(" repo:{o}/{r}"));
    }
    search_prs(gh, &q, 50).await
}

/// Open PR count per repo, in input order; `None` where the repo isn't visible.
pub async fn open_pr_counts(gh: &GitHub, repos: &[(String, String)]) -> GhResult<Vec<Option<u32>>> {
    if repos.is_empty() {
        return Ok(Vec::new());
    }
    let mut query = String::from("query {");
    for (i, (owner, repo)) in repos.iter().enumerate() {
        query.push_str(&format!(
            " r{i}: repository(owner: {}, name: {}) {{ pullRequests(states: OPEN) {{ totalCount }} }}",
            json!(owner),
            json!(repo)
        ));
    }
    query.push_str(" }");
    let data = gh
        .graphql_partial::<serde_json::Map<String, Value>>(&query, json!({}), "open PR counts")
        .await?
        .data
        .unwrap_or_default();
    Ok((0..repos.len())
        .map(|i| {
            data.get(&format!("r{i}"))?
                .pointer("/pullRequests/totalCount")?
                .as_u64()
                .map(|n| n as u32)
        })
        .collect())
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct PrComment {
    pub id: String,
    pub author: Option<String>,
    pub body_html: String,
    pub created_at: String,
    pub url: String,
    pub tag: Option<String>,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct PrCheck {
    pub name: String,
    pub status: String,
    pub url: Option<String>,
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct PrContext {
    pub body_html: String,
    pub comments: Vec<PrComment>,
    pub checks: Vec<PrCheck>,
    pub warning: Option<String>,
}

pub fn asset_url_ok(app_host: &str, raw: &str) -> bool {
    let Some(rest) = raw.strip_prefix("https://") else { return false };
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    if host.is_empty() || host.contains('@') {
        return false;
    }
    host == app_host || host == "github.com" || host == "www.github.com" || host.ends_with(".githubusercontent.com")
}

pub async fn pr_context(gh: &GitHub, pr: &PrRef) -> GhResult<PrContext> {
    let (body, checks, issues, reviews) = tokio::join!(body_html(gh, pr), checks(gh, pr), issue_comments(gh, pr), review_notes(gh, pr));
    let mut warning = Vec::new();
    let checks = unwrap_warn(checks, "checks", &mut warning);
    let mut comments = unwrap_warn(issues, "comments", &mut warning);
    comments.extend(unwrap_warn(reviews, "reviews", &mut warning));
    comments.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    Ok(PrContext { body_html: body?, comments, checks, warning: (!warning.is_empty()).then(|| warning.join(" ")) })
}

fn unwrap_warn<T>(r: GhResult<T>, what: &str, warning: &mut Vec<String>) -> T
where
    T: Default,
{
    match r {
        Ok(v) => v,
        Err(e) => {
            warning.push(format!("{what}: {e}"));
            T::default()
        }
    }
}

pub async fn add_pr_comment(gh: &GitHub, pr: &PrRef, body: &str) -> GhResult<PrComment> {
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
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Added {
        add_comment: Edge,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Edge {
        comment_edge: Option<Wrap>,
    }
    #[derive(Deserialize)]
    struct Wrap {
        node: RawNote,
    }
    let what = slug(pr);
    let q = "query($o: String!, $r: String!, $n: Int!) { repository(owner: $o, name: $r) { pullRequest(number: $n) { id } } }";
    let data: Data = gh.graphql(q, json!({ "o": pr.owner, "r": pr.repo, "n": pr.number }), &what).await?;
    let m = "mutation($id: ID!, $body: String!) { addComment(input: { subjectId: $id, body: $body }) { \
        commentEdge { node { id author { login } bodyHTML createdAt url } } } }";
    let added: Added = gh.mutate(m, json!({ "id": data.repository.pull_request.id, "body": body }), &what).await?;
    let node = added.add_comment.comment_edge.ok_or_else(|| GhError::api("GitHub posted no comment"))?.node;
    Ok(node.into_comment(None))
}

async fn body_html(gh: &GitHub, pr: &PrRef) -> GhResult<String> {
    #[derive(Deserialize)]
    struct Data {
        repository: Option<Repo>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Repo {
        pull_request: Option<Body>,
    }
    #[derive(Deserialize)]
    struct Body {
        #[serde(rename = "bodyHTML")]
        body_html: String,
    }
    let q = "query($o: String!, $r: String!, $n: Int!) { repository(owner: $o, name: $r) { pullRequest(number: $n) { bodyHTML } } }";
    let what = slug(pr);
    let data: Data = gh.graphql(q, json!({ "o": pr.owner, "r": pr.repo, "n": pr.number }), &what).await?;
    data.repository
        .and_then(|r| r.pull_request)
        .map(|b| b.body_html)
        .ok_or_else(|| GhError::NotVisible { what, login: gh.login().into() })
}

async fn checks(gh: &GitHub, pr: &PrRef) -> GhResult<Vec<PrCheck>> {
    #[derive(Deserialize)]
    struct Data {
        repository: Option<Repo>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Repo {
        pull_request: Option<Pr>,
    }
    #[derive(Deserialize)]
    struct Nodes<T> {
        nodes: Vec<T>,
    }
    #[derive(Deserialize)]
    struct Pr {
        commits: Nodes<CommitWrap>,
    }
    #[derive(Deserialize)]
    struct CommitWrap {
        commit: Commit,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Commit {
        status_check_rollup: Option<Rollup>,
    }
    #[derive(Deserialize)]
    struct Rollup {
        contexts: Contexts,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Contexts {
        page_info: Page,
        nodes: Vec<Option<RawContext>>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Page {
        has_next_page: bool,
        end_cursor: Option<String>,
    }
    #[derive(Deserialize)]
    #[serde(tag = "__typename")]
    enum RawContext {
        CheckRun {
            name: String,
            status: String,
            conclusion: Option<String>,
            #[serde(rename = "detailsUrl")]
            details_url: Option<String>,
            title: Option<String>,
            summary: Option<String>,
        },
        StatusContext {
            context: String,
            state: String,
            #[serde(rename = "targetUrl")]
            target_url: Option<String>,
            description: Option<String>,
        },
        #[serde(other)]
        Other,
    }
    let q = "query($o: String!, $r: String!, $n: Int!, $after: String) { repository(owner: $o, name: $r) { \
        pullRequest(number: $n) { commits(last: 1) { nodes { commit { statusCheckRollup { contexts(first: 100, after: $after) { \
        pageInfo { hasNextPage endCursor } nodes { __typename \
        ... on CheckRun { name status conclusion detailsUrl title summary } \
        ... on StatusContext { context state targetUrl description } } } } } } } } } }";
    let what = slug(pr);
    let mut out = Vec::new();
    let mut after: Option<String> = None;
    for _ in 0..5 {
        let data: Data = gh.graphql(q, json!({ "o": pr.owner, "r": pr.repo, "n": pr.number, "after": after }), &what).await?;
        let Some(contexts) = data
            .repository
            .and_then(|r| r.pull_request)
            .and_then(|p| p.commits.nodes.into_iter().next())
            .and_then(|c| c.commit.status_check_rollup)
            .map(|r| r.contexts)
        else {
            break;
        };
        for node in contexts.nodes.into_iter().flatten() {
            let mapped = match node {
                RawContext::CheckRun { name, status, conclusion, details_url, title, summary } => Some(PrCheck {
                    name,
                    status: conclusion.filter(|s| !s.is_empty()).unwrap_or(status),
                    url: details_url,
                    summary: check_summary(title, summary),
                }),
                RawContext::StatusContext { context, state, target_url, description } => {
                    Some(PrCheck { name: context, status: state, url: target_url, summary: nonempty(description) })
                }
                RawContext::Other => None,
            };
            if let Some(c) = mapped {
                out.push(c);
            }
        }
        match (contexts.page_info.has_next_page, contexts.page_info.end_cursor) {
            (true, Some(c)) => after = Some(c),
            _ => break,
        }
    }
    out.sort_by_key(|c| (check_rank(&c.status), c.name.clone()));
    Ok(out)
}

fn check_summary(title: Option<String>, summary: Option<String>) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(t) = nonempty(title) {
        parts.push(t);
    }
    if let Some(s) = nonempty(summary) {
        if parts.first() != Some(&s) {
            parts.push(s);
        }
    }
    if parts.is_empty() { None } else { Some(clip(&parts.join("\n"), 400)) }
}

fn nonempty(s: Option<String>) -> Option<String> {
    s.and_then(|s| {
        let t = s.trim();
        if t.is_empty() { None } else { Some(t.to_string()) }
    })
}

fn clip(s: &str, n: usize) -> String {
    let count = s.chars().count();
    if count <= n { s.to_string() } else { s.chars().take(n).collect() }
}

fn check_rank(status: &str) -> u8 {
    match status {
        "FAILURE" | "ERROR" | "TIMED_OUT" | "STARTUP_FAILURE" | "ACTION_REQUIRED" | "CANCELLED" | "STALE" => 0,
        "PENDING" | "QUEUED" | "IN_PROGRESS" | "WAITING" | "REQUESTED" | "EXPECTED" => 1,
        _ => 2,
    }
}

async fn issue_comments(gh: &GitHub, pr: &PrRef) -> GhResult<Vec<PrComment>> {
    #[derive(Deserialize)]
    struct Raw {
        id: u64,
        user: Option<Login>,
        body: Option<String>,
        body_html: Option<String>,
        created_at: String,
        html_url: String,
    }
    let mut out = Vec::new();
    for page in 1..=20 {
        let path = format!("/repos/{}/{}/issues/{}/comments?per_page=100&page={page}", pr.owner, pr.repo, pr.number);
        let batch: Vec<Raw> = gh.rest_get_full(&path).await?;
        let n = batch.len();
        out.extend(batch.into_iter().map(|c| PrComment {
            id: c.id.to_string(),
            author: c.user.map(|u| u.login),
            body_html: c.body_html.filter(|h| !html_blank(h)).unwrap_or_else(|| text_html(&c.body.unwrap_or_default())),
            created_at: c.created_at,
            url: c.html_url,
            tag: None,
        }));
        if n < 100 {
            break;
        }
    }
    Ok(out)
}

async fn review_notes(gh: &GitHub, pr: &PrRef) -> GhResult<Vec<PrComment>> {
    #[derive(Deserialize)]
    struct Data {
        repository: Option<Repo>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Repo {
        pull_request: Option<Pr>,
    }
    #[derive(Deserialize)]
    struct Pr {
        reviews: ReviewPage,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct ReviewPage {
        page_info: Page,
        nodes: Vec<Option<RawNote>>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Page {
        has_next_page: bool,
        end_cursor: Option<String>,
    }
    let q = "query($o: String!, $r: String!, $n: Int!, $after: String) { repository(owner: $o, name: $r) { \
        pullRequest(number: $n) { reviews(first: 100, after: $after) { pageInfo { hasNextPage endCursor } \
        nodes { id author { login } bodyHTML submittedAt url state } } } } }";
    let what = slug(pr);
    let mut out = Vec::new();
    let mut after: Option<String> = None;
    for _ in 0..20 {
        let data: Data = gh.graphql(q, json!({ "o": pr.owner, "r": pr.repo, "n": pr.number, "after": after }), &what).await?;
        let Some(page) = data.repository.and_then(|r| r.pull_request).map(|p| p.reviews) else { break };
        out.extend(page.nodes.into_iter().flatten().filter_map(|n| {
            if n.state == "PENDING" || html_blank(&n.body_html) {
                return None;
            }
            let tag = review_tag(&n.state);
            Some(n.into_comment(tag))
        }));
        match (page.page_info.has_next_page, page.page_info.end_cursor) {
            (true, Some(c)) => after = Some(c),
            _ => break,
        }
    }
    Ok(out)
}

#[derive(Deserialize)]
struct RawNote {
    id: String,
    author: Option<Login>,
    #[serde(rename = "bodyHTML")]
    body_html: String,
    #[serde(default)]
    #[serde(rename = "submittedAt")]
    submitted_at: Option<String>,
    #[serde(default)]
    #[serde(rename = "createdAt")]
    created_at: Option<String>,
    url: String,
    #[serde(default)]
    state: String,
}

impl RawNote {
    fn into_comment(self, tag: Option<String>) -> PrComment {
        PrComment {
            id: self.id,
            author: self.author.map(|a| a.login),
            body_html: self.body_html,
            created_at: self.submitted_at.or(self.created_at).unwrap_or_default(),
            url: self.url,
            tag,
        }
    }
}

fn review_tag(state: &str) -> Option<String> {
    Some(
        match state {
            "APPROVED" => "approved",
            "CHANGES_REQUESTED" => "changes requested",
            "COMMENTED" => "commented",
            "DISMISSED" => "dismissed",
            _ => return None,
        }
        .to_string(),
    )
}

fn html_blank(h: &str) -> bool {
    if h.contains("<img") || h.contains("<video") || h.contains("<source") {
        return false;
    }
    let mut text = String::new();
    let mut tag = false;
    for c in h.chars() {
        match c {
            '<' => tag = true,
            '>' => tag = false,
            _ if !tag => text.push(c),
            _ => {}
        }
    }
    text.trim().is_empty()
}

fn text_html(s: &str) -> String {
    let mut out = String::from("<p>");
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\n' => out.push_str("<br>"),
            _ => out.push(c),
        }
    }
    out.push_str("</p>");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comment_html_and_check_order() {
        assert!(html_blank("<p> </p>"));
        assert!(html_blank("<div><br></div>"));
        assert!(!html_blank("<p>hi</p>"));
        assert!(!html_blank("<p><img src=\"x\"></p>"));
        assert_eq!(text_html("a<b>&"), "<p>a&lt;b&gt;&amp;</p>");
        assert!(check_rank("FAILURE") < check_rank("PENDING"));
        assert!(check_rank("IN_PROGRESS") < check_rank("SUCCESS"));
        assert_eq!(check_summary(Some("t".into()), Some("s".into())).as_deref(), Some("t\ns"));
        assert_eq!(check_summary(Some(" same ".into()), Some("same".into())).as_deref(), Some("same"));
        assert!(asset_url_ok("github.com", "https://github.com/user-attachments/assets/abc"));
        assert!(asset_url_ok("github.com", "https://private-user-images.githubusercontent.com/1?jwt=a"));
        assert!(asset_url_ok("git.example", "https://git.example/uploads/a.png"));
        assert!(!asset_url_ok("github.com", "https://evil.com/a.png"));
        assert!(!asset_url_ok("github.com", "http://github.com/a.png"));
        assert!(!asset_url_ok("github.com", "https://github.com.evil/a"));
        assert!(!asset_url_ok("github.com", "https://github.com@evil.com/a"));
    }
}

#[cfg(test)]
mod live {
    //! Hits the real API with your `gh` login. Run with
    //! `PRISM_LIVE_TESTS=1 cargo test -p prism-gh live -- --nocapture`.
    use super::*;
    use crate::auth;

    /// The HTTP client is process-wide, so its pooled connections must
    /// outlive any single test: every live test shares one runtime.
    static RT: std::sync::LazyLock<tokio::runtime::Runtime> = std::sync::LazyLock::new(|| {
        tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap()
    });

    async fn client() -> Option<GitHub> {
        std::env::var("PRISM_LIVE_TESTS").ok()?;
        let creds = auth::discover().await;
        let c = auth::select(&creds, "github.com", None).expect("no github.com account");
        Some(GitHub::new("github.com", &c.token, &c.account.login))
    }

    fn pr(owner: &str, repo: &str, number: u32) -> PrRef {
        PrRef { host: "github.com".into(), owner: owner.into(), repo: repo.into(), number }
    }

    #[test]
    fn fetches_detail_and_merge_base() {
        RT.block_on(async {
            let Some(gh) = client().await else { return };
            let r = pr("cli", "cli", 9000);
            let t = std::time::Instant::now();
            let d = pull_request(&gh, &r).await.unwrap();
            let mb = merge_base(&gh, &r, &d.base_sha, &d.head_sha).await.unwrap();
            println!("{} files, merge base {mb}, {:?}", d.changed_files, t.elapsed());
            assert_eq!(d.summary.pr, r);
            assert_eq!(mb.len(), 40);
        })
    }

    #[test]
    fn missing_repo_is_not_visible() {
        RT.block_on(async {
            let Some(gh) = client().await else { return };
            let e = pull_request(&gh, &pr("UtkarshRahim", "definitely-not-a-repo-xyz", 1)).await.unwrap_err();
            assert!(matches!(e, GhError::NotVisible { .. }), "{e:?}");
        })
    }

    #[test]
    fn number_across_repos_skips_missing() {
        RT.block_on(async {
            let Some(gh) = client().await else { return };
            let repos = vec![("cli".into(), "cli".into()), ("nope-xyz-404".into(), "nope".into())];
            let found = find_number(&gh, &repos, 9000).await.unwrap();
            assert_eq!(found.len(), 1);
            let counts = open_pr_counts(&gh, &repos).await.unwrap();
            assert!(counts[0].is_some() && counts[1].is_none());
        })
    }

    #[test]
    fn needs_you_and_threads() {
        RT.block_on(async {
            let Some(gh) = client().await else { return };
            let n = needs_you(&gh).await.unwrap();
            println!("blocking {} rereview {}", n.blocking.len(), n.rereview.len());
            if let Some(hit) = n.rereview.first() {
                let last = crate::review::last_reviewed_commit(&gh, &hit.pr).await.unwrap();
                println!("last reviewed {:?} on {}", last, hit.url);
                assert!(last.is_some(), "re-review PRs have a reviewed commit");
            }
            let open = open_in_repos(&gh, &[("cli".into(), "cli".into()), ("cli".into(), "go-gh".into())]).await.unwrap();
            assert!(!open.is_empty());
            assert!(open.iter().all(|p| p.pr.owner == "cli" && p.state == PrState::Open));
            let threads = crate::threads::review_threads(&gh, &pr("cli", "cli", 9000)).await.unwrap();
            println!("{} threads on cli/cli#9000", threads.len());
            assert_eq!(crate::review::last_reviewed_commit(&gh, &pr("cli", "cli", 9000)).await.unwrap(), None);
        })
    }

    /// Posts real reviews. Point it at a throwaway PR you own:
    /// `PRISM_LIVE_TESTS=1 PRISM_REVIEW_PR=owner/repo#1 cargo test -p prism-gh posts_reviews -- --nocapture`
    #[test]
    fn posts_reviews() {
        use crate::review::{NewComment, Reply, ReviewEvent, last_reviewed_commit, set_thread_resolved, submit_review};
        use crate::threads::{DiffSide, review_threads};
        RT.block_on(async {
            let Ok(spec) = std::env::var("PRISM_REVIEW_PR") else { return };
            let Some(gh) = client().await else { return };
            let crate::SearchQuery::Pr { pr } = crate::query::parse(&spec) else { panic!("bad PRISM_REVIEW_PR") };
            let head = pull_request(&gh, &pr).await.unwrap().head_sha;
            let file = "REVIEW_TEST.md";
            let c = |line, start_line: Option<u32>, body: &str| NewComment {
                path: file.into(),
                line,
                side: DiffSide::Right,
                start_line,
                start_side: None,
                body: body.into(),
            };
            let old: std::collections::HashSet<String> =
                review_threads(&gh, &pr).await.unwrap().into_iter().map(|t| t.id).collect();
            let before = old.len();

            let url = submit_review(&gh, &pr, &head, ReviewEvent::Comment, "LGTM test: single mutation", &[
                c(3, None, "**nit:** single line"),
                c(6, Some(4), "range 4–6"),
            ], &[])
            .await
            .unwrap();
            println!("posted {url}");
            let threads = review_threads(&gh, &pr).await.unwrap();
            assert_eq!(threads.len(), before + 2);
            let threads: Vec<_> = threads.into_iter().filter(|t| !old.contains(&t.id)).collect();
            let range = threads.iter().find(|t| t.comments[0].body_html.contains("range")).unwrap();
            assert_eq!((range.start_line, range.line), (Some(4), Some(6)));
            let single = threads.iter().find(|t| t.comments[0].body_html.contains("single line")).unwrap().clone();
            assert!(single.can_reply && single.can_resolve);

            let url = submit_review(&gh, &pr, &head, ReviewEvent::Comment, "", &[c(5, None, "new comment with a reply")], &[
                Reply { thread_id: single.id.clone(), body: "queued reply".into() },
            ])
            .await
            .unwrap();
            println!("posted {url}");
            let threads = review_threads(&gh, &pr).await.unwrap();
            assert_eq!(threads.len(), before + 3);
            let single = threads.iter().find(|t| t.id == single.id).unwrap();
            assert_eq!(single.comments.len(), 2);
            assert!(single.comments[1].body_html.contains("queued reply"));

            set_thread_resolved(&gh, &pr, &single.id, true).await.unwrap();
            let resolved = review_threads(&gh, &pr).await.unwrap().into_iter().find(|t| t.id == single.id).unwrap();
            assert!(resolved.resolved);
            set_thread_resolved(&gh, &pr, &single.id, false).await.unwrap();
            assert_eq!(last_reviewed_commit(&gh, &pr).await.unwrap().as_deref(), Some(head.as_str()));

            let bad = Reply { thread_id: "PRRT_does_not_exist".into(), body: "x".into() };
            let err = submit_review(&gh, &pr, &head, ReviewEvent::Comment, "", &[c(4, None, "must not land")], &[bad]).await;
            assert!(err.is_err());
            let q = "query($o: String!, $r: String!, $n: Int!) { repository(owner: $o, name: $r) { pullRequest(number: $n) {
              reviews(states: [PENDING], first: 5) { totalCount } } } }";
            let v: serde_json::Value =
                gh.graphql(q, serde_json::json!({ "o": pr.owner, "r": pr.repo, "n": pr.number }), "pending").await.unwrap();
            assert_eq!(v["repository"]["pullRequest"]["reviews"]["totalCount"], 0, "failed submit left a pending review");
            // Threads of a just-deleted pending review linger briefly.
            let mut left = 0;
            for _ in 0..10 {
                left = review_threads(&gh, &pr).await.unwrap().len();
                if left == before + 3 {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
            assert_eq!(left, before + 3, "nothing from the failed submit was posted");
        })
    }

    #[test]
    fn text_search() {
        RT.block_on(async {
            let Some(gh) = client().await else { return };
            let found = search_text(&gh, "codespaces", &[("cli".into(), "cli".into())]).await.unwrap();
            assert!(!found.is_empty());
        })
    }
}
