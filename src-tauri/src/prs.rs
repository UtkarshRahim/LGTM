use prism_gh::pr::{self, ApiFile};
use prism_gh::threads::{ReviewThread, review_threads};
use prism_gh::{GitHub, PrDetail, PrRef, PrSummary, SearchQuery, query};
use prism_repo::diff::{self, FileDiff, FileStatus};
use prism_repo::MirrorRepo;
use prism_store::PrRow;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};
use std::sync::Arc;
use tauri_specta::Event;

use crate::diffs::{DiffSource, FileHunks, FileMeta, PrDiff};
use crate::error::AppResult;
use crate::projects::{project_for, repos_by_host};
use crate::review::{DiffScope, since_key};
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

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct PrView {
    pub detail: PrDetail,
    /// In review order; fetch hunks with `pr_hunks`.
    pub files: Vec<FileMeta>,
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
    let project_id = project_for(state, &p.host, &p.owner, &p.repo)?.map(|(id, _)| id);
    Ok(PrHit { summary, project_id })
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

#[derive(Debug, Clone, Default, Serialize, specta::Type)]
pub struct NeedsYou {
    pub blocking: Vec<PrHit>,
    pub rereview: Vec<PrHit>,
}

/// Home's queue, across github.com and every host your projects live on.
#[tauri::command]
#[specta::specta]
pub async fn needs_you(state: State<'_, AppState>) -> AppResult<NeedsYou> {
    let mut hosts: Vec<String> = repos_by_host(&state)?.into_keys().collect();
    if !hosts.iter().any(|h| h == query::DEFAULT_HOST) {
        hosts.push(query::DEFAULT_HOST.into());
    }
    let mut out = NeedsYou::default();
    for host in hosts {
        let Ok(gh) = state.client(&host).await else { continue };
        let n = pr::needs_you(&gh).await?;
        let hits = |v: Vec<PrSummary>| v.into_iter().map(|s| hit(&state, s)).collect::<AppResult<Vec<_>>>();
        out.blocking.extend(hits(n.blocking)?);
        out.rereview.extend(hits(n.rereview)?);
    }
    Ok(out)
}

#[tauri::command]
#[specta::specta]
pub async fn pr_threads(state: State<'_, AppState>, pr: PrRef) -> AppResult<Vec<ReviewThread>> {
    let gh = state.client(&pr.host).await?;
    Ok(review_threads(&gh, &pr).await?)
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
    let project_id = project_for(state, &pr.host, &pr.owner, &pr.repo)?.map(|(id, _)| id);
    let gh = state.client(&pr.host).await?;

    if let Some(c) = state.db.cached_pr(&pr.host, &pr.owner, &pr.repo, pr.number)? {
        if let Some(diff) = state.diffs.get(&key(pr)).filter(|d| d.is_local_at(&c.head_sha)) {
            let detail: PrDetail = serde_json::from_str(&c.detail_json)?;
            return Ok(PrView { detail, files: diff.metas(), source: DiffSource::Local, project_id });
        }
        if let Some(mb) = &c.merge_base {
            let token = state.token(&pr.host).await?;
            let repo = MirrorRepo { host: &pr.host, owner: &pr.owner, repo: &pr.repo, token: &token };
            if state.mirrors.has_commits(&repo, mb, &c.head_sha).await {
                let files = state.mirrors.diff(&repo, pr.number, mb, &c.head_sha).await?;
                let detail: PrDetail = serde_json::from_str(&c.detail_json)?;
                let diff = state.diffs.insert(key(pr), PrDiff::new(files).local(&c.head_sha));
                return Ok(PrView { detail, files: diff.metas(), source: DiffSource::Local, project_id });
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
    let diff = state.diffs.insert(key(pr), PrDiff::new(api_files.into_iter().map(api_to_diff).collect()));
    Ok(PrView { files: diff.metas(), detail, source: DiffSource::Api, project_id })
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

pub(crate) fn key(pr: &PrRef) -> String {
    format!("{}/{}/{}#{}", pr.host, pr.owner, pr.repo, pr.number)
}

/// Replaces GitHub's patches with the diff from the local mirror, fetching
/// commits as needed. Returns the new file list.
#[tauri::command]
#[specta::specta]
pub async fn pr_sync_local(state: State<'_, AppState>, pr: PrRef) -> AppResult<Vec<FileMeta>> {
    Ok(local(&state, &pr).await?.metas())
}

/// The PR's diff from the mirror at the cached head, computing it if needed.
pub(crate) async fn local(state: &AppState, pr: &PrRef) -> AppResult<Arc<PrDiff>> {
    if let Some(c) = state.db.cached_pr(&pr.host, &pr.owner, &pr.repo, pr.number)? {
        if let Some(d) = state.diffs.get(&key(pr)).filter(|d| d.is_local_at(&c.head_sha)) {
            return Ok(d);
        }
    }
    let (head, files) = local_diff(state, pr).await?;
    Ok(state.diffs.insert(key(pr), PrDiff::new(files).local(&head)))
}

pub(crate) fn cached_detail(state: &AppState, pr: &PrRef) -> AppResult<PrDetail> {
    let c = state
        .db
        .cached_pr(&pr.host, &pr.owner, &pr.repo, pr.number)?
        .ok_or_else(|| crate::error::AppError::invalid("Open the pull request first"))?;
    Ok(serde_json::from_str(&c.detail_json)?)
}

/// Hunks (with syntax highlighting) for the given files of an opened PR.
#[tauri::command]
#[specta::specta]
pub async fn pr_hunks(
    state: State<'_, AppState>,
    pr: PrRef,
    scope: DiffScope,
    paths: Vec<String>,
) -> AppResult<Vec<FileHunks>> {
    let diff: Arc<PrDiff> = match (scope, state.diffs.get(&key(&pr))) {
        (DiffScope::Since, _) => match state.diffs.get(&since_key(&pr)) {
            Some(d) => d,
            None => crate::review::since(&state, &pr).await?.and_then(|_| state.diffs.get(&since_key(&pr))).ok_or_else(
                || crate::error::AppError::invalid("You haven't reviewed this pull request yet"),
            )?,
        },
        (DiffScope::All, Some(d)) => d,
        (DiffScope::All, None) => {
            open(&state, &pr).await?;
            state.diffs.get(&key(&pr)).expect("open caches the diff")
        }
    };
    Ok(tauri::async_runtime::spawn_blocking(move || diff.hunks(&paths)).await.expect("highlighting panicked"))
}

/// Returns the head commit the diff was taken at, and the diff.
async fn local_diff(state: &AppState, pr: &PrRef) -> AppResult<(String, Vec<FileDiff>)> {
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
    let files = state.mirrors.diff(&repo, pr.number, &merge_base, &cached.head_sha).await?;
    Ok((cached.head_sha, files))
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
            diffs: Default::default(),
            runs: Default::default(),
            chats: Default::default(),
        };
        state.credentials().await;

        for (owner, repo, number) in [("cli", "cli", 9000), ("tauri-apps", "tauri", 11000)] {
            let pr = PrRef { host: "github.com".into(), owner: owner.into(), repo: repo.into(), number };
            let t = Instant::now();
            let cold = open(&state, &pr).await.unwrap();
            let cold_ms = t.elapsed().as_millis();
            let paths: Vec<String> = cold.files.iter().filter(|f| !f.patch_missing).map(|f| f.path.clone()).collect();
            let api_hunks = state.diffs.get(&key(&pr)).unwrap().hunks(&paths);
            let t = Instant::now();
            let local = PrDiff::new(local_diff(&state, &pr).await.unwrap().1);
            let sync_ms = t.elapsed().as_millis();
            let local_hunks = local.hunks(&paths);
            let t = Instant::now();
            let cached = open(&state, &pr).await.unwrap();
            let cached_ms = t.elapsed().as_millis();

            println!(
                "{owner}/{repo}#{number}: {} files | cold (GitHub patches) {cold_ms} ms | mirror sync {sync_ms} ms | cached (local) {cached_ms} ms",
                cold.files.len()
            );
            assert!(matches!(cold.source, DiffSource::Api) && matches!(cached.source, DiffSource::Local));
            let strip = |h: Vec<FileHunks>| h.into_iter().map(|f| (f.path, f.hunks)).collect::<Vec<_>>();
            assert_eq!(strip(api_hunks), strip(local_hunks), "GitHub patches and local diff disagree");
            assert!(cached_ms < 200, "cached open took {cached_ms} ms");
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Writes what the UI would receive for a PR, for the browser harness:
    /// `PRISM_FIXTURE_PR=owner/repo#123 PRISM_FIXTURE_OUT=/tmp/pr.json cargo test -p prism fixture`
    #[tokio::test]
    async fn fixture() {
        use std::sync::{Mutex, RwLock};
        let (Ok(spec), Ok(out)) = (std::env::var("PRISM_FIXTURE_PR"), std::env::var("PRISM_FIXTURE_OUT")) else { return };
        let SearchQuery::Pr { pr } = query::parse(&spec) else { panic!("PRISM_FIXTURE_PR must name one PR") };
        let dir = std::env::temp_dir().join(format!("prism-fixture-{}", std::process::id()));
        let state = AppState {
            db: prism_store::Db::open(&dir.join("prism.db")).unwrap(),
            db_path: dir.join("prism.db"),
            settings: RwLock::new(Default::default()),
            settings_path: dir.join("settings.toml"),
            mirrors: prism_repo::Mirrors::new(dir.join("mirrors")),
            worktrees_dir: dir.join("worktrees"),
            creds: tokio::sync::RwLock::new(None),
            watchers: Mutex::new(Default::default()),
            diffs: Default::default(),
            runs: Default::default(),
            chats: Default::default(),
        };
        state.credentials().await;
        let mut view = open(&state, &pr).await.unwrap();
        let (head, files) = local_diff(&state, &pr).await.unwrap();
        let diff = state.diffs.insert(key(&pr), PrDiff::new(files).local(&head));
        view.files = diff.metas();
        view.source = DiffSource::Local;
        let paths: Vec<String> = view.files.iter().map(|f| f.path.clone()).collect();
        let gh = state.client(&pr.host).await.unwrap();
        let threads = review_threads(&gh, &pr).await.unwrap();
        let json = serde_json::json!({ "pr": pr, "view": view, "hunks": diff.hunks(&paths), "threads": threads });
        std::fs::write(&out, serde_json::to_vec(&json).unwrap()).unwrap();
        println!("wrote {out}: {} files, {} threads", paths.len(), threads.len());
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// `PRISM_GATE_PR=owner/repo#123 cargo test -p prism --release huge -- --nocapture`
    #[tokio::test]
    async fn gate_huge_pr() {
        use std::sync::{Mutex, RwLock};
        use std::time::Instant;
        let Ok(spec) = std::env::var("PRISM_GATE_PR") else { return };
        let SearchQuery::Pr { pr } = query::parse(&spec) else { panic!("PRISM_GATE_PR must name one PR") };
        let dir = std::env::temp_dir().join(format!("prism-huge-{}", std::process::id()));
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
            diffs: Default::default(),
            runs: Default::default(),
            chats: Default::default(),
        };
        state.credentials().await;

        let t = Instant::now();
        let cold = open(&state, &pr).await.unwrap();
        println!("{} files | cold open {} ms", cold.files.len(), t.elapsed().as_millis());

        let diff = state.diffs.get(&key(&pr)).unwrap();
        let paths: Vec<String> = cold.files.iter().map(|f| f.path.clone()).collect();
        let t = Instant::now();
        let mut slowest = 0;
        for batch in paths.chunks(40) {
            let b = Instant::now();
            diff.hunks(batch);
            slowest = slowest.max(b.elapsed().as_millis());
        }
        println!("all hunks + highlighting {} ms, slowest batch of 40 {slowest} ms", t.elapsed().as_millis());

        let t = Instant::now();
        let (_, local) = local_diff(&state, &pr).await.unwrap();
        println!("mirror sync {} ms ({} files)", t.elapsed().as_millis(), local.len());
        let t = Instant::now();
        let cached = open(&state, &pr).await.unwrap();
        println!("cached open, fresh app {} ms ({:?})", t.elapsed().as_millis(), cached.source);
        let t = Instant::now();
        open(&state, &pr).await.unwrap();
        println!("cached open, same session {} ms", t.elapsed().as_millis());
        if std::env::var("PRISM_KEEP_GATE_DIR").is_ok() {
            println!("kept {}", dir.display());
        } else {
            std::fs::remove_dir_all(dir).unwrap();
        }
    }
}
