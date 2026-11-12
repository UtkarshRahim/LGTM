//! A minimal Agent Client Protocol client: spawns an agent over stdio,
//! opens sessions, sends prompts and streams what the agent does.
//! Skill runs are read-only; see [`Agent`] for how that's enforced.

mod catalog;

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin};
use tokio::sync::{Mutex, mpsc, oneshot};

pub use catalog::{AgentSpec, catalog};

#[derive(Debug, Clone, thiserror::Error)]
pub enum AgentError {
    #[error("couldn't start {0}")]
    Spawn(String),
    #[error("the agent exited{0}")]
    Exited(String),
    #[error("{message}")]
    Rpc { code: i64, message: String },
    #[error("unexpected reply from the agent: {0}")]
    Protocol(String),
}

pub type AgentResult<T> = Result<T, AgentError>;

/// Something the agent did, streamed while a prompt runs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event {
    Message { text: String },
    Thought { text: String },
    /// A tool call started or changed; fields are `None` when unchanged.
    Tool { id: String, title: Option<String>, tool: Option<String>, status: Option<String> },
    /// LGTM refused a tool call the read-only policy doesn't allow.
    Denied { title: String, tool: String },
    Plan { entries: Vec<String> },
}

#[derive(Debug, Clone)]
pub struct SessionMode {
    pub id: String,
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone)]
pub struct Session {
    pub id: String,
    pub modes: Vec<SessionMode>,
    pub current_mode: Option<String>,
}

impl Session {
    /// The agent's own read-only mode, if it has one.
    pub fn read_only_mode(&self) -> Option<&str> {
        const IDS: [&str; 5] = ["ask", "read-only", "read_only", "readonly", "plan"];
        IDS.iter()
            .find_map(|want| self.modes.iter().find(|m| m.id.eq_ignore_ascii_case(want)))
            .or_else(|| self.modes.iter().find(|m| m.description.to_ascii_lowercase().contains("read-only")))
            .map(|m| m.id.as_str())
    }
}

#[derive(Debug, Clone)]
pub struct Turn {
    pub stop_reason: String,
    /// Everything the agent said during the turn.
    pub text: String,
}

type Pending = Arc<std::sync::Mutex<HashMap<u64, oneshot::Sender<AgentResult<Value>>>>>;

/// A running agent process.
///
/// Read-only means: file reads are served only inside `root`, writes and
/// terminals are refused, and permission requests are granted only for
/// reading, searching and thinking. Agents that edit without asking can't be
/// stopped by the protocol, so callers should also pick the agent's
/// read-only mode and run it in a throwaway checkout.
pub struct Agent {
    _child: Child,
    stdin: Arc<Mutex<ChildStdin>>,
    pending: Pending,
    next_id: AtomicU64,
    turn_text: Arc<std::sync::Mutex<String>>,
    stderr: Arc<std::sync::Mutex<VecDeque<String>>>,
}

const STDERR_LINES: usize = 20;
const ALLOWED_TOOLS: [&str; 4] = ["read", "search", "think", "fetch"];

