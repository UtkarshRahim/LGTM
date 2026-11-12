use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::time::Duration;

use futures_util::future::join_all;
use prism_gh::PrRef;
use prism_repo::checkout::{self, CheckoutMode, CheckoutOutcome, CheckoutRequest};
use prism_repo::{ProjectInfo, ProjectStatus, RemoteRepo, project, watch};
use prism_store::{DirtyCheckout, NewProject, ProjectRow, RemoteRow};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};
use tauri_specta::Event;

use crate::error::{AppError, AppResult};
use crate::prs::PrHit;
use crate::state::AppState;

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct Project {
    pub id: i32,
    pub path: String,
    pub name: String,
    /// Forge repos among the project's remotes; the first is the primary.
    pub remotes: Vec<RemoteRepo>,
    /// `None` if the folder is missing or no longer a git repo.
    pub status: Option<ProjectStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, Event)]
pub struct ProjectStatusChanged {
    pub id: i32,
    pub status: Option<ProjectStatus>,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct RepoCount {
    pub owner: String,
    pub repo: String,
    pub open: u32,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct PrCount {
    pub project_id: i32,
    /// Sum over the visible repos; `None` if none were visible.
    pub open: Option<u32>,
    pub by_repo: Vec<RepoCount>,
}

fn to_remote(r: RemoteRow) -> RemoteRepo {
    RemoteRepo { remote: r.remote, host: r.host, owner: r.owner, repo: r.repo }
}

fn to_row(r: &RemoteRepo) -> RemoteRow {
    RemoteRow { remote: r.remote.clone(), host: r.host.clone(), owner: r.owner.clone(), repo: r.repo.clone() }
}

pub fn remotes_by_project(state: &AppState) -> AppResult<HashMap<i32, Vec<RemoteRepo>>> {
    let mut map: HashMap<i32, Vec<RemoteRepo>> = HashMap::new();
    for (id, r) in state.db.project_remotes()? {
        map.entry(id).or_default().push(to_remote(r));
    }
    Ok(map)
}

async fn to_project(row: ProjectRow, remotes: Vec<RemoteRepo>) -> Project {
    let status = project::status(Path::new(&row.path)).await.ok();
    Project {
        id: row.id,
        name: Path::new(&row.path).file_name().map_or(row.path.clone(), |n| n.to_string_lossy().into_owned()),
        remotes,
        path: row.path,
        status,
    }
}

/// Watches a project's `HEAD` and index, emitting a debounced status event.
pub fn start_watching(app: &AppHandle, id: i32, git_dir: &Path, root: String) {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let watcher = match watch::watch(git_dir, tx) {
        Ok(w) => w,
        Err(e) => {
            tracing::warn!(id, error = %e, "can't watch project");
            return;
        }
    };
    app.state::<AppState>().watchers.lock().unwrap().insert(id, watcher);

    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        while rx.recv().await.is_some() {
            // Coalesce the burst of events from one git operation.
            tokio::time::sleep(Duration::from_millis(150)).await;
            while rx.try_recv().is_ok() {}
            let status = project::status(Path::new(&root)).await.ok();
            let _ = ProjectStatusChanged { id, status }.emit(&app);
        }
    });
}

/// At startup: refresh each project's remotes (they may have changed while
/// LGTM was closed) and start its watcher.
pub async fn watch_all(app: &AppHandle) {
    let state = app.state::<AppState>();
    let rows = match state.db.projects() {
        Ok(rows) => rows,
        Err(e) => return tracing::error!(error = %e, "loading projects"),
    };
    for row in rows {
        if let Ok(info) = project::inspect(Path::new(&row.path)).await {
            if let Err(e) = save_remotes(&state, row.id, &info) {
                tracing::warn!(id = row.id, error = %e, "refreshing project remotes");
            }
            start_watching(app, row.id, Path::new(&info.git_dir), info.root);
        }
    }
}

fn save_remotes(state: &AppState, id: i32, info: &ProjectInfo) -> AppResult<()> {
    Ok(state.db.set_project_remotes(id, &info.remotes.iter().map(to_row).collect::<Vec<_>>())?)
}

#[tauri::command]
#[specta::specta]
pub async fn list_projects(state: State<'_, AppState>) -> AppResult<Vec<Project>> {
    let rows = state.db.projects()?;
    let mut remotes = remotes_by_project(&state)?;
    Ok(join_all(rows.into_iter().map(|r| {
        let rs = remotes.remove(&r.id).unwrap_or_default();
        to_project(r, rs)
    }))
    .await)
}

#[tauri::command]
#[specta::specta]
pub async fn add_project(app: AppHandle, state: State<'_, AppState>, path: String) -> AppResult<Project> {
    let info = project::inspect(Path::new(&path)).await.map_err(|e| match e {
        prism_repo::GitError::NotARepo { .. } => AppError::invalid(format!("{path} is not a git repository")),
        e => e.into(),
    })?;
    let id = state.db.add_project(&NewProject { path: &info.root, host: None, owner: None, repo: None, remote: None })?;
    save_remotes(&state, id, &info)?;
    start_watching(&app, id, Path::new(&info.git_dir), info.root.clone());
    let row = state.db.projects()?.into_iter().find(|p| p.id == id).ok_or_else(|| AppError::invalid("project vanished"))?;
    Ok(to_project(row, info.remotes).await)
}

