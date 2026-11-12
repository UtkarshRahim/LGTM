use std::collections::BTreeMap;

use prism_gh::pr::{self, ApiFile};
use prism_gh::{GitHub, PrDetail, PrRef, PrSummary, SearchQuery, query};
use prism_repo::diff::{self, FileDiff, FileStatus};
use prism_repo::MirrorRepo;
use prism_store::PrRow;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};
use tauri_specta::Event;

use crate::error::AppResult;
use crate::projects::project_for;
use crate::state::AppState;

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct PrHit {
    pub summary: PrSummary,
    pub project_id: Option<i32>,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SearchOutcome {
    /// The input named one PR; open it directly.
    Open { pr: PrRef },
    List { query: SearchQuery, hits: Vec<PrHit> },
}

#[derive(Debug, Clone, Copy, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum DiffSource {
    /// GitHub's per-file patches; call `pr_local_diff` for the local version.
    Api,
    Local,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct PrView {
    pub detail: PrDetail,
    pub files: Vec<FileDiff>,
    pub source: DiffSource,
    pub project_id: Option<i32>,
}

/// Emitted when a background refresh finds new commits on an open PR.
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, Event)]
pub struct PrUpdated {
    pub pr: PrRef,
    pub head_sha: String,
}

fn hit(state: &AppState, summary: PrSummary) -> AppResult<PrHit> {
    let p = &summary.pr;
    let project_id = project_for(state, &p.host, &p.owner, &p.repo)?.map(|r| r.id);
    Ok(PrHit { summary, project_id })
}

/// Project repos grouped by host.
fn repos_by_host(state: &AppState) -> AppResult<BTreeMap<String, Vec<(String, String)>>> {
    let mut map: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for p in state.db.projects()? {
        if let (Some(h), Some(o), Some(r)) = (p.host, p.owner, p.repo) {
            let repos = map.entry(h).or_default();
            if !repos.contains(&(o.clone(), r.clone())) {
                repos.push((o, r));
            }
        }
    }
    Ok(map)
}

#[tauri::command]
#[specta::specta]
pub async fn search(state: State<'_, AppState>, input: String) -> AppResult<SearchOutcome> {
    let q = query::parse(&input);
    let mut hits = Vec::new();
    match &q {
        SearchQuery::Pr { pr } => return Ok(SearchOutcome::Open { pr: pr.clone() }),
        SearchQuery::Empty => {}
        SearchQuery::Number { number } => {
            for (host, repos) in repos_by_host(&state)? {
                let Ok(gh) = state.client(&host).await else { continue };
                for s in pr::find_number(&gh, &repos, *number).await? {
                    hits.push(hit(&state, s)?);
                }
            }
        }
        SearchQuery::Text { text } => {
            let mut by_host = repos_by_host(&state)?;
            by_host.entry(query::DEFAULT_HOST.into()).or_default();
            for (host, repos) in by_host {
                let Ok(gh) = state.client(&host).await else { continue };
                for s in pr::search_text(&gh, text, &repos).await? {
                    hits.push(hit(&state, s)?);
                }
            }
        }
    }
    Ok(SearchOutcome::List { query: q, hits })
}

/// Open PRs involving the signed-in github.com account, most recently updated first.
#[tauri::command]
#[specta::specta]
pub async fn involving_me(state: State<'_, AppState>) -> AppResult<Vec<PrHit>> {
    let gh = state.client(query::DEFAULT_HOST).await?;
    pr::search_text(&gh, "is:open sort:updated-desc", &[])
        .await?
        .into_iter()
        .map(|s| hit(&state, s))
        .collect()
}

fn api_to_diff(f: ApiFile) -> FileDiff {
    let status = match f.status.as_str() {
        "added" => FileStatus::Added,
        "removed" => FileStatus::Deleted,
        "renamed" => FileStatus::Renamed,
        "copied" => FileStatus::Copied,
        _ => FileStatus::Modified,
    };
    let pure_rename = status == FileStatus::Renamed && f.additions + f.deletions == 0;
    FileDiff {
        hunks: f.patch.as_deref().map(diff::parse_patch).unwrap_or_default(),
        patch_missing: f.patch.is_none() && !pure_rename,
        path: f.filename,
        old_path: f.previous_filename,
        status,
        binary: false,
        additions: f.additions,
        deletions: f.deletions,
    }
}