impl Agent {
    pub async fn spawn(spec: &AgentSpec, root: &Path, events: mpsc::UnboundedSender<Event>) -> AgentResult<Self> {
        let mut cmd = prism_env::tokio_command(&spec.program).await;
        cmd.args(&spec.args)
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = cmd.spawn().map_err(|e| AgentError::Spawn(format!("{}: {e}", spec.program)))?;
        let stdin = Arc::new(Mutex::new(child.stdin.take().expect("piped")));
        let stdout = child.stdout.take().expect("piped");
        let stderr_pipe = child.stderr.take().expect("piped");

        let pending: Pending = Default::default();
        let turn_text = Arc::new(std::sync::Mutex::new(String::new()));
        let stderr = Arc::new(std::sync::Mutex::new(VecDeque::new()));

        let tail = stderr.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr_pipe).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let mut t = tail.lock().unwrap();
                if t.len() == STDERR_LINES {
                    t.pop_front();
                }
                t.push_back(line);
            }
        });

        let reader = Reader {
            stdin: stdin.clone(),
            pending: pending.clone(),
            turn_text: turn_text.clone(),
            stderr: stderr.clone(),
            events,
            root: root.canonicalize().unwrap_or_else(|_| root.to_owned()),
        };
        tokio::spawn(reader.run(BufReader::new(stdout)));

        Ok(Self { _child: child, stdin, pending, next_id: AtomicU64::new(1), turn_text, stderr })
    }

    async fn request(&self, method: &str, params: Value) -> AgentResult<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        let msg = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        if write(&self.stdin, &msg).await.is_err() {
            self.pending.lock().unwrap().remove(&id);
            return Err(self.exited());
        }
        rx.await.unwrap_or_else(|_| Err(self.exited()))
    }

    fn exited(&self) -> AgentError {
        exited_error(&self.stderr)
    }

    pub async fn initialize(&self) -> AgentResult<()> {
        let caps = json!({ "fs": { "readTextFile": true, "writeTextFile": false }, "terminal": false });
        let r = self.request("initialize", json!({ "protocolVersion": 1, "clientCapabilities": caps })).await?;
        match r.get("protocolVersion").and_then(Value::as_u64) {
            Some(1) => Ok(()),
            v => Err(AgentError::Protocol(format!("unsupported protocol version {v:?}"))),
        }
    }

    pub async fn new_session(&self, cwd: &Path) -> AgentResult<Session> {
        let r = self.request("session/new", json!({ "cwd": cwd, "mcpServers": [] })).await?;
        let id = r.get("sessionId").and_then(Value::as_str).ok_or_else(|| AgentError::Protocol("no sessionId".into()))?;
        let modes = r.pointer("/modes/availableModes").and_then(Value::as_array).cloned().unwrap_or_default();
        let s = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_owned();
        Ok(Session {
            id: id.to_owned(),
            modes: modes.iter().map(|m| SessionMode { id: s(m, "id"), name: s(m, "name"), description: s(m, "description") }).collect(),
            current_mode: r.pointer("/modes/currentModeId").and_then(Value::as_str).map(str::to_owned),
        })
    }

    pub async fn set_mode(&self, session: &str, mode: &str) -> AgentResult<()> {
        self.request("session/set_mode", json!({ "sessionId": session, "modeId": mode })).await.map(drop)
    }

    /// Sends a prompt and waits for the turn to end.
    pub async fn prompt(&self, session: &str, text: &str) -> AgentResult<Turn> {
        self.turn_text.lock().unwrap().clear();
        let r = self
            .request("session/prompt", json!({ "sessionId": session, "prompt": [{ "type": "text", "text": text }] }))
            .await?;
        Ok(Turn {
            stop_reason: r.get("stopReason").and_then(Value::as_str).unwrap_or("end_turn").to_owned(),
            text: std::mem::take(&mut *self.turn_text.lock().unwrap()),
        })
    }

    /// Asks the agent to stop the current turn; `prompt` then returns `cancelled`.
    pub async fn cancel(&self, session: &str) {
        let msg = json!({ "jsonrpc": "2.0", "method": "session/cancel", "params": { "sessionId": session } });
        let _ = write(&self.stdin, &msg).await;
    }
}

fn exited_error(stderr: &std::sync::Mutex<VecDeque<String>>) -> AgentError {
    let tail: Vec<String> = stderr.lock().unwrap().iter().rev().take(3).rev().cloned().collect();
    AgentError::Exited(if tail.is_empty() { String::new() } else { format!(": {}", tail.join(" / ")) })
}

async fn write(stdin: &Mutex<ChildStdin>, msg: &Value) -> std::io::Result<()> {
    let mut line = serde_json::to_vec(msg).expect("JSON values serialize");
    line.push(b'\n');
    let mut w = stdin.lock().await;
    w.write_all(&line).await?;
    w.flush().await
}

struct Reader {
    stdin: Arc<Mutex<ChildStdin>>,
    pending: Pending,
    turn_text: Arc<std::sync::Mutex<String>>,
    stderr: Arc<std::sync::Mutex<VecDeque<String>>>,
    events: mpsc::UnboundedSender<Event>,
    root: PathBuf,
}

