use serde::{Deserialize, Serialize};

/// Fully qualified pull request reference.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, specta::Type)]
pub struct PrRef {
    pub host: String,
    pub owner: String,
    pub repo: String,
    pub number: u32,
}

/// What the user typed into the search box.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SearchQuery {
    /// A full PR URL or `owner/repo#n`.
    Pr { pr: PrRef },
    /// `#n`: look it up in every added project's repo.
    Number { number: u32 },
    Text { text: String },
    Empty,
}

pub const DEFAULT_HOST: &str = "github.com";

pub fn parse(input: &str) -> SearchQuery {
    let s = input.trim();
    if s.is_empty() {
        return SearchQuery::Empty;
    }
    if let Some(pr) = parse_url(s).or_else(|| parse_short(s)) {
        return SearchQuery::Pr { pr };
    }
    if let Some(n) = s.strip_prefix('#').and_then(parse_number) {
        return SearchQuery::Number { number: n };
    }
    SearchQuery::Text { text: s.to_owned() }
}

fn parse_number(s: &str) -> Option<u32> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok().filter(|&n| n > 0)
}

fn valid_name(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}

/// `https://<host>/<owner>/<repo>/pull/<n>[/files|/commits|#...|?...]`,
/// scheme optional so pasted `github.com/...` works too.
fn parse_url(s: &str) -> Option<PrRef> {
    let rest = s
        .strip_prefix("https://")
        .or_else(|| s.strip_prefix("http://"))
        .unwrap_or(s);
    let rest = rest.split(['?', '#']).next()?;
    let mut parts = rest.split('/');
    let host = parts.next()?;
    let owner = parts.next()?;
    let repo = parts.next()?;
    if parts.next()? != "pull" {
        return None;
    }
    let number = parse_number(parts.next()?)?;
    if !host.contains('.') && host != "localhost" || !valid_name(owner) || !valid_name(repo) {
        return None;
    }
    Some(PrRef {
        host: host.to_ascii_lowercase(),
        owner: owner.into(),
        repo: repo.into(),
        number,
    })
}

/// `owner/repo#n`
fn parse_short(s: &str) -> Option<PrRef> {
    let (slug, n) = s.split_once('#')?;
    let (owner, repo) = slug.split_once('/')?;
    if !valid_name(owner) || !valid_name(repo) {
        return None;
    }
    Some(PrRef {
        host: DEFAULT_HOST.into(),
        owner: owner.into(),
        repo: repo.into(),
        number: parse_number(n)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr(host: &str, owner: &str, repo: &str, number: u32) -> SearchQuery {
        SearchQuery::Pr { pr: PrRef { host: host.into(), owner: owner.into(), repo: repo.into(), number } }
    }

    #[test]
    fn urls() {
        let want = pr("github.com", "tauri-apps", "tauri", 1284);
        assert_eq!(parse("https://github.com/tauri-apps/tauri/pull/1284"), want);
        assert_eq!(parse("  github.com/tauri-apps/tauri/pull/1284/files  "), want);
        assert_eq!(parse("https://github.com/tauri-apps/tauri/pull/1284#discussion_r1"), want);
        assert_eq!(parse("https://github.com/tauri-apps/tauri/pull/1284?w=1"), want);
        assert_eq!(parse("https://GitHub.com/tauri-apps/tauri/pull/1284/commits/abc"), want);
    }

    #[test]
    fn ghes_url() {
        assert_eq!(parse("https://git.corp.example/team/api/pull/7"), pr("git.corp.example", "team", "api", 7));
    }

    #[test]
    fn short_forms() {
        assert_eq!(parse("zed-industries/zed#42"), pr("github.com", "zed-industries", "zed", 42));
        assert_eq!(parse("#1284"), SearchQuery::Number { number: 1284 });
    }

    #[test]
    fn text_and_edge_cases() {
        assert_eq!(parse(""), SearchQuery::Empty);
        assert_eq!(parse("   "), SearchQuery::Empty);
        assert_eq!(parse("fix flaky test"), SearchQuery::Text { text: "fix flaky test".into() });
        assert_eq!(parse("#0"), SearchQuery::Text { text: "#0".into() });
        assert_eq!(parse("#12a"), SearchQuery::Text { text: "#12a".into() });
        assert_eq!(parse("https://github.com/o/r/issues/5"), SearchQuery::Text { text: "https://github.com/o/r/issues/5".into() });
        assert_eq!(parse("a b/c#5"), SearchQuery::Text { text: "a b/c#5".into() });
    }
}
