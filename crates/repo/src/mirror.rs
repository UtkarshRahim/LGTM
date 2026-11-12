//! Per-repo bare, blobless mirrors used for PR diffs. Only the commits a PR
//! needs are fetched (depth 1); file contents arrive lazily when `git diff`
//! asks for them. Users' own project folders are never touched.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::diff::{self, FileDiff};
use crate::git::{Git, GitResult};

pub struct Mirrors {
    root: PathBuf,
    locks: Mutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>,
}

pub struct MirrorRepo<'a> {
    pub host: &'a str,
    pub owner: &'a str,
    pub repo: &'a str,
    pub token: &'a str,
}

impl Mirrors {
    pub fn new(root: PathBuf) -> Self {
        Self { root, locks: Mutex::new(HashMap::new()) }
    }

    fn path(&self, r: &MirrorRepo<'_>) -> PathBuf {
        self.root.join(r.host).join(r.owner).join(format!("{}.git", r.repo))
    }

    fn lock(&self, path: &Path) -> Arc<tokio::sync::Mutex<()>> {
        self.locks.lock().unwrap().entry(path.to_owned()).or_default().clone()
    }

    /// True if both commits are already in the mirror, i.e. a diff needs no network
    /// beyond (possibly) lazy blob fetches.
    pub async fn has_commits(&self, r: &MirrorRepo<'_>, base: &str, head: &str) -> bool {
        let path = self.path(r);
        if !path.join("HEAD").exists() {
            return false;
        }
        has_commits(&path, base, head).await
    }

    /// Diff `base..head` for PR `number`, fetching whatever is missing.
    pub async fn diff(&self, r: &MirrorRepo<'_>, number: u32, base: &str, head: &str) -> GitResult<Vec<FileDiff>> {
        let pins = [(format!("refs/prism/pull/{number}/base"), base), (format!("refs/prism/pull/{number}/head"), head)];
        let mut out = self.diffs(r, &pins, &[(base, head)]).await?;
        Ok(out.remove(0))
    }

    /// Fetches the pinned commits if any are missing, keeps them reachable
    /// under the given refs, and returns one diff per `(from, to)` pair.
    pub async fn diffs(
        &self,
        r: &MirrorRepo<'_>,
        pins: &[(String, &str)],
        pairs: &[(&str, &str)],
    ) -> GitResult<Vec<Vec<FileDiff>>> {
        let path = self.path(r);
        let lock = self.lock(&path);
        let _guard = lock.lock().await;

        if !path.join("HEAD").exists() {
            init(&path, r).await?;
        }
        let git = Git::new(&path).with_token(r.token);
        let mut missing = Vec::new();
        for (_, sha) in pins {
            if !has_commit(&path, sha).await && !missing.contains(sha) {
                missing.push(*sha);
            }
        }
        if !missing.is_empty() {
            let mut args = vec!["fetch", "--quiet", "--no-tags", "--no-write-fetch-head", "--depth=1", "--filter=blob:none", "origin"];
            args.extend(&missing);
            git.run(args).await?;
            for (name, sha) in pins {
                git.run(["update-ref", name, sha]).await?;
            }
        }
        let mut out = Vec::with_capacity(pairs.len());
        for (from, to) in pairs {
            let text = git
                .output(["diff", "--no-ext-diff", "--no-textconv", "-M", "--src-prefix=a/", "--dst-prefix=b/", from, to])
                .await?;
            out.push(diff::parse_git_diff(&String::from_utf8_lossy(&text)));
        }
        Ok(out)
    }
}

async fn has_commit(path: &Path, sha: &str) -> bool {
    Git::new(path).offline().succeeds(["cat-file", "-e", &format!("{sha}^{{commit}}")]).await
}

async fn has_commits(path: &Path, base: &str, head: &str) -> bool {
    let git = Git::new(path).offline();
    let (head, base) = (format!("{head}^{{commit}}"), format!("{base}^{{commit}}"));
    let (a, b) = tokio::join!(git.succeeds(["cat-file", "-e", &head]), git.succeeds(["cat-file", "-e", &base]));
    a && b
}

async fn init(path: &Path, r: &MirrorRepo<'_>) -> GitResult<()> {
    tokio::fs::create_dir_all(path).await.map_err(|e| crate::GitError::Spawn(e.to_string()))?;
    let git = Git::new(path);
    git.run(["init", "--quiet", "--bare"]).await?;
    let url = format!("https://{}/{}/{}.git", r.host, r.owner, r.repo);
    for (k, v) in [
        ("core.repositoryformatversion", "1"),
        ("extensions.partialClone", "origin"),
        ("remote.origin.url", url.as_str()),
        ("remote.origin.promisor", "true"),
        ("remote.origin.partialCloneFilter", "blob:none"),
        ("gc.auto", "0"),
        ("maintenance.auto", "false"),
    ] {
        git.run(["config", k, v]).await?;
    }
    Ok(())
}

#[cfg(test)]
mod live {
    //! `PRISM_LIVE_TESTS=1 cargo test -p prism-repo live -- --nocapture`
    use super::*;

    #[tokio::test]
    async fn diffs_a_public_pr() {
        if std::env::var("PRISM_LIVE_TESTS").is_err() {
            return;
        }
        let token = String::from_utf8(std::process::Command::new("gh").args(["auth", "token"]).output().unwrap().stdout)
            .unwrap()
            .trim()
            .to_owned();
        let root = std::env::temp_dir().join(format!("prism-mirror-{}", std::process::id()));
        let mirrors = Mirrors::new(root.clone());
        let r = MirrorRepo { host: "github.com", owner: "cli", repo: "cli", token: &token };
        // cli/cli#9000: merge base and head from the GitHub API.
        let base = "63640b16a72f4752e7642e0b6bd8639b6b564183";
        let head = String::from_utf8(
            std::process::Command::new("gh").args(["api", "repos/cli/cli/pulls/9000", "--jq", ".head.sha"]).output().unwrap().stdout,
        )
        .unwrap()
        .trim()
        .to_owned();

        let t = std::time::Instant::now();
        let files = mirrors.diff(&r, 9000, base, &head).await.unwrap();
        let cold = t.elapsed();
        let t = std::time::Instant::now();
        let again = mirrors.diff(&r, 9000, base, &head).await.unwrap();
        println!("{} files; cold {cold:?}, warm {:?}", files.len(), t.elapsed());
        assert_eq!(files.len(), 4);
        assert_eq!(files, again);
        assert!(mirrors.has_commits(&r, base, &head).await);
        assert!(!mirrors.has_commits(&r, base, "0123456789abcdef0123456789abcdef01234567").await);
        let objects = Git::new(&mirrors.path(&r)).run(["count-objects", "-v"]).await.unwrap();
        println!("{objects}");
        let shallow = std::fs::read_to_string(mirrors.path(&r).join("shallow")).unwrap();
        assert_eq!(shallow.lines().count(), 2, "only the two PR commits are fetched");
        std::fs::remove_dir_all(root).unwrap();
    }
}
