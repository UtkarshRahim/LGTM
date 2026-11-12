//! Authoring a review: local drafts and viewed marks, submitting, resolving
//! threads, and the "since your review" diff.

use std::collections::HashMap;

use prism_gh::PrRef;
use prism_gh::pr;
use prism_gh::review::{self, NewComment, Reply, ReviewEvent};
use prism_gh::threads::DiffSide;
use prism_repo::MirrorRepo;
use prism_store::DraftTarget as StoredTarget;
use prism_store::DraftRow;
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::diffs::{FileMeta, PrDiff};
use crate::error::{AppError, AppResult};
use crate::prs::{self, key};
use crate::state::AppState;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "lowercase")]
pub enum CommentLabel {
    Praise,
    Nit,
    Suggestion,
    Question,
}

impl CommentLabel {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Praise => "praise",
            Self::Nit => "nit",
            Self::Suggestion => "suggestion",
            Self::Question => "question",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        [Self::Praise, Self::Nit, Self::Suggestion, Self::Question].into_iter().find(|l| l.as_str() == s)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DraftTarget {
    Line { path: String, line: u32, side: DiffSide, start_line: Option<u32>, start_side: Option<DiffSide> },
    Reply { thread_id: String },
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct Draft {
    pub id: i32,
    pub target: DraftTarget,
    pub body: String,
    pub label: Option<CommentLabel>,
    /// Head the draft was written against; line drafts on an older head
    /// aren't submitted until they're re-placed.
    pub head_sha: String,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct ViewedFile {
    pub path: String,
    pub patch_hash: String,
}

#[derive(Debug, Clone, Default, Serialize, specta::Type)]
pub struct LocalReview {
    pub drafts: Vec<Draft>,
    pub viewed: Vec<ViewedFile>,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct SinceReview {
    pub reviewed_sha: String,
    pub head_sha: String,
    /// The branch was force-pushed since your review, so only files whose
    /// change differs from what you reviewed are shown.
    pub rewritten: bool,
    pub files: Vec<FileMeta>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum DiffScope {
    All,
    Since,
}

fn side_str(s: DiffSide) -> &'static str {
    match s {
        DiffSide::Left => "LEFT",
        DiffSide::Right => "RIGHT",
    }
}

fn parse_side(s: &str) -> DiffSide {
    if s == "LEFT" { DiffSide::Left } else { DiffSide::Right }
}

pub(crate) fn to_stored(t: &DraftTarget) -> StoredTarget {
    match t {
        DraftTarget::Reply { thread_id } => StoredTarget::Reply { thread_id: thread_id.clone() },
        DraftTarget::Line { path, line, side, start_line, start_side } => StoredTarget::Line {
            path: path.clone(),
            line: *line,
            side: side_str(*side).into(),
            start_line: *start_line,
            start_side: start_side.map(|s| side_str(s).into()),
        },
    }
}

fn from_row(r: DraftRow) -> Draft {
    let target = match r.target {
        StoredTarget::Reply { thread_id } => DraftTarget::Reply { thread_id },
        StoredTarget::Line { path, line, side, start_line, start_side } => DraftTarget::Line {
            path,
            line,
            side: parse_side(&side),
            start_line,
            start_side: start_side.as_deref().map(parse_side),
        },
    };
    Draft {
        id: r.id as i32,
        target,
        body: r.body,
        label: r.label.as_deref().and_then(CommentLabel::parse),
        head_sha: r.head_sha,
    }
}

/// The body as posted: Conventional Comments puts the label first.
fn posted_body(label: Option<CommentLabel>, body: &str) -> String {
    match label {
        Some(l) => format!("**{}:** {body}", l.as_str()),
        None => body.to_owned(),
    }
}

fn pr_id(state: &AppState, pr: &PrRef) -> AppResult<Option<i64>> {
    Ok(state.db.pr_id(&pr.host, &pr.owner, &pr.repo, pr.number)?)
}

fn require_pr_id(state: &AppState, pr: &PrRef) -> AppResult<i64> {
    pr_id(state, pr)?.ok_or_else(|| AppError::invalid("Open the pull request first"))
}

#[tauri::command]
#[specta::specta]
pub async fn pr_local_review(state: State<'_, AppState>, pr: PrRef) -> AppResult<LocalReview> {
    let Some(id) = pr_id(&state, &pr)? else { return Ok(LocalReview::default()) };
    Ok(LocalReview {
        drafts: state.db.drafts(id)?.into_iter().map(from_row).collect(),
        viewed: state.db.viewed(id)?.into_iter().map(|(path, patch_hash)| ViewedFile { path, patch_hash }).collect(),
    })
}

#[tauri::command]
#[specta::specta]
pub async fn add_draft(
    state: State<'_, AppState>,
    pr: PrRef,
    head_sha: String,
    target: DraftTarget,
    body: String,
    label: Option<CommentLabel>,
) -> AppResult<Draft> {
    let id = require_pr_id(&state, &pr)?;
    let draft_id = state.db.add_draft(id, &to_stored(&target), &body, label.map(CommentLabel::as_str), &head_sha)?;
    Ok(Draft { id: draft_id as i32, target, body, label, head_sha })
}

#[tauri::command]
#[specta::specta]
pub async fn update_draft(state: State<'_, AppState>, id: i32, body: String, label: Option<CommentLabel>) -> AppResult<()> {
    if !state.db.update_draft(id.into(), &body, label.map(CommentLabel::as_str))? {
        return Err(AppError::invalid("That draft no longer exists"));
    }
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn delete_draft(state: State<'_, AppState>, id: i32) -> AppResult<()> {
    Ok(state.db.discard_draft(id.into())?)
}

/// Marks a file viewed at its current patch hash, or clears the mark.
#[tauri::command]
#[specta::specta]
pub async fn set_viewed(state: State<'_, AppState>, pr: PrRef, path: String, patch_hash: Option<String>) -> AppResult<()> {
    let id = require_pr_id(&state, &pr)?;
    Ok(state.db.set_viewed(id, &path, patch_hash.as_deref())?)
}

/// Posts every draft written against `head_sha` plus all queued replies,
/// then deletes them locally. Returns the review's URL.
#[tauri::command]
#[specta::specta]
pub async fn submit_review(
    state: State<'_, AppState>,
    pr: PrRef,
    head_sha: String,
    event: ReviewEvent,
    body: String,
) -> AppResult<String> {
    let id = require_pr_id(&state, &pr)?;
    let mut comments = Vec::new();
    let mut replies = Vec::new();
    let mut submitted = Vec::new();
    for d in state.db.drafts(id)?.into_iter().map(from_row) {
        let text = posted_body(d.label, &d.body);
        match d.target {
            DraftTarget::Reply { thread_id } => replies.push(Reply { thread_id, body: text }),
            DraftTarget::Line { path, line, side, start_line, start_side } if d.head_sha == head_sha => {
                comments.push(NewComment { path, line, side, start_line, start_side, body: text })
            }
            DraftTarget::Line { .. } => continue,
        }
        submitted.push(i64::from(d.id));
    }
    if event == ReviewEvent::Comment && body.trim().is_empty() && submitted.is_empty() {
        return Err(AppError::invalid("Write a comment or a summary first"));
    }
    let gh = state.client(&pr.host).await?;
    let url = review::submit_review(&gh, &pr, &head_sha, event, body.trim(), &comments, &replies).await?;
    state.db.delete_drafts(&submitted)?;
    Ok(url)
}

#[tauri::command]
#[specta::specta]
pub async fn set_thread_resolved(state: State<'_, AppState>, pr: PrRef, thread_id: String, resolved: bool) -> AppResult<()> {
    let gh = state.client(&pr.host).await?;
    Ok(review::set_thread_resolved(&gh, &pr, &thread_id, resolved).await?)
}

pub fn since_key(pr: &PrRef) -> String {
    format!("since:{}", key(pr))
}

/// What changed since your last submitted review, or `None` if you haven't
/// reviewed. Only files whose change differs from what you reviewed are
/// listed, diffed from the reviewed commit to the current head; this also
/// drops files that only arrived by merging the base branch in.
#[tauri::command]
#[specta::specta]
pub async fn pr_since_review(state: State<'_, AppState>, pr: PrRef) -> AppResult<Option<SinceReview>> {
    since(&state, &pr).await
}

pub(crate) async fn since(state: &AppState, pr: &PrRef) -> AppResult<Option<SinceReview>> {
    let gh = state.client(&pr.host).await?;
    let Some(reviewed) = review::last_reviewed_commit(&gh, pr).await? else { return Ok(None) };
    since_commit(state, &gh, pr, reviewed).await.map(Some)
}

async fn since_commit(state: &AppState, gh: &prism_gh::GitHub, pr: &PrRef, reviewed: String) -> AppResult<SinceReview> {
    let current = prs::local(state, pr).await?;
    let head = current.local_head().expect("local diffs know their head").to_owned();
    if reviewed == head {
        return Ok(SinceReview { reviewed_sha: reviewed, head_sha: head, rewritten: false, files: Vec::new() });
    }
    let detail = prs::cached_detail(state, pr)?;
    let (ancestor, old_base) =
        tokio::join!(pr::is_ancestor(gh, pr, &reviewed, &head), pr::merge_base(gh, pr, &detail.base_sha, &reviewed));
    let rewritten = !ancestor?;
    let tag = format!("{reviewed}..{head}");
    let skey = since_key(pr);
    if let Some(d) = state.diffs.get(&skey).filter(|d| d.is_local_at(&tag)) {
        return Ok(SinceReview { reviewed_sha: reviewed, head_sha: head, rewritten, files: d.metas() });
    }

    let old_base = old_base?;
    let token = state.token(&pr.host).await?;
    let repo = MirrorRepo { host: &pr.host, owner: &pr.owner, repo: &pr.repo, token: &token };
    let n = pr.number;
    let pins = [
        (format!("refs/prism/pull/{n}/reviewed-base"), old_base.as_str()),
        (format!("refs/prism/pull/{n}/reviewed"), reviewed.as_str()),
        (format!("refs/prism/pull/{n}/head"), head.as_str()),
    ];
    let mut out = state.mirrors.diffs(&repo, &pins, &[(&old_base, &reviewed), (&reviewed, &head)]).await?;
    let delta = out.pop().expect("two pairs");
    let then = out.pop().expect("two pairs");

    let then: HashMap<&str, String> = then.iter().map(|f| (f.path.as_str(), f.patch_hash())).collect();
    let now: HashMap<String, String> = current.metas().into_iter().map(|m| (m.path, m.patch_hash)).collect();
    let files = delta.into_iter().filter(|f| changed(then.get(f.path.as_str()), now.get(&f.path))).collect();
    let diff = state.diffs.insert(skey, PrDiff::new(files).local(&tag));
    Ok(SinceReview { reviewed_sha: reviewed, head_sha: head, rewritten, files: diff.metas() })
}

/// A file belongs in "since your review" if it's in the PR now or was then,
/// and its change isn't the one you reviewed.
fn changed(then: Option<&String>, now: Option<&String>) -> bool {
    match (then, now) {
        (None, None) => false,
        (a, b) => a != b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_prefix_the_body() {
        assert_eq!(posted_body(Some(CommentLabel::Nit), "rename"), "**nit:** rename");
        assert_eq!(posted_body(None, "rename"), "rename");
        assert_eq!(CommentLabel::parse("question"), Some(CommentLabel::Question));
    }

    #[test]
    fn since_filter() {
        let (a, b) = ("a".to_owned(), "b".to_owned());
        assert!(!changed(None, None), "only touched by the base branch");
        assert!(!changed(Some(&a), Some(&a)), "same change, e.g. after a rebase");
        assert!(changed(Some(&a), Some(&b)));
        assert!(changed(None, Some(&a)), "newly added to the PR");
        assert!(changed(Some(&a), None), "dropped from the PR");
    }

    /// Phase 3 gate: after a force-push, "since your review" shows only what
    /// changed. Uses the commit before the PR's last force-push as the review:
    /// `PRISM_SINCE_PR=owner/repo#123 cargo test -p prism gate_since -- --nocapture`
    #[tokio::test(flavor = "multi_thread")]
    async fn gate_since_force_push() {
        use prism_gh::{SearchQuery, query};
        use std::sync::{Mutex, RwLock};
        let Ok(spec) = std::env::var("PRISM_SINCE_PR") else { return };
        let SearchQuery::Pr { pr } = query::parse(&spec) else { panic!("PRISM_SINCE_PR must name one PR") };
        let dir = std::env::temp_dir().join(format!("prism-since-{}", std::process::id()));
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
        };
        state.credentials().await;
        let gh = state.client(&pr.host).await.unwrap();

        #[derive(Deserialize)]
        struct Data {
            repository: serde_json::Value,
        }
        let q = "query($o: String!, $r: String!, $n: Int!) { repository(owner: $o, name: $r) { pullRequest(number: $n) {
          timelineItems(itemTypes: [HEAD_REF_FORCE_PUSHED_EVENT], last: 1) { nodes { ... on HeadRefForcePushedEvent { beforeCommit { oid } } } } } } }";
        let d: Data = gh.graphql(q, serde_json::json!({ "o": pr.owner, "r": pr.repo, "n": pr.number }), "gate").await.unwrap();
        let before = d.repository["pullRequest"]["timelineItems"]["nodes"][0]["beforeCommit"]["oid"]
            .as_str()
            .expect("PR has a force-push")
            .to_owned();

        let t = std::time::Instant::now();
        prs::local(&state, &pr).await.unwrap();
        let all = state.diffs.get(&key(&pr)).unwrap().metas();
        let s = since_commit(&state, &gh, &pr, before.clone()).await.unwrap();
        let ms = t.elapsed().as_millis();

        let token = state.token(&pr.host).await.unwrap();
        let repo = MirrorRepo { host: &pr.host, owner: &pr.owner, repo: &pr.repo, token: &token };
        let raw = state.mirrors.diffs(&repo, &[], &[(&before, &s.head_sha)]).await.unwrap().remove(0);
        println!(
            "{spec}: {} files in PR, {} touched between {} and {}, {} shown since review (rewritten: {}) in {ms} ms",
            all.len(),
            raw.len(),
            &before[..8],
            &s.head_sha[..8],
            s.files.len(),
            s.rewritten,
        );
        for f in &s.files {
            println!("  {} +{} -{}", f.path, f.additions, f.deletions);
        }
        assert!(s.rewritten);
        assert!(s.files.len() <= raw.len());
        let in_pr: std::collections::HashSet<&str> = all.iter().map(|m| m.path.as_str()).collect();
        let raw_only = raw.iter().filter(|f| !in_pr.contains(f.path.as_str())).count();
        println!("  {raw_only} files changed only by the base branch were dropped or were removed from the PR");
        let hunks = state.diffs.get(&since_key(&pr)).unwrap().hunks(&s.files.iter().map(|f| f.path.clone()).collect::<Vec<_>>());
        assert_eq!(hunks.len(), s.files.len());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn targets_round_trip_through_the_store() {
        let t = DraftTarget::Line {
            path: "a.rs".into(),
            line: 3,
            side: DiffSide::Left,
            start_line: Some(1),
            start_side: Some(DiffSide::Left),
        };
        let row = DraftRow { id: 1, target: to_stored(&t), body: "x".into(), label: Some("nit".into()), head_sha: "h".into(), created_at: 0 };
        let d = from_row(row);
        assert_eq!((d.target, d.label), (t, Some(CommentLabel::Nit)));
    }
}
