use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::git::{Git, GitResult};
use crate::remote::{self, RemoteRepo};

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct ProjectInfo {
    pub root: String,
    pub git_dir: String,
    /// Every forge repo among the remotes; the first is the primary.
    pub remotes: Vec<RemoteRepo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct ProjectStatus {
    /// `None` when HEAD is detached.
    pub branch: Option<String>,
    pub head_sha: Option<String>,
    /// Tracked files with staged or unstaged changes. Untracked files are not
    /// counted: scanning for them is slow on large repos and they rarely block
    /// a checkout.
    pub changed: u32,
}

impl ProjectStatus {
    pub fn is_dirty(&self) -> bool {
        self.changed > 0
    }
}

pub async fn inspect(path: &Path) -> GitResult<ProjectInfo> {
    let git = Git::new(path);
    let out = git.run(["rev-parse", "--show-toplevel", "--absolute-git-dir"]).await?;
    let mut lines = out.lines();
    let root = lines.next().unwrap_or_default().to_owned();
    let git_dir = lines.next().unwrap_or_default().to_owned();
    let remotes = Git::new(Path::new(&root)).run(["remote", "-v"]).await?;
    Ok(ProjectInfo { root, git_dir, remotes: remote::all(&remotes) })
}

pub async fn status(root: &Path) -> GitResult<ProjectStatus> {
    let out = Git::new(root)
        .run(["status", "--porcelain=v2", "--branch", "--untracked-files=no", "--ignore-submodules=dirty"])
        .await?;
    Ok(parse_status(&out))
}

fn parse_status(out: &str) -> ProjectStatus {
    let mut s = ProjectStatus { branch: None, head_sha: None, changed: 0 };
    for line in out.lines() {
        if let Some(h) = line.strip_prefix("# branch.head ") {
            s.branch = (h != "(detached)").then(|| h.to_owned());
        } else if let Some(oid) = line.strip_prefix("# branch.oid ") {
            s.head_sha = (oid != "(initial)").then(|| oid.to_owned());
        } else if matches!(line.as_bytes().first(), Some(b'1' | b'2' | b'u')) {
            s.changed += 1;
        }
    }
    s
}

pub fn git_dir_files(git_dir: &Path) -> [PathBuf; 2] {
    [git_dir.join("HEAD"), git_dir.join("index")]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn porcelain_v2() {
        let out = "# branch.oid abc123\n# branch.head feature/x\n# branch.upstream origin/feature/x\n\
                   1 .M N... 100644 100644 100644 a b src/a.rs\n2 R. N... 100644 100644 100644 a b R100 new\told\n";
        assert_eq!(
            parse_status(out),
            ProjectStatus { branch: Some("feature/x".into()), head_sha: Some("abc123".into()), changed: 2 }
        );
        let detached = parse_status("# branch.oid abc\n# branch.head (detached)\n");
        assert_eq!(detached.branch, None);
        assert!(!detached.is_dirty());
    }

    #[tokio::test]
    async fn inspects_a_real_repo() {
        let dir = std::env::temp_dir().join(format!("prism-proj-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let git = Git::new(&dir);
        git.run(["init", "-q", "-b", "main"]).await.unwrap();
        git.run(["remote", "add", "origin", "git@github.com:acme/api.git"]).await.unwrap();
        std::fs::write(dir.join("a.txt"), "1").unwrap();
        git.run(["add", "."]).await.unwrap();
        git.run(["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "init"]).await.unwrap();

        let info = inspect(&dir.join(".")).await.unwrap();
        assert_eq!(info.remotes[0].owner, "acme");
        assert!(info.git_dir.ends_with(".git"));

        let clean = status(&dir).await.unwrap();
        assert_eq!((clean.branch.as_deref(), clean.changed), (Some("main"), 0));
        std::fs::write(dir.join("a.txt"), "2").unwrap();
        assert!(status(&dir).await.unwrap().is_dirty());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
