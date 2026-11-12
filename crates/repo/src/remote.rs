use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub struct RemoteRepo {
    pub remote: String,
    pub host: String,
    pub owner: String,
    pub repo: String,
}

/// Parses `https://host/owner/repo(.git)`, `git@host:owner/repo(.git)` and
/// `ssh://git@host(:port)/owner/repo(.git)`.
pub fn parse_url(url: &str) -> Option<(String, String, String)> {
    let url = url.trim();
    let (host, path) = if let Some(rest) = url.split_once("://").map(|(_, r)| r) {
        let rest = rest.rsplit_once('@').map_or(rest, |(_, r)| r);
        let (host, path) = rest.split_once('/')?;
        (host.split(':').next()?, path)
    } else {
        // scp-like: [user@]host:owner/repo
        let rest = url.rsplit_once('@').map_or(url, |(_, r)| r);
        rest.split_once(':')?
    };
    let path = path.trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let (owner, repo) = path.split_once('/')?;
    if host.is_empty() || owner.is_empty() || repo.is_empty() || repo.contains('/') {
        return None;
    }
    Some((host.to_ascii_lowercase(), owner.to_owned(), repo.to_owned()))
}

/// Every forge repo in `git remote -v` output, one per distinct repo:
/// `upstream` first (fork workflows), then `origin`, then the rest by name.
pub fn all(remote_v: &str) -> Vec<RemoteRepo> {
    let mut remotes: Vec<RemoteRepo> = remote_v
        .lines()
        .filter(|l| l.ends_with("(fetch)"))
        .filter_map(|l| {
            let mut parts = l.split_whitespace();
            let name = parts.next()?;
            let (host, owner, repo) = parse_url(parts.next()?)?;
            Some(RemoteRepo { remote: name.into(), host, owner, repo })
        })
        .collect();
    let rank = |r: &RemoteRepo| match r.remote.as_str() {
        "upstream" => 0,
        "origin" => 1,
        _ => 2,
    };
    remotes.sort_by(|a, b| rank(a).cmp(&rank(b)).then_with(|| a.remote.cmp(&b.remote)));
    let mut seen = Vec::new();
    remotes.retain(|r| {
        let key = (r.host.clone(), r.owner.to_ascii_lowercase(), r.repo.to_ascii_lowercase());
        let fresh = !seen.contains(&key);
        seen.push(key);
        fresh
    });
    remotes
}

/// The project's primary repo: the first of [`all`].
pub fn pick(remote_v: &str) -> Option<RemoteRepo> {
    all(remote_v).into_iter().next()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(host: &str, owner: &str, repo: &str) -> Option<(String, String, String)> {
        Some((host.into(), owner.into(), repo.into()))
    }

    #[test]
    fn url_forms() {
        assert_eq!(parse_url("https://github.com/zed-industries/zed.git"), t("github.com", "zed-industries", "zed"));
        assert_eq!(parse_url("https://github.com/zed-industries/zed"), t("github.com", "zed-industries", "zed"));
        assert_eq!(parse_url("https://user:tok@github.com/a/b.git"), t("github.com", "a", "b"));
        assert_eq!(parse_url("git@github.com:a/b.git"), t("github.com", "a", "b"));
        assert_eq!(parse_url("ssh://git@git.corp.example:2222/team/api.git"), t("git.corp.example", "team", "api"));
        assert_eq!(parse_url("git@github.com:a/b.c.git"), t("github.com", "a", "b.c"));
        assert_eq!(parse_url("/local/path/repo"), None);
    }

    #[test]
    fn prefers_upstream_then_origin() {
        let out = "origin\tgit@github.com:me/zed.git (fetch)\norigin\tgit@github.com:me/zed.git (push)\n\
                   upstream\thttps://github.com/zed-industries/zed.git (fetch)\n";
        let r = pick(out).unwrap();
        assert_eq!((r.remote.as_str(), r.owner.as_str()), ("upstream", "zed-industries"));

        let r = pick("fork\tgit@github.com:x/y.git (fetch)\norigin\tgit@github.com:a/b.git (fetch)\n").unwrap();
        assert_eq!(r.remote, "origin");
        assert_eq!(pick("").map(|r| r.remote), None);
    }

    #[test]
    fn all_remotes_ordered_and_deduped() {
        let out = "alice\tgit@github.com:alice/zed.git (fetch)\n\
                   origin\tgit@github.com:me/zed.git (fetch)\n\
                   mirror\thttps://github.com/me/zed (fetch)\n\
                   upstream\thttps://github.com/zed-industries/zed.git (fetch)\n";
        let names: Vec<_> = all(out).into_iter().map(|r| r.remote).collect();
        assert_eq!(names, ["upstream", "origin", "alice"]);
    }
}