fn cache(state: &AppState, d: &PrDetail) -> AppResult<i64> {
    let p = &d.summary.pr;
    let state_str = serde_json::to_value(d.summary.state)?;
    Ok(state.db.upsert_pr(&PrRow {
        host: &p.host,
        owner: &p.owner,
        repo: &p.repo,
        number: p.number,
        title: &d.summary.title,
        state: state_str.as_str().unwrap_or("OPEN"),
        author: d.summary.author.as_deref(),
        head_sha: &d.head_sha,
        base_sha: &d.base_sha,
        detail_json: &serde_json::to_string(d)?,
    })?)
}

#[tauri::command]
#[specta::specta]
pub async fn open_pr(app: AppHandle, state: State<'_, AppState>, pr: PrRef) -> AppResult<PrView> {
    let view = open(&state, &pr).await?;
    if let DiffSource::Local = view.source {
        spawn_refresh(app, state.client(&pr.host).await?, pr, view.detail.head_sha.clone());
    }
    Ok(view)
}

/// Serves a cached PR from the mirror when possible, otherwise GitHub's patches.
async fn open(state: &AppState, pr: &PrRef) -> AppResult<PrView> {
    let project_id = project_for(state, &pr.host, &pr.owner, &pr.repo)?.map(|r| r.id);
    let gh = state.client(&pr.host).await?;

    if let Some(c) = state.db.cached_pr(&pr.host, &pr.owner, &pr.repo, pr.number)? {
        if let Some(mb) = &c.merge_base {
            let token = state.token(&pr.host).await?;
            let repo = MirrorRepo { host: &pr.host, owner: &pr.owner, repo: &pr.repo, token: &token };
            if state.mirrors.has_commits(&repo, mb, &c.head_sha).await {
                let files = state.mirrors.diff(&repo, pr.number, mb, &c.head_sha).await?;
                let detail: PrDetail = serde_json::from_str(&c.detail_json)?;
                return Ok(PrView { detail, files, source: DiffSource::Local, project_id });
            }
        }
    }

    let (detail, first) = tokio::join!(pr::pull_request(&gh, pr), pr::pr_files_page(&gh, pr, 1));
    let detail = detail?;
    let mut api_files = first?;
    if detail.changed_files as usize > api_files.len() {
        api_files.extend(pr::pr_files_rest(&gh, pr, detail.changed_files).await?);
    }
    cache(state, &detail)?;
    Ok(PrView {
        files: api_files.into_iter().map(api_to_diff).collect(),
        detail,
        source: DiffSource::Api,
        project_id,
    })
}

/// Re-fetches PR metadata after serving it from cache; emits `PrUpdated`
/// if the head moved.
fn spawn_refresh(app: AppHandle, gh: GitHub, pr: PrRef, cached_head: String) {
    tauri::async_runtime::spawn(async move {
        let detail = match pr::pull_request(&gh, &pr).await {
            Ok(d) => d,
            Err(e) => return tracing::debug!(error = %e, "background PR refresh failed"),
        };
        let state = app.state::<AppState>();
        if let Err(e) = cache(&state, &detail) {
            return tracing::warn!(error = %e, "caching refreshed PR");
        }
        if detail.head_sha != cached_head {
            let _ = PrUpdated { pr, head_sha: detail.head_sha }.emit(&app);
        }
    });
}

/// The PR diff from the local mirror, fetching commits as needed.
#[tauri::command]
#[specta::specta]
pub async fn pr_local_diff(state: State<'_, AppState>, pr: PrRef) -> AppResult<Vec<FileDiff>> {
    local_diff(&state, &pr).await
}

