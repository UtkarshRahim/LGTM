//! macOS GUI apps launch with a minimal `PATH`, so `gh`, `git` and agent CLIs
//! installed via Homebrew, nvm, etc. are not found. We run the user's
//! interactive login shell in the background and apply its environment to
//! every child process. The last capture is cached on disk so commands spawned
//! right after launch don't wait for a slow shell.
//!
//! Spawn every external process through [`command`] or [`tokio_command`].

use std::collections::HashMap;
use std::ffi::OsStr;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Serialize;

const MARKER: &str = "__PRISM_ENV_START__";
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(10);
const SKIP: &[&str] = &["PWD", "OLDPWD", "SHLVL", "_", "TERM", "TERM_PROGRAM", "TERM_SESSION_ID"];

type Vars = Arc<HashMap<String, String>>;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum EnvSource {
    /// No capture available yet; children inherit Prism's own environment.
    Inherited,
    Cache,
    Fresh,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct ShellEnvStatus {
    pub shell: String,
    pub source: EnvSource,
    pub var_count: u32,
    pub capture_ms: Option<u32>,
    pub error: Option<String>,
}

struct Inner {
    vars: Option<Vars>,
    status: ShellEnvStatus,
    /// True once the background capture has finished, successfully or not.
    settled: bool,
}

struct ShellEnv {
    inner: Mutex<Inner>,
    settled: Condvar,
}

static ENV: OnceLock<ShellEnv> = OnceLock::new();

fn shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into())
}

fn global() -> &'static ShellEnv {
    ENV.get_or_init(|| ShellEnv {
        inner: Mutex::new(Inner {
            vars: None,
            status: ShellEnvStatus {
                shell: shell(),
                source: EnvSource::Inherited,
                var_count: 0,
                capture_ms: None,
                error: None,
            },
            settled: true,
        }),
        settled: Condvar::new(),
    })
}

/// Loads the cached environment (if any) and starts a fresh capture on a
/// background thread, refreshing the cache when it completes.
pub fn init(cache_path: PathBuf) {
    let env = global();
    {
        let mut inner = env.inner.lock().unwrap();
        if let Some(vars) = read_cache(&cache_path) {
            inner.status.source = EnvSource::Cache;
            inner.status.var_count = vars.len() as u32;
            inner.vars = Some(Arc::new(vars));
        }
        inner.settled = false;
    }

    std::thread::Builder::new()
        .name("shell-env".into())
        .spawn(move || {
            let shell = shell();
            let start = Instant::now();
            let result = capture(&shell);
            let elapsed = start.elapsed().as_millis() as u32;
            let mut inner = env.inner.lock().unwrap();
            inner.status.capture_ms = Some(elapsed);
            match result {
                Ok(vars) => {
                    if let Err(e) = write_cache(&cache_path, &vars) {
                        inner.status.error = Some(format!("writing env cache: {e}"));
                    }
                    inner.status.source = EnvSource::Fresh;
                    inner.status.var_count = vars.len() as u32;
                    inner.vars = Some(Arc::new(vars));
                }
                Err(e) => inner.status.error = Some(e),
            }
            inner.settled = true;
            env.settled.notify_all();
        })
        .expect("spawning shell-env thread");
}

pub fn status() -> ShellEnvStatus {
    global().inner.lock().unwrap().status.clone()
}

/// Returns the best environment available, waiting for the background
/// capture only when there is no cached copy.
fn vars_blocking() -> Option<Vars> {
    let env = global();
    let mut inner = env.inner.lock().unwrap();
    while inner.vars.is_none() && !inner.settled {
        inner = env.settled.wait(inner).unwrap();
    }
    inner.vars.clone()
}

fn vars_ready() -> Result<Option<Vars>, ()> {
    let inner = global().inner.lock().unwrap();
    if inner.vars.is_some() || inner.settled { Ok(inner.vars.clone()) } else { Err(()) }
}

fn overlay(vars: &HashMap<String, String>) -> impl Iterator<Item = (&String, &String)> {
    vars.iter().filter(|(k, _)| !SKIP.contains(&k.as_str()))
}

/// A `std::process::Command` with the login-shell environment applied.
/// May block on first-ever launch until the shell capture finishes.
pub fn command(program: impl AsRef<OsStr>) -> Command {
    let mut cmd = Command::new(program);
    if let Some(vars) = vars_blocking() {
        cmd.envs(overlay(&vars));
    }
    cmd
}

/// Async counterpart of [`command`]; never blocks the async runtime.
pub async fn tokio_command(program: impl AsRef<OsStr>) -> tokio::process::Command {
    let vars = match vars_ready() {
        Ok(v) => v,
        Err(()) => tokio::task::spawn_blocking(vars_blocking).await.ok().flatten(),
    };
    let mut cmd = tokio::process::Command::new(program);
    if let Some(vars) = vars {
        cmd.envs(overlay(&vars));
    }
    cmd
}

fn read_cache(path: &Path) -> Option<HashMap<String, String>> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

fn write_cache(path: &Path, vars: &HashMap<String, String>) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_vec(vars)?)?;
    std::fs::rename(tmp, path)
}

fn capture(shell: &str) -> Result<HashMap<String, String>, String> {
    let mut child = Command::new(shell)
        .args(["-l", "-i", "-c", &format!("printf '{MARKER}'; /usr/bin/env -0")])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("spawning {shell}: {e}"))?;

    // Read on a separate thread so a chatty rc file can't fill the pipe and deadlock.
    let mut stdout = child.stdout.take().unwrap();
    let reader = std::thread::spawn(move || {
        let mut out = Vec::new();
        stdout.read_to_end(&mut out).map(|_| out)
    });

    let deadline = Instant::now() + CAPTURE_TIMEOUT;
    loop {
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(_) => break,
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                return Err(format!("{shell} did not exit within {}s", CAPTURE_TIMEOUT.as_secs()));
            }
            None => std::thread::sleep(Duration::from_millis(10)),
        }
    }

    let out = reader.join().map_err(|_| "reader panicked")?.map_err(|e| e.to_string())?;
    parse(&out).ok_or_else(|| "shell output had no environment marker".into())
}

fn parse(out: &[u8]) -> Option<HashMap<String, String>> {
    let text = String::from_utf8_lossy(out);
    let (_, env) = text.split_once(MARKER)?;
    Some(
        env.split('\0')
            .filter_map(|kv| kv.split_once('='))
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_after_marker_ignoring_rc_noise() {
        let out = format!("welcome!\n{MARKER}PATH=/opt/homebrew/bin:/usr/bin\0MULTI=a\nb\0EQ=x=y\0");
        let vars = parse(out.as_bytes()).unwrap();
        assert_eq!(vars["PATH"], "/opt/homebrew/bin:/usr/bin");
        assert_eq!(vars["MULTI"], "a\nb");
        assert_eq!(vars["EQ"], "x=y");
    }

    #[tokio::test]
    async fn init_caches_and_applies_env() {
        let dir = std::env::temp_dir().join(format!("prism-env-{}", std::process::id()));
        let cache = dir.join("shell-env.json");
        let _ = std::fs::remove_dir_all(&dir);

        init(cache.clone());
        let out = tokio_command("sh").await.args(["-c", "echo $PATH"]).output().await.unwrap();
        let path = String::from_utf8(out.stdout).unwrap();

        let status = status();
        assert_eq!(status.source, EnvSource::Fresh, "{:?}", status.error);
        assert!(cache.exists());
        assert_eq!(path.trim(), read_cache(&cache).unwrap()["PATH"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