impl Reader {
    async fn run(self, stdout: BufReader<tokio::process::ChildStdout>) {
        let mut lines = stdout.lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let Ok(msg) = serde_json::from_str::<Value>(&line) else {
                tracing::debug!(%line, "ignoring non-JSON line from agent");
                continue;
            };
            match (msg.get("id"), msg.get("method").and_then(Value::as_str)) {
                (Some(id), Some(method)) => {
                    let reply = match self.handle_request(method, msg.get("params").unwrap_or(&Value::Null)).await {
                        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
                        Err((code, message)) => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }),
                    };
                    let _ = write(&self.stdin, &reply).await;
                }
                (None, Some(method)) => self.handle_notification(method, msg.get("params").unwrap_or(&Value::Null)),
                (Some(id), None) => {
                    let Some(id) = id.as_u64() else { continue };
                    let Some(tx) = self.pending.lock().unwrap().remove(&id) else { continue };
                    let result = match msg.get("error") {
                        Some(e) => Err(AgentError::Rpc {
                            code: e.get("code").and_then(Value::as_i64).unwrap_or(0),
                            message: e.get("message").and_then(Value::as_str).unwrap_or("agent error").to_owned(),
                        }),
                        None => Ok(msg.get("result").cloned().unwrap_or(Value::Null)),
                    };
                    let _ = tx.send(result);
                }
                (None, None) => {}
            }
        }
        // Give the stderr reader a moment to collect the reason.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let err = exited_error(&self.stderr);
        for (_, tx) in self.pending.lock().unwrap().drain() {
            let _ = tx.send(Err(err.clone()));
        }
    }

    async fn handle_request(&self, method: &str, p: &Value) -> Result<Value, (i64, String)> {
        match method {
            "session/request_permission" => Ok(self.permission(p)),
            "fs/read_text_file" => {
                let path = p.get("path").and_then(Value::as_str).ok_or((-32602, "missing path".to_owned()))?;
                let real = Path::new(path).canonicalize().map_err(|e| (-32002, format!("{path}: {e}")))?;
                if !real.starts_with(&self.root) {
                    return Err((-32002, format!("{path} is outside the checkout")));
                }
                let text = tokio::fs::read_to_string(&real).await.map_err(|e| (-32002, format!("{path}: {e}")))?;
                let start = p.get("line").and_then(Value::as_u64).map_or(0, |l| l.saturating_sub(1) as usize);
                let limit = p.get("limit").and_then(Value::as_u64).map_or(usize::MAX, |l| l as usize);
                let content = if start == 0 && limit == usize::MAX {
                    text
                } else {
                    text.lines().skip(start).take(limit).collect::<Vec<_>>().join("\n")
                };
                Ok(json!({ "content": content }))
            }
            "fs/write_text_file" => Err((-32000, "this review is read-only".to_owned())),
            _ => Err((-32601, format!("{method} isn't supported"))),
        }
    }

    fn permission(&self, p: &Value) -> Value {
        let tool = p.pointer("/toolCall/kind").and_then(Value::as_str).unwrap_or("other");
        let title = p.pointer("/toolCall/title").and_then(Value::as_str).unwrap_or(tool).to_owned();
        let options = p.get("options").and_then(Value::as_array).cloned().unwrap_or_default();
        let pick = |kinds: &[&str]| {
            kinds.iter().find_map(|k| {
                options.iter().find(|o| o.get("kind").and_then(Value::as_str) == Some(k)).and_then(|o| o.get("optionId").cloned())
            })
        };
        let allowed = ALLOWED_TOOLS.contains(&tool);
        let choice = if allowed { pick(&["allow_once", "allow_always"]) } else { pick(&["reject_once", "reject_always"]) };
        if !allowed {
            let _ = self.events.send(Event::Denied { title, tool: tool.to_owned() });
        }
        match choice {
            Some(id) => json!({ "outcome": { "outcome": "selected", "optionId": id } }),
            None => json!({ "outcome": { "outcome": "cancelled" } }),
        }
    }

    fn handle_notification(&self, method: &str, p: &Value) {
        if method != "session/update" {
            return;
        }
        let Some(u) = p.get("update") else { return };
        let s = |k: &str| u.get(k).and_then(Value::as_str).map(str::to_owned);
        let event = match u.get("sessionUpdate").and_then(Value::as_str) {
            Some("agent_message_chunk") => {
                let text = u.pointer("/content/text").and_then(Value::as_str).unwrap_or("").to_owned();
                self.turn_text.lock().unwrap().push_str(&text);
                Event::Message { text }
            }
            Some("agent_thought_chunk") => {
                Event::Thought { text: u.pointer("/content/text").and_then(Value::as_str).unwrap_or("").to_owned() }
            }
            Some("tool_call" | "tool_call_update") => {
                Event::Tool { id: s("toolCallId").unwrap_or_default(), title: s("title"), tool: s("kind"), status: s("status") }
            }
            Some("plan") => Event::Plan {
                entries: u
                    .get("entries")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(|e| e.get("content").and_then(Value::as_str).map(str::to_owned)).collect())
                    .unwrap_or_default(),
            },
            _ => return,
        };
        let _ = self.events.send(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny ACP agent in Python that exercises permissions and file reads.
    const FAKE: &str = r#"
import json, sys
def send(o): sys.stdout.write(json.dumps(o) + "\n"); sys.stdout.flush()
pending = {}
for line in sys.stdin:
    m = json.loads(line)
    if m.get("method") == "initialize":
        send({"jsonrpc": "2.0", "id": m["id"], "result": {"protocolVersion": 1}})
    elif m.get("method") == "session/new":
        send({"jsonrpc": "2.0", "id": m["id"], "result": {"sessionId": "s1", "modes": {"currentModeId": "agent",
            "availableModes": [{"id": "agent", "name": "Agent"}, {"id": "ask", "name": "Ask"}]}}})
    elif m.get("method") == "session/set_mode":
        send({"jsonrpc": "2.0", "id": m["id"], "result": {}})
    elif m.get("method") == "session/prompt":
        pid = m["id"]
        opts = [{"optionId": "y", "kind": "allow_once"}, {"optionId": "n", "kind": "reject_once"}]
        send({"jsonrpc": "2.0", "id": 100, "method": "session/request_permission", "params": {"toolCall": {"kind": "edit", "title": "Edit a.txt"}, "options": opts}})
        send({"jsonrpc": "2.0", "id": 101, "method": "session/request_permission", "params": {"toolCall": {"kind": "read", "title": "Read a.txt"}, "options": opts}})
        send({"jsonrpc": "2.0", "id": 102, "method": "fs/read_text_file", "params": {"path": sys.argv[1] + "/a.txt", "line": 2, "limit": 1}})
        send({"jsonrpc": "2.0", "id": 103, "method": "fs/read_text_file", "params": {"path": "/etc/hosts"}})
        send({"jsonrpc": "2.0", "id": 104, "method": "fs/write_text_file", "params": {"path": "x", "content": ""}})
        answers = {}
        while len(answers) < 5:
            r = json.loads(sys.stdin.readline()); answers[r["id"]] = r
        summary = [answers[100]["result"]["outcome"]["optionId"], answers[101]["result"]["outcome"]["optionId"],
                   answers[102]["result"]["content"], "error" in answers[103], "error" in answers[104]]
        send({"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": "s1", "update": {"sessionUpdate": "tool_call", "toolCallId": "t1", "title": "Read", "kind": "read", "status": "pending"}}})
        send({"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": "s1", "update": {"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": json.dumps(summary)}}}})
        send({"jsonrpc": "2.0", "id": pid, "result": {"stopReason": "end_turn"}})