async fn local_diff(state: &AppState, pr: &PrRef) -> AppResult<Vec<FileDiff>> {
    let gh = state.client(&pr.host).await?;
    let token = state.token(&pr.host).await?;
    let cached = match state.db.cached_pr(&pr.host, &pr.owner, &pr.repo, pr.number)? {
        Some(c) => c,
        None => {
            cache(&state, &pr::pull_request(&gh, &pr).await?)?;
            state.db.cached_pr(&pr.host, &pr.owner, &pr.repo, pr.number)?.expect("just cached")
        }
    };
    let merge_base = match cached.merge_base {
        Some(mb) => mb,
        None => {
            let detail: PrDetail = serde_json::from_str(&cached.detail_json)?;
            let mb = pr::merge_base(&gh, &pr, &detail.base_sha, &cached.head_sha).await?;
            state.db.set_merge_base(cached.id, &cached.head_sha, &mb)?;
            mb
        }
    };
    let repo = MirrorRepo { host: &pr.host, owner: &pr.owner, repo: &pr.repo, token: &token };
    Ok(state.mirrors.diff(&repo, pr.number, &merge_base, &cached.head_sha).await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_files_map_to_diffs() {
        let f = |status: &str, patch: Option<&str>, adds| ApiFile {
            filename: "b.rs".into(),
            previous_filename: None,
            status: status.into(),
            additions: adds,
            deletions: 0,
            patch: patch.map(Into::into),
        };
        let d = api_to_diff(f("added", Some("@@ -0,0 +1 @@\n+x"), 1));
        assert_eq!((d.status, d.hunks.len(), d.patch_missing), (FileStatus::Added, 1, false));
        assert!(api_to_diff(f("modified", None, 5000)).patch_missing);
        assert!(!api_to_diff(f("renamed", None, 0)).patch_missing);
    }

    /// Phase 1 gate, measured below the UI:
    /// `PRISM_LIVE_TESTS=1 cargo test -p prism gate -- --nocapture`
    #[tokio::test(flavor = "multi_thread")]
    async fn gate_cold_and_cached_open() {
        use std::sync::{Mutex, RwLock};
        use std::time::Instant;
        if std::env::var("PRISM_LIVE_TESTS").is_err() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("prism-gate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let state = AppState {
            db: prism_store::Db::open(&dir.join("prism.db")).unwrap(),
            db_path: dir.join("prism.db"),
            settings: RwLock::new(Default::default()),
            settings_path: dir.join("settings.toml"),
            mirrors: prism_repo::Mirrors::new(dir.join("mirrors")),
            worktrees_dir: dir.join("worktrees"),
            creds: tokio::sync::RwLock::new(None),
            watchers: Mutex::new(Default::default()),
        };
        state.credentials().await;

        for (owner, repo, number) in [("cli", "cli", 9000), ("tauri-apps", "tauri", 11000)] {
            let pr = PrRef { host: "github.com".into(), owner: owner.into(), repo: repo.into(), number };
            let t = Instant::now();
            let cold = open(&state, &pr).await.unwrap();
            let cold_ms = t.elapsed().as_millis();
            let t = Instant::now();
            let local = local_diff(&state, &pr).await.unwrap();
            let sync_ms = t.elapsed().as_millis();
            let t = Instant::now();
            let cached = open(&state, &pr).await.unwrap();
            let cached_ms = t.elapsed().as_millis();

            println!(
                "{owner}/{repo}#{number}: {} files | cold (GitHub patches) {cold_ms} ms | mirror sync {sync_ms} ms | cached (local) {cached_ms} ms",
                cold.files.len()
            );
            assert!(matches!(cold.source, DiffSource::Api) && matches!(cached.source, DiffSource::Local));
            let api_hunks: Vec<_> = cold.files.iter().filter(|f| !f.patch_missing).map(|f| (&f.path, &f.hunks)).collect();
            let local_hunks: Vec<_> = local
                .iter()
                .filter(|f| api_hunks.iter().any(|(p, _)| *p == &f.path))
                .map(|f| (&f.path, &f.hunks))
                .collect();
            assert_eq!(api_hunks, local_hunks, "GitHub patches and local diff disagree");
            assert!(cached_ms < 200, "cached open took {cached_ms} ms");
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}
