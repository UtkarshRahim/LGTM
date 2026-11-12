use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::git::{Git, GitError, GitResult};
use crate::project;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum CheckoutMode {
    /// `gh pr checkout` if the project is clean; otherwise ask.
    Auto,
    /// `git worktree add` at the PR head; the project folder is untouched.
    Worktree,
    /// Stash uncommitted work, then `gh pr checkout`.
    Stash,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CheckoutOutcome {
    CheckedOut { branch: Option<String>, stashed: bool },
    Worktree { path: String },
    /// The project has uncommitted changes; the caller must pick a mode.
    NeedsChoice { changed: u32 },
}

pub struct CheckoutRequest<'a> {
    pub root: &'a Path,
    /// Remote in the project that points at the PR's repo.
    pub remote: &'a str,
    pub owner: &'a str,
    pub repo: &'a str,
    pub number: u32,
    pub head_sha: &'a str,
    pub mode: CheckoutMode,
    pub detach: bool,
    pub force: bool,
    /// Where worktrees are created: `<worktrees>/<owner>/<repo>/pr-<n>`.
    pub worktrees: &'a Path,
}

pub async fn checkout(req: &CheckoutRequest<'_>) -> GitResult<CheckoutOutcome> {
    let status = project::status(req.root).await?;
    match (req.mode, status.is_dirty()) {
        (CheckoutMode::Worktree, _) => worktree(req).await,
        (CheckoutMode::Auto, true) => Ok(CheckoutOutcome::NeedsChoice { changed: status.changed }),
        (CheckoutMode::Auto, false) | (CheckoutMode::Stash, false) => gh_checkout(req, false).await,
        (CheckoutMode::Stash, true) => {
            let git = Git::new(req.root);
            git.run(["stash", "push", "--quiet", "-m", &format!("prism: before checking out #{}", req.number)])
                .await?;
            match gh_checkout(req, true).await {
                Ok(out) => Ok(out),
                Err(e) => {
                    if let Err(pop) = git.run(["stash", "pop", "--quiet"]).await {
                        tracing::error!(error = %pop, "restoring stash after failed checkout");
                    }
                    Err(e)
                }
            }
        }
    }
}

async fn gh_checkout(req: &CheckoutRequest<'_>, stashed: bool) -> GitResult<CheckoutOutcome> {
    let slug = format!("{}/{}", req.owner, req.repo);
    let number = req.number.to_string();
    let mut args = vec!["pr", "checkout", number.as_str(), "--repo", slug.as_str()];
    if req.detach {
        args.push("--detach");
    }
    if req.force {
        args.push("--force");
    }
    let out = prism_env::tokio_command("gh")
        .await
        .args(&args)
        .current_dir(req.root)
        .env("GH_PROMPT_DISABLED", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .await
        .map_err(|e| GitError::Spawn(format!("gh: {e}")))?;
    if !out.status.success() {
        return Err(GitError::Failed {
            args: format!("gh {}", args.join(" ")),
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_owned(),
        });
    }
    let branch = project::status(req.root).await?.branch;
    Ok(CheckoutOutcome::CheckedOut { branch, stashed })
}

async fn worktree(req: &CheckoutRequest<'_>) -> GitResult<CheckoutOutcome> {
    let path: PathBuf = req.worktrees.join(req.owner).join(req.repo).join(format!("pr-{}", req.number));
    let git = Git::new(req.root);
    git.run(["fetch", "--quiet", "--no-tags", req.remote, &format!("pull/{}/head", req.number)])
        .await?;

    if path.join(".git").exists() {
        Git::new(&path).run(["checkout", "--quiet", "--detach", req.head_sha]).await?;
    } else {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|e| GitError::Spawn(e.to_string()))?;
        }
        // Drop stale registrations whose directories were deleted by hand.
        git.run(["worktree", "prune"]).await?;
        let p = path.to_string_lossy();
        git.run(["worktree", "add", "--quiet", "--detach", p.as_ref(), req.head_sha]).await?;
    }
    Ok(CheckoutOutcome::Worktree { path: path.display().to_string() })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A project repo whose `origin` is a local bare repo with a fake
    /// `refs/pull/7/head`, so checkout paths run without network.
    async fn fixture(name: &str) -> (PathBuf, PathBuf, String) {
        let base = std::env::temp_dir().join(format!("prism-co-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (origin, project) = (base.join("origin.git"), base.join("project"));
        std::fs::create_dir_all(&project).unwrap();
        let g = Git::new(&project);
        let commit = ["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm"];
        g.run(["init", "-q", "-b", "main"]).await.unwrap();
        std::fs::write(project.join("a.txt"), "1").unwrap();
        g.run(["add", "."]).await.unwrap();
        g.run(commit.iter().copied().chain(["init"])).await.unwrap();
        g.run(["checkout", "-qb", "pr"]).await.unwrap();
        std::fs::write(project.join("b.txt"), "pr").unwrap();
        g.run(["add", "."]).await.unwrap();
        g.run(commit.iter().copied().chain(["pr"])).await.unwrap();
        let head = g.run(["rev-parse", "HEAD"]).await.unwrap().trim().to_owned();
        g.run(["checkout", "-q", "main"]).await.unwrap();
        Git::new(&base).run(["init", "-q", "--bare", "origin.git"]).await.unwrap();
        g.run(["remote", "add", "origin", origin.to_str().unwrap()]).await.unwrap();
        g.run(["push", "-q", "origin", "main", "pr:refs/pull/7/head"]).await.unwrap();
        g.run(["branch", "-qD", "pr"]).await.unwrap();
        (base, project, head)
    }

    fn req<'a>(project: &'a Path, wt: &'a Path, head: &'a str, mode: CheckoutMode) -> CheckoutRequest<'a> {
        CheckoutRequest {
            root: project,
            remote: "origin",
            owner: "acme",
            repo: "api",
            number: 7,
            head_sha: head,
            mode,
            detach: false,
            force: false,
            worktrees: wt,
        }
    }

    #[tokio::test]
    async fn dirty_project_asks_then_worktree_leaves_it_alone() {
        let (base, project, head) = fixture("wt").await;
        let wt = base.join("worktrees");
        std::fs::write(project.join("a.txt"), "local edit").unwrap();

        let out = checkout(&req(&project, &wt, &head, CheckoutMode::Auto)).await.unwrap();
        assert!(matches!(out, CheckoutOutcome::NeedsChoice { changed: 1 }));

        let CheckoutOutcome::Worktree { path } = checkout(&req(&project, &wt, &head, CheckoutMode::Worktree)).await.unwrap()
        else {
            panic!()
        };
        assert_eq!(std::fs::read_to_string(Path::new(&path).join("b.txt")).unwrap(), "pr");
        assert_eq!(std::fs::read_to_string(project.join("a.txt")).unwrap(), "local edit");

        // Second time reuses the existing worktree.
        assert!(checkout(&req(&project, &wt, &head, CheckoutMode::Worktree)).await.is_ok());
        std::fs::remove_dir_all(base).unwrap();
    }
}
