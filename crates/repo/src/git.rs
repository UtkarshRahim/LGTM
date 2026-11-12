use std::ffi::OsStr;
use std::path::Path;

use base64::Engine;

#[derive(Debug, Clone, thiserror::Error)]
pub enum GitError {
    #[error("{path} is not a git repository")]
    NotARepo { path: String },
    #[error("`git {args}` failed: {stderr}")]
    Failed { args: String, stderr: String },
    #[error("couldn't run git: {0}")]
    Spawn(String),
}

pub type GitResult<T> = Result<T, GitError>;

/// Runs git with settings that make output stable regardless of user config.
pub struct Git<'a> {
    dir: &'a Path,
    token: Option<&'a str>,
    offline: bool,
}

impl<'a> Git<'a> {
    pub fn new(dir: &'a Path) -> Self {
        Self { dir, token: None, offline: false }
    }

    /// Authenticates HTTPS fetches with a token passed through the
    /// environment, so it never appears in process arguments.
    pub fn with_token(mut self, token: &'a str) -> Self {
        self.token = Some(token);
        self
    }

    /// Never fetch missing objects from a promisor remote. Without this, probing
    /// a partial clone for an absent commit downloads its entire history.
    pub fn offline(mut self) -> Self {
        self.offline = true;
        self
    }

    pub async fn run<I, S>(&self, args: I) -> GitResult<String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let out = self.output(args).await?;
        Ok(String::from_utf8_lossy(&out).into_owned())
    }

    pub async fn output<I, S>(&self, args: I) -> GitResult<Vec<u8>>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let args: Vec<_> = args.into_iter().map(|a| a.as_ref().to_owned()).collect();
        let mut cmd = prism_env::tokio_command("git").await;
        cmd.current_dir(self.dir)
            .args(["-c", "core.quotepath=false", "-c", "color.ui=false"])
            .args(&args)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .kill_on_drop(true);
        if self.offline {
            cmd.env("GIT_NO_LAZY_FETCH", "1");
        }
        if let Some(token) = self.token {
            let basic = base64::engine::general_purpose::STANDARD.encode(format!("x-access-token:{token}"));
            cmd.env("GIT_CONFIG_COUNT", "1")
                .env("GIT_CONFIG_KEY_0", "http.extraHeader")
                .env("GIT_CONFIG_VALUE_0", format!("Authorization: Basic {basic}"));
        }
        let out = cmd.output().await.map_err(|e| GitError::Spawn(e.to_string()))?;
        if out.status.success() {
            return Ok(out.stdout);
        }
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_owned();
        if stderr.contains("not a git repository") {
            return Err(GitError::NotARepo { path: self.dir.display().to_string() });
        }
        Err(GitError::Failed {
            args: args.iter().map(|a| a.to_string_lossy()).collect::<Vec<_>>().join(" "),
            stderr,
        })
    }

    /// True if the command exits 0; for probes like `cat-file -e`.
    pub async fn succeeds<I, S>(&self, args: I) -> bool
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.output(args).await.is_ok()
    }
}
