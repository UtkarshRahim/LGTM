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

async fn search_prs(gh: &GitHub, q: &str) -> GhResult<Vec<PrSummary>> {
    #[derive(Deserialize)]
    struct Data {
        search: Nodes,
    }
    #[derive(Deserialize)]
    struct Nodes {
        nodes: Vec<Option<RawPr>>,
    }
    let query = format!(
        "query($q: String!) {{ search(query: $q, type: ISSUE, first: 30) {{ nodes {{ ... on PullRequest {{ {PR_FIELDS} }} }} }} }}"
    );
    let data: Data = gh.graphql(&query, json!({ "q": q }), "search").await?;
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
    let (blocking, rereview) = tokio::join!(search_prs(gh, &blocking_q), search_rereview(gh, &rereview_q));
    Ok(NeedsYou { blocking: blocking?, rereview: rereview? })
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
            let threads = crate::threads::review_threads(&gh, &pr("cli", "cli", 9000)).await.unwrap();
            println!("{} threads on cli/cli#9000", threads.len());
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