#[tauri::command]
#[specta::specta]
pub async fn remove_project(state: State<'_, AppState>, id: i32) -> AppResult<()> {
    state.watchers.lock().unwrap().remove(&id);
    state.db.remove_project(id)?;
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn project_status(state: State<'_, AppState>, id: i32) -> AppResult<Option<ProjectStatus>> {
    let row = find_row(&state, id)?;
    Ok(project::status(Path::new(&row.path)).await.ok())
}

fn find_row(state: &AppState, id: i32) -> AppResult<ProjectRow> {
    state.db.projects()?.into_iter().find(|p| p.id == id).ok_or_else(|| AppError::invalid("unknown project"))
}

/// Distinct project repos grouped by host.
pub fn repos_by_host(state: &AppState) -> AppResult<BTreeMap<String, Vec<(String, String)>>> {
    let mut map: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for (_, r) in state.db.project_remotes()? {
        let repos = map.entry(r.host).or_default();
        if !repos.iter().any(|(o, n)| o.eq_ignore_ascii_case(&r.owner) && n.eq_ignore_ascii_case(&r.repo)) {
            repos.push((r.owner, r.repo));
        }
    }
    Ok(map)
}

/// Open PRs across every remote of one project.
#[tauri::command]
#[specta::specta]
pub async fn project_prs(state: State<'_, AppState>, project_id: i32) -> AppResult<Vec<PrHit>> {
    let remotes = remotes_by_project(&state)?.remove(&project_id).unwrap_or_default();
    let mut by_host: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for r in remotes {
        by_host.entry(r.host).or_default().push((r.owner, r.repo));
    }
    let mut hits = Vec::new();
    for (host, repos) in by_host {
        let gh = state.client(&host).await?;
        for summary in prism_gh::pr::open_in_repos(&gh, &repos).await? {
            hits.push(PrHit { summary, project_id: Some(project_id) });
        }
    }
    hits.sort_by(|a, b| b.summary.updated_at.cmp(&a.summary.updated_at));
    Ok(hits)
}

/// Open PR counts across every remote, one GraphQL request per host.
#[tauri::command]
#[specta::specta]
pub async fn project_pr_counts(state: State<'_, AppState>) -> AppResult<Vec<PrCount>> {
    let remotes = remotes_by_project(&state)?;
    let mut counts: HashMap<(String, String, String), u32> = HashMap::new();
    for (host, repos) in repos_by_host(&state)? {
        let Ok(gh) = state.client(&host).await else { continue };
        let open = prism_gh::pr::open_pr_counts(&gh, &repos).await?;
        for ((owner, repo), n) in repos.into_iter().zip(open) {
            if let Some(n) = n {
                counts.insert((host.clone(), owner.to_ascii_lowercase(), repo.to_ascii_lowercase()), n);
            }
        }
    }
    Ok(remotes
        .into_iter()
        .map(|(project_id, rs)| {
            let by_repo: Vec<RepoCount> = rs
                .iter()
                .filter_map(|r| {
                    let n = counts.get(&(r.host.clone(), r.owner.to_ascii_lowercase(), r.repo.to_ascii_lowercase()))?;
                    Some(RepoCount { owner: r.owner.clone(), repo: r.repo.clone(), open: *n })
                })
                .collect();
            let open = (!by_repo.is_empty()).then(|| by_repo.iter().map(|c| c.open).sum());
            PrCount { project_id, open, by_repo }
        })
        .collect())
}

/// The first project with a remote pointing at `owner/repo` on `host`.
pub fn project_for(state: &AppState, host: &str, owner: &str, repo: &str) -> AppResult<Option<(i32, RemoteRepo)>> {
    Ok(state.db.project_remotes()?.into_iter().find_map(|(id, r)| {
        (r.host == host && r.owner.eq_ignore_ascii_case(owner) && r.repo.eq_ignore_ascii_case(repo))
            .then(|| (id, to_remote(r)))
    }))
}

#[tauri::command]
#[specta::specta]
pub async fn checkout_pr(
    state: State<'_, AppState>,
    project_id: i32,
    pr: PrRef,
    mode: CheckoutMode,
    detach: bool,
    force: bool,
) -> AppResult<CheckoutOutcome> {
    let row = find_row(&state, project_id)?;
    let remote = remotes_by_project(&state)?
        .remove(&project_id)
        .unwrap_or_default()
        .into_iter()
        .find(|r| r.host == pr.host && r.owner.eq_ignore_ascii_case(&pr.owner) && r.repo.eq_ignore_ascii_case(&pr.repo))
        .ok_or_else(|| AppError::invalid(format!("{} has no remote for {}/{}", row.path, pr.owner, pr.repo)))?;
    let cached = state
        .db
        .cached_pr(&pr.host, &pr.owner, &pr.repo, pr.number)?
        .ok_or_else(|| AppError::invalid("open the PR before checking it out"))?;

    let when_dirty = state.settings.read().unwrap().checkout.when_dirty;
    let mode = match (mode, when_dirty) {
        (CheckoutMode::Auto, DirtyCheckout::Worktree) => {
            let dirty = project::status(Path::new(&row.path)).await?.is_dirty();
            if dirty { CheckoutMode::Worktree } else { CheckoutMode::Auto }
        }
        (m, _) => m,
    };
    Ok(checkout::checkout(&CheckoutRequest {
        root: Path::new(&row.path),
        remote: &remote.remote,
        owner: &remote.owner,
        repo: &remote.repo,
        number: pr.number,
        head_sha: &cached.head_sha,
        mode,
        detach,
        force,
        worktrees: &state.worktrees_dir,
    })
    .await?)
}