"#;

    #[tokio::test]
    async fn read_only_policy_over_acp() {
        let dir = std::env::temp_dir().join(format!("prism-acp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "one\ntwo\nthree\n").unwrap();
        std::fs::write(dir.join("fake.py"), FAKE).unwrap();
        let root = dir.canonicalize().unwrap();
        let spec = AgentSpec {
            id: "fake".into(),
            name: "Fake".into(),
            program: "python3".into(),
            args: vec![root.join("fake.py").to_string_lossy().into(), root.to_string_lossy().into()],
        };
        let (tx, mut rx) = mpsc::unbounded_channel();
        let agent = Agent::spawn(&spec, &root, tx).await.unwrap();
        agent.initialize().await.unwrap();
        let s = agent.new_session(&root).await.unwrap();
        assert_eq!(s.read_only_mode(), Some("ask"));
        agent.set_mode(&s.id, "ask").await.unwrap();
        let turn = agent.prompt(&s.id, "go").await.unwrap();
        assert_eq!(turn.stop_reason, "end_turn");
        assert_eq!(turn.text, r#"["n", "y", "two", true, true]"#);

        let mut events = Vec::new();
        while let Ok(e) = rx.try_recv() {
            events.push(e);
        }
        assert_eq!(events[0], Event::Denied { title: "Edit a.txt".into(), tool: "edit".into() });
        assert!(events.iter().any(|e| matches!(e, Event::Tool { id, .. } if id == "t1")));
        drop(agent);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn reports_why_an_agent_died() {
        let spec = AgentSpec {
            id: "dead".into(),
            name: "Dead".into(),
            program: "sh".into(),
            args: vec!["-c".into(), "echo 'not logged in' >&2; exit 1".into()],
        };
        let (tx, _rx) = mpsc::unbounded_channel();
        let agent = Agent::spawn(&spec, &std::env::temp_dir(), tx).await.unwrap();
        let err = agent.initialize().await.unwrap_err();
        assert!(err.to_string().contains("not logged in"), "{err}");
    }
}
