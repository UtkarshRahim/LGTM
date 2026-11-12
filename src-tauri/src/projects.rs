use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use futures_util::future::join_all;
use prism_repo::checkout::{self, CheckoutMode, CheckoutOutcome, CheckoutRequest};
use prism_repo::{ProjectStatus, RemoteRepo, project, watch};
use prism_store::{DirtyCheckout, NewProject, ProjectRow};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};
use tauri_specta::Event;

use crate::error::{AppError, AppResult};
use crate::state::AppState;

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct Project {
    pub id: i32,
    pub path: String,
    pub name: String,
    pub remote: Option<RemoteRepo>,
    /// `None` if the folder is missing or no longer a git repo.
    pub status: Option<ProjectStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, Event)]
pub struct ProjectStatusChanged {
    pub id: i32,
    pub status: Option<ProjectStatus>,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct PrCount {
    pub project_id: i32,
    pub open: Option<u32>,
}

fn remote_of(row: &ProjectRow) -> Option<RemoteRepo> {
    Some(RemoteRepo {
        remote: row.remote.clone()?,
        host: row.host.clone()?,
        owner: row.owner.clone()?,
        repo: row.repo.clone()?,
    })
}

async fn to_project(row: ProjectRow) -> Project {
    let status = project::status(Path::new(&row.path)).await.ok();
    Project {
        id: row.id,
        name: Path::new(&row.path).file_name().map_or(row.path.clone(), |n| n.to_string_lossy().into_owned()),
        remote: remote_of(&row),
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

pub async fn watch_all(app: &AppHandle) {
    let rows = match app.state::<AppState>().db.projects() {
        Ok(rows) => rows,
        Err(e) => return tracing::error!(error = %e, "loading projects"),
    };
    for row in rows {
        if let Ok(info) = project::inspect(Path::new(&row.path)).await {
            start_watching(app, row.id, Path::new(&info.git_dir), info.root);
        }
    }
}

#[tauri::command]
#[specta::specta]
pub async fn list_projects(state: State<'_, AppState>) -> AppResult<Vec<Project>> {
    let rows = state.db.projects()?;
    Ok(join_all(rows.into_iter().map(to_project)).await)
}

#[tauri::command]
#[specta::specta]
pub async fn add_project(app: AppHandle, state: State<'_, AppState>, path: String) -> AppResult<Project> {
    let info = project::inspect(Path::new(&path)).await.map_err(|e| match e {
        prism_repo::GitError::NotARepo { .. } => AppError::invalid(format!("{path} is not a git repository")),
        e => e.into(),
    })?;
    let r = info.remote.as_ref();
    let id = state.db.add_project(&NewProject {
        path: &info.root,
        host: r.map(|r| r.host.as_str()),
        owner: r.map(|r| r.owner.as_str()),
        repo: r.map(|r| r.repo.as_str()),
        remote: r.map(|r| r.remote.as_str()),
    })?;
    start_watching(&app, id, Path::new(&info.git_dir), info.root.clone());
    let row = state.db.projects()?.into_iter().find(|p| p.id == id).ok_or_else(|| AppError::invalid("project vanished"))?;
    Ok(to_project(row).await)
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
    let row = state.db.projects()?.into_iter().find(|p| p.id == id).ok_or_else(|| AppError::invalid("unknown project"))?;
    Ok(project::status(Path::new(&row.path)).await.ok())
}

/// Open PR counts, one GraphQL request per host.
#[tauri::command]
#[specta::specta]
pub async fn project_pr_counts(state: State<'_, AppState>) -> AppResult<Vec<PrCount>> {
    let mut by_host: BTreeMap<String, Vec<(i32, (String, String))>> = BTreeMap::new();
    for row in state.db.projects()? {
        if let Some(r) = remote_of(&row) {
            by_host.entry(r.host).or_default().push((row.id, (r.owner, r.repo)));
        }
    }
    let mut out = Vec::new();
    for (host, items) in by_host {
        let Ok(gh) = state.client(&host).await else { continue };
        let repos: Vec<_> = items.iter().map(|(_, r)| r.clone()).collect();
        let counts = prism_gh::pr::open_pr_counts(&gh, &repos).await?;
        out.extend(items.iter().zip(counts).map(|((id, _), open)| PrCount { project_id: *id, open }));
    }
    Ok(out)
}

/// The project whose remote points at `owner/repo` on `host`, if any.
pub fn project_for(state: &AppState, host: &str, owner: &str, repo: &str) -> AppResult<Option<ProjectRow>> {
    Ok(state.db.projects()?.into_iter().find(|p| {
        p.host.as_deref() == Some(host)
            && p.owner.as_deref().is_some_and(|o| o.eq_ignore_ascii_case(owner))
            && p.repo.as_deref().is_some_and(|r| r.eq_ignore_ascii_case(repo))
    }))
}

#[tauri::command]
#[specta::specta]
pub async fn checkout_pr(
    state: State<'_, AppState>,
    project_id: i32,
    number: u32,
    mode: CheckoutMode,
    detach: bool,
    force: bool,
) -> AppResult<CheckoutOutcome> {
    let row = state.db.projects()?.into_iter().find(|p| p.id == project_id).ok_or_else(|| AppError::invalid("unknown project"))?;
    let r = remote_of(&row).ok_or_else(|| AppError::invalid("project has no GitHub remote"))?;
    let cached = state
        .db
        .cached_pr(&r.host, &r.owner, &r.repo, number)?
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
        remote: &r.remote,
        owner: &r.owner,
        repo: &r.repo,
        number,
        head_sha: &cached.head_sha,
        mode,
        detach,
        force,
        worktrees: &state.worktrees_dir,
    })
    .await?)
}
