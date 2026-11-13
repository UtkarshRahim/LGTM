//! Running review skills on a PR through an ACP agent, and the findings
//! they produce.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use prism_agent::{Agent, AgentSpec, Event};
use prism_gh::PrRef;
use prism_repo::MirrorRepo;
use prism_skills::{ChangedFile, Finding, PrContext, Severity, Skill};
use prism_store::{DraftTarget as StoredTarget, FindingRow, NewFinding, RunRow};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};
use tauri_specta::Event as _;
use tokio::sync::{mpsc, watch};

use crate::error::{AppError, AppResult};
use crate::projects::project_for;
use crate::prs;
use crate::review::{CommentLabel, Draft, DraftTarget};
use crate::state::AppState;

const RETRIES: usize = 2;
const RUN_TIMEOUT: Duration = Duration::from_secs(20 * 60);
const CANCEL_GRACE: Duration = Duration::from_secs(5);
const DIFF_FILE: &str = ".prism/pr.diff";

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct AgentInfo {
    pub id: String,
    pub name: String,
    /// Its CLI is installed.
    pub available: bool,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct SkillsView {
    pub skills: Vec<Skill>,
    pub agents: Vec<AgentInfo>,
    pub pinned: Option<String>,
    pub agent: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Running,
    Done,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct RunView {
    pub id: i32,
    pub skill: String,
    pub skill_name: String,
    pub agent: String,
    pub head_sha: String,
    pub status: RunStatus,
    pub error: Option<String>,
    /// The skill's writeup, without the findings JSON. Empty on older runs.
    pub report: Option<String>,
    pub started_at: u32,
    pub ended_at: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum FindingStatus {
    Open,
    Accepted,
    Dismissed,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct FindingView {
    pub id: i32,
    pub run_id: i32,
    /// Head the run reviewed; findings from older heads are stale.
    pub head_sha: String,
    pub path: String,
    pub line: Option<u32>,
    pub end_line: Option<u32>,
    pub severity: Severity,
    pub title: String,
    pub body: String,
    pub suggestion: Option<String>,
    pub status: FindingStatus,
    pub dismiss_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct SkillState {
    /// Newest first.
    pub runs: Vec<RunView>,
    pub findings: Vec<FindingView>,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RunUpdate {
    /// A step LGTM took, e.g. preparing the checkout.
    Step {
        text: String,
        warning: bool,
    },
    Agent {
        event: Event,
    },
    Finished {
        run: RunView,
        findings: Vec<FindingView>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, tauri_specta::Event)]
pub struct SkillRunEvent {
    pub run_id: i32,
    pub pr: PrRef,
    pub update: RunUpdate,
}

/// Live runs: their cancel switch and log, kept for the session.
#[derive(Default)]
pub struct Runs {
    inner: Mutex<HashMap<i64, Live>>,
}

struct Live {
    pr: String,
    pr_ref: PrRef,
    cancel: watch::Sender<bool>,
    log: Vec<RunUpdate>,
    running: bool,
}

fn run_view(r: RunRow) -> RunView {
    RunView {
        id: r.id as i32,
        skill: r.skill,
        skill_name: r.skill_name,
        agent: r.agent,
        head_sha: r.head_sha,
        status: match r.status.as_str() {
            "running" => RunStatus::Running,
            "done" => RunStatus::Done,
            "cancelled" => RunStatus::Cancelled,
            _ => RunStatus::Failed,
        },
        error: r.error,
        report: r.report,
        started_at: r.started_at as u32,
        ended_at: r.ended_at.map(|t| t as u32),
    }
}

fn finding_view(f: FindingRow) -> FindingView {
    FindingView {
        id: f.id as i32,
        run_id: f.run_id as i32,
        head_sha: f.head_sha,
        path: f.path,
        line: f.line,
        end_line: f.end_line,
        severity: Severity::parse(&f.severity).unwrap_or(Severity::Nit),
        title: f.title,
        body: f.body,
        suggestion: f.suggestion,
        status: match f.status.as_str() {
            "accepted" => FindingStatus::Accepted,
            "dismissed" => FindingStatus::Dismissed,
            _ => FindingStatus::Open,
        },
        dismiss_reason: f.dismiss_reason,
    }
}

fn expand_home(dir: &str) -> PathBuf {
    match dir.strip_prefix("~/") {
        Some(rest) => PathBuf::from(prism_env::var("HOME").unwrap_or_default()).join(rest),
        None => PathBuf::from(dir),
    }
}

fn discover(state: &AppState, pr: Option<&PrRef>) -> AppResult<Vec<Skill>> {
    let repo_root = match pr {
        Some(pr) => match project_for(state, &pr.host, &pr.owner, &pr.repo)? {
            Some((id, _)) => state.db.projects()?.into_iter().find(|p| p.id == id).map(|p| PathBuf::from(p.path)),
            None => None,
        },
        None => None,
    };
    let dirs: Vec<PathBuf> = state.settings.read().unwrap().skills.dirs.iter().map(|d| expand_home(d)).collect();
    Ok(prism_skills::discover(repo_root.as_deref(), &dirs))
}

fn agents() -> Vec<(AgentSpec, bool)> {
    prism_agent::catalog()
}

#[tauri::command]
#[specta::specta]
pub async fn list_skills(state: State<'_, AppState>, pr: Option<PrRef>) -> AppResult<SkillsView> {
    let skills = discover(&state, pr.as_ref())?;
    let agents = tauri::async_runtime::spawn_blocking(agents).await.expect("catalog doesn't panic");
    let s = state.settings.read().unwrap().skills.clone();
    Ok(SkillsView {
        skills,
        agents: agents.into_iter().map(|(a, available)| AgentInfo { id: a.id, name: a.name, available }).collect(),
        pinned: s.pinned,
        agent: s.agent,
    })
}

#[tauri::command]
#[specta::specta]
pub async fn set_skill_prefs(state: State<'_, AppState>, pinned: Option<String>, agent: String) -> AppResult<()> {
    let mut s = state.settings.read().unwrap().clone();
    s.skills.pinned = pinned;
    s.skills.agent = agent;
    s.save(&state.settings_path)?;
    *state.settings.write().unwrap() = s;
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn pr_skill_state(state: State<'_, AppState>, pr: PrRef) -> AppResult<SkillState> {
    let Some(id) = state.db.pr_id(&pr.host, &pr.owner, &pr.repo, pr.number)? else {
        return Ok(SkillState { runs: Vec::new(), findings: Vec::new() });
    };
    Ok(SkillState {
        runs: state.db.runs(id)?.into_iter().map(run_view).collect(),
        findings: state.db.findings(id)?.into_iter().map(finding_view).collect(),
    })
}

/// What a run has done so far in this session.
#[tauri::command]
#[specta::specta]
pub async fn skill_run_log(state: State<'_, AppState>, run_id: i32) -> AppResult<Vec<RunUpdate>> {
    Ok(state.runs.inner.lock().unwrap().get(&i64::from(run_id)).map(|l| l.log.clone()).unwrap_or_default())
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct RunningReview {
    pub run_id: i32,
    pub pr: PrRef,
}

impl Runs {
    pub fn running(&self) -> Vec<RunningReview> {
        let runs = self.inner.lock().unwrap();
        runs.iter()
            .filter(|(_, l)| l.running)
            .map(|(id, l)| RunningReview { run_id: *id as i32, pr: l.pr_ref.clone() })
            .collect()
    }

    pub fn cancel_all(&self) {
        for l in self.inner.lock().unwrap().values().filter(|l| l.running) {
            let _ = l.cancel.send(true);
        }
    }
}

/// Auto reviews still running, so closing a tab or the app can ask first.
#[tauri::command]
#[specta::specta]
pub async fn running_reviews(state: State<'_, AppState>) -> AppResult<Vec<RunningReview>> {
    Ok(state.runs.running())
}

/// Asks before quitting while auto reviews run; quitting cancels them so
/// their agents exit with the app.
pub fn request_quit(app: &AppHandle) {
    use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
    let n = app.state::<AppState>().runs.running().len();
    if n == 0 {
        app.exit(0);
        return;
    }
    let app = app.clone();
    let what = if n == 1 { "An auto review is".to_string() } else { format!("{n} auto reviews are") };
    app.dialog()
        .message(format!("{what} still running. Quitting stops it."))
        .title("Quit LGTM?")
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCancelCustom("Quit".into(), "Cancel".into()))
        .show(move |quit| {
            if !quit {
                return;
            }
            app.state::<AppState>().runs.cancel_all();
            tauri::async_runtime::spawn(async move {
                for _ in 0..40 {
                    if app.state::<AppState>().runs.running().is_empty() {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                app.exit(0);
            });
        });
}

#[tauri::command]
#[specta::specta]
pub async fn cancel_skill_run(state: State<'_, AppState>, run_id: i32) -> AppResult<()> {
    if let Some(l) = state.runs.inner.lock().unwrap().get(&i64::from(run_id)) {
        let _ = l.cancel.send(true);
    }
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn run_skill(app: AppHandle, state: State<'_, AppState>, pr: PrRef, skill_id: String) -> AppResult<RunView> {
    let skill = discover(&state, Some(&pr))?
        .into_iter()
        .find(|s| s.id == skill_id)
        .ok_or_else(|| AppError::invalid("That review is no longer there"))?;
    let agent_id = state.settings.read().unwrap().skills.agent.clone();
    let (spec, available) = agents()
        .into_iter()
        .find(|(a, _)| a.id == agent_id)
        .ok_or_else(|| AppError::invalid(format!("Unknown agent {agent_id}")))?;
    if !available {
        return Err(AppError::invalid(format!("{} isn't installed", spec.name)));
    }
    let key = prs::key(&pr);
    if state.runs.inner.lock().unwrap().values().any(|l| l.running && l.pr == key) {
        return Err(AppError::invalid("An auto review is already running on this pull request"));
    }
    let detail = prs::cached_detail(&state, &pr)?;
    let pr_id = state.db.pr_id(&pr.host, &pr.owner, &pr.repo, pr.number)?.expect("cached_detail found it");
    let run_id = state.db.start_run(pr_id, &skill.id, &skill.name, &spec.id, &detail.head_sha)?;
    let (cancel, cancelled) = watch::channel(false);
    state.runs.inner.lock().unwrap().insert(run_id, Live { pr: key, pr_ref: pr.clone(), cancel, log: Vec::new(), running: true });

    let run = state.db.runs(pr_id)?.into_iter().find(|r| r.id == run_id).map(run_view).expect("just started");
    let job = Job { app: app.clone(), pr: pr.clone(), run_id, skill, spec, head: detail.head_sha.clone() };
    tauri::async_runtime::spawn(job.run(detail, cancelled));
    Ok(run)
}

struct Job {
    app: AppHandle,
    pr: PrRef,
    run_id: i64,
    skill: Skill,
    spec: AgentSpec,
    head: String,
}

enum Outcome {
    Done { findings: Vec<Finding>, report: Option<String> },
    Cancelled,
}

impl Job {
    fn emit(&self, update: RunUpdate) {
        let state = self.app.state::<AppState>();
        if let Some(l) = state.runs.inner.lock().unwrap().get_mut(&self.run_id) {
            l.log.push(update.clone());
        }
        let _ = SkillRunEvent { run_id: self.run_id as i32, pr: self.pr.clone(), update }.emit(&self.app);
    }

    async fn run(self, detail: prism_gh::PrDetail, cancelled: watch::Receiver<bool>) {
        let state = self.app.state::<AppState>();
        let target = Target { pr: &self.pr, skill: &self.skill, spec: &self.spec, head: &self.head, detail: &detail };
        let result = tokio::time::timeout(RUN_TIMEOUT, execute(&state, &target, cancelled, &|u| self.emit(u)))
            .await
            .unwrap_or_else(|_| Err(AppError::invalid("The run took longer than 20 minutes")));
        let (status, error, findings, report) = match result {
            Ok(Outcome::Done { findings, report }) => ("done", None, findings, report),
            Ok(Outcome::Cancelled) => ("cancelled", None, Vec::new(), None),
            Err(e) => ("failed", Some(e.to_string()), Vec::new(), None),
        };
        let rows: Vec<NewFinding<'_>> = findings
            .iter()
            .map(|f| NewFinding {
                path: &f.path,
                line: f.line,
                end_line: f.end_line,
                severity: f.severity.as_str(),
                title: &f.title,
                body: &f.body,
                suggestion: f.suggestion.as_deref(),
            })
            .collect();
        if let Err(e) = state.db.finish_run(self.run_id, status, error.as_deref(), report.as_deref(), &rows) {
            tracing::error!(error = %e, "saving skill run");
        }
        let pr_id = state.db.pr_id(&self.pr.host, &self.pr.owner, &self.pr.repo, self.pr.number).ok().flatten();
        let run =
            pr_id.and_then(|id| state.db.runs(id).ok()).and_then(|rs| rs.into_iter().find(|r| r.id == self.run_id));
        let findings = pr_id
            .and_then(|id| state.db.findings(id).ok())
            .map(|fs| fs.into_iter().filter(|f| f.run_id == self.run_id).map(finding_view).collect())
            .unwrap_or_default();
        if let Some(run) = run {
            self.emit(RunUpdate::Finished { run: run_view(run), findings });
        }
        if let Some(l) = state.runs.inner.lock().unwrap().get_mut(&self.run_id) {
            l.running = false;
        }
    }
}

struct Target<'a> {
    pr: &'a PrRef,
    skill: &'a Skill,
    spec: &'a AgentSpec,
    head: &'a str,
    detail: &'a prism_gh::PrDetail,
}

/// Checks out the PR head, runs the skill read-only and reads its findings.
async fn execute(
    state: &AppState,
    t: &Target<'_>,
    mut cancelled: watch::Receiver<bool>,
    emit: &(dyn Fn(RunUpdate) + Sync),
) -> AppResult<Outcome> {
    let step = |text: String| emit(RunUpdate::Step { text, warning: false });
    let warn = |text: String| emit(RunUpdate::Step { text, warning: true });
    let (pr, detail) = (t.pr, t.detail);

    step("Preparing a checkout of the PR head".into());
    let diff = prs::local(state, pr).await?;
    let cached = state.db.cached_pr(&pr.host, &pr.owner, &pr.repo, pr.number)?.expect("local caches the PR");
    let merge_base = cached.merge_base.clone().expect("local sets the merge base");
    if cached.head_sha != t.head {
        return Err(AppError::invalid("New commits arrived; reload the PR and run again"));
    }
    let dir = state
        .worktrees_dir
        .join("skills")
        .join(&pr.host)
        .join(&pr.owner)
        .join(&pr.repo)
        .join(format!("pr-{}", pr.number));
    let token = state.token(&pr.host).await?;
    let repo = MirrorRepo { host: &pr.host, owner: &pr.owner, repo: &pr.repo, token: &token };
    state.mirrors.checkout(&repo, &dir, &merge_base, t.head).await?;
    let root = dir.canonicalize().map_err(|e| AppError::invalid(format!("checkout missing: {e}")))?;
    if *cancelled.borrow() {
        return Ok(Outcome::Cancelled);
    }

    let files: Vec<ChangedFile> = diff
        .ranges()
        .into_iter()
        .map(|(path, status, ranges)| ChangedFile { path, status: status_word(status), ranges })
        .collect();
    let body = prism_skills::load_body(Path::new(&t.skill.dir))
        .map_err(|e| AppError::invalid(format!("Couldn't read {}: {e}", t.skill.name)))?;
    let prompt = prism_skills::prompt(
        &t.skill.name,
        &body,
        &PrContext {
            title: &detail.summary.title,
            description: &detail.body,
            head_ref: &detail.head_ref,
            base_ref: &detail.base_ref,
            head_sha: t.head,
            merge_base: &merge_base,
            diff_path: DIFF_FILE,
            files: &files,
        },
    );

    step(format!("Starting {}", t.spec.name));
    let (tx, mut rx) = mpsc::unbounded_channel();
    let agent = Agent::spawn(t.spec, &root, tx).await.map_err(agent_err)?;
    let forward = async {
        while let Some(event) = rx.recv().await {
            if !matches!(event, Event::Thought { .. }) {
                emit(RunUpdate::Agent { event });
            }
        }
    };
    let work = async {
        agent.initialize().await.map_err(agent_err)?;
        let session = agent.new_session(&root).await.map_err(agent_err)?;
        match session.read_only_mode() {
            Some(mode) => {
                agent.set_mode(&session.id, mode).await.map_err(agent_err)?;
                let name = session.modes.iter().find(|m| m.id == mode).map_or(mode, |m| m.name.as_str());
                step(format!("Using {}'s {name} mode (read-only)", t.spec.name));
            }
            None => warn(format!("{} has no read-only mode; edits it asks to make are refused", t.spec.name)),
        }
        step(format!("Running {}", t.skill.name));
        let mut text = prompt;
        for attempt in 0..=RETRIES {
            let turn = {
                let turn = agent.prompt(&session.id, &text);
                tokio::pin!(turn);
                tokio::select! {
                    t = &mut turn => t.map_err(agent_err)?,
                    _ = async { drop(cancelled.wait_for(|c| *c).await) } => {
                        agent.cancel(&session.id).await;
                        let _ = tokio::time::timeout(CANCEL_GRACE, &mut turn).await;
                        return Ok(Outcome::Cancelled);
                    }
                }
            };
            if turn.stop_reason == "cancelled" {
                return Ok(Outcome::Cancelled);
            }
            match prism_skills::parse_reply(&turn.text, &root.to_string_lossy()) {
                Ok((findings, report)) => return Ok(Outcome::Done { findings, report }),
                Err(e) if attempt < RETRIES => {
                    warn(format!("Couldn't read the findings ({e}); asking again"));
                    text = prism_skills::retry_prompt(&e);
                }
                Err(e) => {
                    return Err(AppError::invalid(format!("The agent didn't return findings: {e}")));
                }
            }
        }
        unreachable!("the loop returns on its last attempt")
    };
    let outcome = tokio::select! {
        o = work => o,
        _ = forward => Err(AppError::invalid("The agent stopped unexpectedly")),
    };
    drop(agent);

    match state.mirrors.restore(&root).await {
        Ok(changed) if !changed.is_empty() => warn(format!(
            "The agent changed {} file(s) in LGTM's checkout despite read-only mode; they were reverted: {}",
            changed.len(),
            changed.join(", ")
        )),
        Ok(_) => {}
        Err(e) => warn(format!("Couldn't check the checkout for changes: {e}")),
    }
    let outcome = outcome?;
    if let Outcome::Done { findings, .. } = &outcome {
        step(format!("{} finding{}", findings.len(), if findings.len() == 1 { "" } else { "s" }));
    }
    Ok(outcome)
}

fn agent_err(e: prism_agent::AgentError) -> AppError {
    AppError::Agent { message: e.to_string() }
}

fn status_word(s: prism_repo::diff::FileStatus) -> &'static str {
    use prism_repo::diff::FileStatus::*;
    match s {
        Added => "added",
        Deleted => "deleted",
        Modified => "modified",
        Renamed => "renamed",
        Copied => "copied",
    }
}

/// Turns a finding into a draft comment, with the body as you edited it.
#[tauri::command]
#[specta::specta]
pub async fn accept_finding(
    state: State<'_, AppState>,
    pr: PrRef,
    finding_id: i32,
    head_sha: String,
    target: DraftTarget,
    body: String,
    label: Option<CommentLabel>,
) -> AppResult<Draft> {
    let pr_id = state
        .db
        .pr_id(&pr.host, &pr.owner, &pr.repo, pr.number)?
        .ok_or_else(|| AppError::invalid("Open the pull request first"))?;
    let stored = crate::review::to_stored(&target);
    if !matches!(stored, StoredTarget::Line { .. }) {
        return Err(AppError::invalid("Findings become line comments"));
    }
    let label_str = label.map(|l| l.as_str());
    let id = state.db.accept_finding(finding_id.into(), pr_id, &stored, &body, label_str, &head_sha)?;
    Ok(Draft { id: id as i32, target, body, label, head_sha })
}

/// Dismisses a finding with a reason, or reopens it with `None`.
#[tauri::command]
#[specta::specta]
pub async fn dismiss_finding(state: State<'_, AppState>, finding_id: i32, reason: Option<String>) -> AppResult<()> {
    Ok(state.db.dismiss_finding(finding_id.into(), reason.as_deref())?)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Phase 4 gate: a skill run lands findings on a real PR.
    /// `PRISM_SKILL_PR=owner/repo#123 cargo test -p prism gate_skill -- --nocapture` (uses cursor-agent)
    #[tokio::test(flavor = "multi_thread")]
    async fn gate_skill_run() {
        use std::sync::{Mutex as StdMutex, RwLock};
        let Ok(spec) = std::env::var("PRISM_SKILL_PR") else { return };
        let prism_gh::SearchQuery::Pr { pr } = prism_gh::query::parse(&spec) else { panic!("bad PRISM_SKILL_PR") };
        let dir = std::env::temp_dir().join(format!("prism-skill-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let skills_dir = dir.join("skills");
        std::fs::create_dir_all(skills_dir.join("bug-hunt")).unwrap();
        std::fs::write(
            skills_dir.join("bug-hunt/SKILL.md"),
            "---\nname: bug-hunt\ndescription: Find correctness bugs\n---\nReview the change for correctness bugs, \
             unhandled errors and missing tests. Report only the few issues that matter most.\n",
        )
        .unwrap();
        let mut settings = prism_store::Settings::default();
        settings.skills.dirs = vec![skills_dir.to_string_lossy().into()];
        let state = AppState {
            db: prism_store::Db::open(&dir.join("prism.db")).unwrap(),
            db_path: dir.join("prism.db"),
            settings: RwLock::new(settings),
            settings_path: dir.join("settings.toml"),
            mirrors: prism_repo::Mirrors::new(dir.join("mirrors")),
            worktrees_dir: dir.join("worktrees"),
            creds: tokio::sync::RwLock::new(None),
            watchers: StdMutex::new(Default::default()),
            diffs: Default::default(),
            runs: Default::default(),
        };
        state.credentials().await;
        let diff = prs::local(&state, &pr).await.unwrap();
        let detail = prs::cached_detail(&state, &pr).unwrap();
        let skill = discover(&state, Some(&pr)).unwrap().into_iter().find(|s| s.name == "bug-hunt").unwrap();
        let (agent, available) = agents().into_iter().find(|(a, _)| a.id == "cursor").unwrap();
        assert!(available, "cursor-agent isn't installed");

        let log = StdMutex::new(Vec::new());
        let (_cancel, cancelled) = watch::channel(false);
        let t0 = std::time::Instant::now();
        let target = Target { pr: &pr, skill: &skill, spec: &agent, head: &detail.head_sha, detail: &detail };
        let outcome = execute(&state, &target, cancelled, &|u| log.lock().unwrap().push(u)).await.unwrap();
        let secs = t0.elapsed().as_secs();
        let Outcome::Done { findings, .. } = outcome else { panic!("cancelled") };

        let log = log.into_inner().unwrap();
        for u in &log {
            match u {
                RunUpdate::Step { text, warning } => println!("{} {text}", if *warning { "!" } else { "·" }),
                RunUpdate::Agent { event: Event::Tool { title: Some(t), .. } } => println!("  tool: {t}"),
                RunUpdate::Agent { event: Event::Denied { title, .. } } => println!("  denied: {title}"),
                _ => {}
            }
        }
        let ranges: HashMap<String, Vec<(u32, u32)>> = diff.ranges().into_iter().map(|(p, _, r)| (p, r)).collect();
        let mut inline = 0;
        for f in &findings {
            let on_diff = f.line.is_some_and(|l| ranges.get(&f.path).is_some_and(|r| r.iter().any(|&(a, b)| a <= l && l <= b)));
            inline += usize::from(on_diff);
            println!("[{:?}] {}:{:?} {} {}", f.severity, f.path, f.line, f.title, if on_diff { "" } else { "(off diff)" });
        }
        println!("{} findings, {inline} on changed lines, in {secs} s", findings.len());
        assert!(log.iter().any(|u| matches!(u, RunUpdate::Step { text, .. } if text.contains("read-only"))));
        assert!(!log.iter().any(|u| matches!(u, RunUpdate::Step { text, .. } if text.contains("reverted"))));
        assert!(findings.iter().all(|f| ranges.contains_key(&f.path)), "findings name files in the PR");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn expands_home() {
        let home = prism_env::var("HOME").unwrap();
        assert_eq!(expand_home("~/.claude/skills"), PathBuf::from(home).join(".claude/skills"));
        assert_eq!(expand_home("/abs"), PathBuf::from("/abs"));
    }
}
