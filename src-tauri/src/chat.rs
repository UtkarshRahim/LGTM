//! Read-only questions about a pull request. One shot per question: the prompt
//! carries the last few turns, then the process exits.
//!
//! ponytail: the last 20 turns are pasted into a fresh prompt, not a warm ACP
//! session. Older turns are dropped; keep the process if answers start
//! forgetting earlier ones.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use prism_agent::{Agent, AgentSpec, Event};
use prism_gh::PrRef;
use prism_repo::MirrorRepo;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};
use tauri_specta::Event as _;
use tokio::sync::{mpsc, watch};

use crate::error::{AppError, AppResult};
use crate::prs;
use crate::state::AppState;

const HISTORY: usize = 20;
const DESC_CHARS: usize = 4000;
const QUESTION_CHARS: usize = 8000;
const CHAT_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const CANCEL_GRACE: Duration = Duration::from_secs(5);
const DIFF_FILE: &str = ".prism/pr.diff";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum ChatRole {
    User,
    Assistant,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct ChatMessage {
    pub id: i32,
    pub role: ChatRole,
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChatUpdate {
    Delta { text: String },
    Done { id: i32, body: String },
    Failed { error: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, tauri_specta::Event)]
pub struct ChatEvent {
    pub pr: PrRef,
    pub update: ChatUpdate,
}

/// Questions in flight, keyed by pull request, so a second question and an
/// auto review can wait instead of sharing the checkout.
#[derive(Default)]
pub struct Chats {
    inner: Mutex<std::collections::HashMap<String, watch::Sender<bool>>>,
}

impl Chats {
    pub fn busy(&self, key: &str) -> bool {
        self.inner.lock().unwrap().contains_key(key)
    }

    pub fn busy_count(&self) -> usize {
        self.inner.lock().unwrap().len()
    }

    /// `None` when this pull request already has a question in flight.
    pub fn start(&self, key: &str) -> Option<watch::Receiver<bool>> {
        let mut guard = self.inner.lock().unwrap();
        if guard.contains_key(key) {
            return None;
        }
        let (tx, rx) = watch::channel(false);
        guard.insert(key.to_string(), tx);
        Some(rx)
    }

    pub fn finish(&self, key: &str) {
        self.inner.lock().unwrap().remove(key);
    }

    pub fn cancel(&self, key: &str) {
        if let Some(tx) = self.inner.lock().unwrap().get(key) {
            let _ = tx.send(true);
        }
    }

    pub fn cancel_all(&self) {
        for tx in self.inner.lock().unwrap().values() {
            let _ = tx.send(true);
        }
    }
}

struct FinishChat {
    app: AppHandle,
    key: String,
}

impl Drop for FinishChat {
    fn drop(&mut self) {
        self.app.state::<AppState>().chats.finish(&self.key);
    }
}

fn message(id: i64, role: &str, body: String) -> Option<ChatMessage> {
    let role = match role {
        "user" => ChatRole::User,
        "assistant" => ChatRole::Assistant,
        _ => return None,
    };
    Some(ChatMessage { id: id as i32, role, body })
}

#[tauri::command]
#[specta::specta]
pub async fn pr_chat(state: State<'_, AppState>, pr: PrRef) -> AppResult<Vec<ChatMessage>> {
    let Some(pr_id) = state.db.pr_id(&pr.host, &pr.owner, &pr.repo, pr.number)? else {
        return Ok(Vec::new());
    };
    Ok(state.db.chat_messages(pr_id)?.into_iter().filter_map(|r| message(r.id, &r.role, r.body)).collect())
}

#[tauri::command]
#[specta::specta]
pub async fn cancel_chat(state: State<'_, AppState>, pr: PrRef) -> AppResult<()> {
    state.chats.cancel(&prs::key(&pr));
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn ask_chat(app: AppHandle, state: State<'_, AppState>, pr: PrRef, text: String) -> AppResult<ChatMessage> {
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err(AppError::invalid("Write a question first"));
    }
    if text.chars().count() > QUESTION_CHARS {
        return Err(AppError::invalid("Keep the question under 8000 characters"));
    }
    let agent_id = state.settings.read().unwrap().skills.agent.clone();
    let (spec, available) = prism_agent::catalog()
        .into_iter()
        .find(|(a, _)| a.id == agent_id)
        .ok_or_else(|| AppError::invalid(format!("Unknown agent {agent_id}")))?;
    if !available {
        return Err(AppError::invalid(format!("{} isn't installed", spec.name)));
    }
    let key = prs::key(&pr);
    if state.runs.busy(&key) {
        return Err(AppError::invalid("An auto review is running on this pull request"));
    }
    let Some(cancelled) = state.chats.start(&key) else {
        return Err(AppError::invalid("Still answering the previous question"));
    };
    let guard = FinishChat { app: app.clone(), key: key.clone() };
    let detail = prs::cached_detail(&state, &pr)?;
    let pr_id = state.db.pr_id(&pr.host, &pr.owner, &pr.repo, pr.number)?.expect("cached_detail found it");
    let id = state.db.add_chat(pr_id, "user", &text)?;
    let prior = state
        .db
        .chat_messages(pr_id)?
        .into_iter()
        .filter(|r| r.id != id)
        .map(|r| (r.role, r.body))
        .collect();
    let saved = message(id, "user", text.clone()).expect("user");
    let job = Job { app: app.clone(), pr, spec, detail, question: text, prior };
    tauri::async_runtime::spawn(async move {
        job.run(cancelled).await;
        drop(guard);
    });
    Ok(saved)
}

struct Job {
    app: AppHandle,
    pr: PrRef,
    spec: AgentSpec,
    detail: prism_gh::PrDetail,
    question: String,
    prior: Vec<(String, String)>,
}

enum Answer {
    Text(String),
    /// Cancelled. `String` is whatever had already been streamed.
    Stopped(String),
}

impl Job {
    fn emit(&self, update: ChatUpdate) {
        let _ = ChatEvent { pr: self.pr.clone(), update }.emit(&self.app);
    }

    async fn run(self, mut cancelled: watch::Receiver<bool>) {
        let outcome = tokio::time::timeout(CHAT_TIMEOUT, self.answer(&mut cancelled))
            .await
            .unwrap_or_else(|_| Err(AppError::invalid("The answer took longer than 10 minutes")));
        match outcome {
            Ok(Answer::Text(body)) => self.store(body),
            Ok(Answer::Stopped(body)) if !body.trim().is_empty() => self.store(body),
            Ok(Answer::Stopped(_)) => self.emit(ChatUpdate::Failed { error: "Cancelled".into() }),
            Err(e) => self.emit(ChatUpdate::Failed { error: e.to_string() }),
        }
    }

    fn store(&self, body: String) {
        let state = self.app.state::<AppState>();
        let saved = state
            .db
            .pr_id(&self.pr.host, &self.pr.owner, &self.pr.repo, self.pr.number)
            .ok()
            .flatten()
            .and_then(|id| state.db.add_chat(id, "assistant", &body).ok());
        match saved {
            Some(id) => self.emit(ChatUpdate::Done { id: id as i32, body }),
            None => self.emit(ChatUpdate::Failed { error: "Couldn't save the answer".into() }),
        }
    }

    async fn answer(&self, cancelled: &mut watch::Receiver<bool>) -> AppResult<Answer> {
        let state = self.app.state::<AppState>();
        let _ = prs::local(&state, &self.pr).await?;
        let cached = state.db.cached_pr(&self.pr.host, &self.pr.owner, &self.pr.repo, self.pr.number)?.expect("local caches the PR");
        let merge_base = cached.merge_base.clone().expect("local sets the merge base");
        if cached.head_sha != self.detail.head_sha {
            return Err(AppError::invalid("New commits arrived; reload the pull request and ask again"));
        }
        let dir = checkout_dir(&state, &self.pr);
        let token = state.token(&self.pr.host).await?;
        let repo = MirrorRepo { host: &self.pr.host, owner: &self.pr.owner, repo: &self.pr.repo, token: &token };
        state.mirrors.checkout(&repo, &dir, &merge_base, &self.detail.head_sha).await?;
        let root = dir.canonicalize().map_err(|e| AppError::invalid(format!("checkout missing: {e}")))?;
        if *cancelled.borrow() {
            return Ok(Answer::Stopped(String::new()));
        }

        let prompt = prompt(
            &self.detail.summary.title,
            &self.detail.body,
            &self.detail.head_ref,
            &self.detail.base_ref,
            DIFF_FILE,
            &self.prior,
            &self.question,
        );
        let (tx, mut rx) = mpsc::unbounded_channel();
        let agent = Agent::spawn(&self.spec, &root, tx).await.map_err(agent_err)?;
        let streamed = Arc::new(Mutex::new(String::new()));
        let app = self.app.clone();
        let pr = self.pr.clone();
        let live = streamed.clone();
        let forward = async move {
            while let Some(event) = rx.recv().await {
                if let Event::Message { text } = event {
                    live.lock().unwrap().push_str(&text);
                    let _ = ChatEvent { pr: pr.clone(), update: ChatUpdate::Delta { text } }.emit(&app);
                }
            }
        };
        let work = async {
            agent.initialize().await.map_err(agent_err)?;
            let session = agent.new_session(&root).await.map_err(agent_err)?;
            if let Some(mode) = session.read_only_mode() {
                agent.set_mode(&session.id, mode).await.map_err(agent_err)?;
            }
            let turn = {
                let turn = agent.prompt(&session.id, &prompt);
                tokio::pin!(turn);
                tokio::select! {
                    t = &mut turn => t.map_err(agent_err)?,
                    _ = async { drop(cancelled.wait_for(|c| *c).await) } => {
                        agent.cancel(&session.id).await;
                        let _ = tokio::time::timeout(CANCEL_GRACE, &mut turn).await;
                        return Ok(Answer::Stopped(streamed.lock().unwrap().clone()));
                    }
                }
            };
            if turn.stop_reason == "cancelled" {
                return Ok(Answer::Stopped(streamed.lock().unwrap().clone()));
            }
            let body = turn.text.trim().to_string();
            if body.is_empty() {
                return Err(AppError::invalid("The agent didn't answer"));
            }
            Ok(Answer::Text(body))
        };
        let outcome = tokio::select! {
            o = work => o,
            _ = forward => Err(AppError::invalid("The agent stopped unexpectedly")),
        };
        drop(agent);
        if let Err(e) = state.mirrors.restore(&root).await {
            tracing::warn!(error = %e, "chat checkout restore");
        }
        outcome
    }
}

fn checkout_dir(state: &AppState, pr: &PrRef) -> PathBuf {
    // Same folder as auto review. The busy flags keep the two from using it together.
    state.worktrees_dir.join("skills").join(&pr.host).join(&pr.owner).join(&pr.repo).join(format!("pr-{}", pr.number))
}

fn agent_err(e: prism_agent::AgentError) -> AppError {
    AppError::Agent { message: e.to_string() }
}

fn clip(s: &str, n: usize) -> String {
    match s.char_indices().nth(n) {
        Some((i, _)) => format!("{}...", &s[..i]),
        None => s.to_string(),
    }
}

pub(crate) fn prompt(
    title: &str,
    description: &str,
    head_ref: &str,
    base_ref: &str,
    diff_path: &str,
    history: &[(String, String)],
    question: &str,
) -> String {
    let mut out = format!(
        "Answer a question about this pull request. The diff is in `{diff_path}` (relative to this checkout). \
         Read that and the files it names. Do not edit files. Do not return JSON.\n\n\
         Title: {title}\nBranch: {head_ref} into {base_ref}\n"
    );
    let desc = clip(description.trim(), DESC_CHARS);
    if !desc.is_empty() {
        out.push_str("\nDescription:\n");
        out.push_str(&desc);
        out.push('\n');
    }
    let start = history.len().saturating_sub(HISTORY);
    if start < history.len() {
        out.push_str("\nConversation so far:\n");
        for (role, body) in &history[start..] {
            let who = if role == "user" { "User" } else { "Assistant" };
            out.push_str(who);
            out.push_str(": ");
            out.push_str(body);
            out.push('\n');
        }
    }
    out.push_str("\nQuestion:\n");
    out.push_str(question);
    out.push_str("\n\nAnswer in plain prose.\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_uses_the_diff_and_drops_old_turns() {
        let history: Vec<(String, String)> = (0..25)
            .map(|i| ((if i % 2 == 0 { "user" } else { "assistant" }).into(), format!("turn {i}")))
            .collect();
        let p = prompt("Fix ids", &"d".repeat(4001), "feat", "main", DIFF_FILE, &history, "Why 404?");
        assert!(p.contains(".prism/pr.diff"));
        assert!(p.contains("Why 404?"));
        assert!(p.contains("Fix ids"));
        assert!(p.contains("Do not edit files"));
        assert!(p.contains("Assistant: turn 5"));
        assert!(!p.contains("turn 4"));
        assert!(p.contains(&"d".repeat(4000)));
        assert!(!p.contains(&"d".repeat(4001)));
        assert!(!p.contains("```json"));
    }
}
