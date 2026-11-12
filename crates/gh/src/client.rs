use std::sync::{Arc, LazyLock};
use std::time::Duration;

use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderMap, RETRY_AFTER, USER_AGENT};
use reqwest::{RequestBuilder, Response, StatusCode};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::sync::Semaphore;

use crate::error::{GhError, GhResult};

/// One queue for every request to every host. GitHub's secondary limits
/// allow 100 concurrent requests, but the budget is shared with `gh` and other
/// tools, so stay well under.
static PERMITS: LazyLock<Arc<Semaphore>> = LazyLock::new(|| Arc::new(Semaphore::new(6)));
static HTTP: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .pool_idle_timeout(Duration::from_secs(90))
        .build()
        .expect("building HTTP client")
});

const MAX_ATTEMPTS: u32 = 4;
const MAX_RETRY_AFTER: Duration = Duration::from_secs(60);

#[derive(Clone)]
pub struct GitHub {
    host: String,
    login: String,
    rest_base: String,
    graphql_url: String,
    auth: String,
}

#[derive(Deserialize)]
struct GraphqlResponse<T> {
    data: Option<T>,
    #[serde(default)]
    errors: Vec<GraphqlError>,
}

#[derive(Debug, Deserialize)]
pub struct GraphqlError {
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub message: String,
    #[serde(default)]
    pub path: Vec<Value>,
}

/// Data plus per-field errors, for batched queries where some aliases can
/// fail (e.g. one repo not visible) while others succeed.
pub struct Partial<T> {
    pub data: Option<T>,
    pub errors: Vec<GraphqlError>,
}

impl GitHub {
    pub fn new(host: &str, token: &str, login: &str) -> Self {
        let (rest_base, graphql_url) = if host == "github.com" {
            ("https://api.github.com".to_owned(), "https://api.github.com/graphql".to_owned())
        } else {
            (format!("https://{host}/api/v3"), format!("https://{host}/api/graphql"))
        };
        Self { host: host.into(), login: login.into(), rest_base, graphql_url, auth: format!("Bearer {token}") }
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn login(&self) -> &str {
        &self.login
    }

    fn request(&self, method: reqwest::Method, url: &str) -> RequestBuilder {
        HTTP.request(method, url)
            .header(AUTHORIZATION, &self.auth)
            .header(USER_AGENT, "Prism")
            .header(ACCEPT, "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
    }

    pub async fn viewer_login(&self) -> GhResult<String> {
        #[derive(Deserialize)]
        struct User {
            login: String,
        }
        Ok(self.rest_get::<User>("/user").await?.login)
    }

    pub async fn rest_get<T: DeserializeOwned>(&self, path: &str) -> GhResult<T> {
        let url = format!("{}{path}", self.rest_base);
        let resp = self.send(|| self.request(reqwest::Method::GET, &url), path).await?;
        Ok(resp.json().await?)
    }

    /// Runs a GraphQL query; any error in the response fails the call.
    pub async fn graphql<T: DeserializeOwned>(&self, query: &str, variables: Value, what: &str) -> GhResult<T> {
        let Partial { data, errors } = self.graphql_partial(query, variables, what).await?;
        if let Some(e) = errors.into_iter().next() {
            return Err(self.classify_graphql(&e, what));
        }
        data.ok_or_else(|| GhError::api("GraphQL response had no data"))
    }

    pub async fn graphql_partial<T: DeserializeOwned>(
        &self,
        query: &str,
        variables: Value,
        what: &str,
    ) -> GhResult<Partial<T>> {
        let body = serde_json::json!({ "query": query, "variables": variables });
        let resp = self
            .send(|| self.request(reqwest::Method::POST, &self.graphql_url).json(&body), what)
            .await?;
        let parsed: GraphqlResponse<T> = resp.json().await?;
        if parsed.data.is_none() {
            if let Some(e) = parsed.errors.first() {
                return Err(self.classify_graphql(e, what));
            }
        }
        Ok(Partial { data: parsed.data, errors: parsed.errors })
    }

    pub fn classify_graphql(&self, e: &GraphqlError, what: &str) -> GhError {
        match e.kind.as_deref() {
            Some("NOT_FOUND") => self.not_visible(what),
            Some("RATE_LIMITED") => GhError::RateLimited { reset_at: None },
            _ if e.message.contains("SAML") => self.sso_required(what, None),
            _ => GhError::api(e.message.clone()),
        }
    }

    fn not_visible(&self, what: &str) -> GhError {
        GhError::NotVisible { what: what.into(), login: self.login.clone() }
    }

    /// `what` starts with `owner/repo`; the org is its owner.
    fn sso_required(&self, what: &str, url: Option<String>) -> GhError {
        let org = what.split(['/', '#']).next().unwrap_or(what).to_owned();
        let url = url.unwrap_or_else(|| format!("https://{}/orgs/{org}/sso", self.host));
        GhError::SsoRequired { org, url }
    }

    async fn send(&self, build: impl Fn() -> RequestBuilder, what: &str) -> GhResult<Response> {
        let mut attempt = 0;
        loop {
            attempt += 1;
            let result = {
                let _permit = PERMITS.acquire().await.expect("semaphore closed");
                build().send().await
            };
            let resp = match result {
                Ok(r) => r,
                Err(e) if attempt < MAX_ATTEMPTS && (e.is_connect() || e.is_timeout()) => {
                    tokio::time::sleep(backoff(attempt)).await;
                    continue;
                }
                Err(e) => return Err(e.into()),
            };

            let status = resp.status();
            if status.is_success() {
                return Ok(resp);
            }
            let headers = resp.headers().clone();
            let body = resp.text().await.unwrap_or_default();
            match self.classify_status(status, &headers, &body, what) {
                Retry::After(wait) if attempt < MAX_ATTEMPTS => {
                    tracing::debug!(%status, ?wait, attempt, "retrying GitHub request");
                    tokio::time::sleep(wait).await;
                }
                Retry::After(_) => return Err(GhError::api(format!("{status}: {}", truncate(&body)))),
                Retry::No(e) => return Err(e),
            }
        }
    }

    fn classify_status(&self, status: StatusCode, headers: &HeaderMap, body: &str, what: &str) -> Retry {
        let header = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());

        if let Some(sso) = header("x-github-sso") {
            let url = sso.split("url=").nth(1).map(|u| u.trim().to_owned());
            return Retry::No(self.sso_required(what, url));
        }
        match status {
            StatusCode::UNAUTHORIZED => Retry::No(GhError::Unauthorized { host: self.host.clone() }),
            StatusCode::NOT_FOUND => Retry::No(self.not_visible(what)),
            StatusCode::FORBIDDEN | StatusCode::TOO_MANY_REQUESTS => {
                if let Some(secs) = header(RETRY_AFTER.as_str()).and_then(|s| s.parse::<u64>().ok()) {
                    let wait = Duration::from_secs(secs);
                    if wait <= MAX_RETRY_AFTER {
                        return Retry::After(wait);
                    }
                }
                if header("x-ratelimit-remaining") == Some("0") {
                    let reset_at = header("x-ratelimit-reset").and_then(|s| s.parse().ok());
                    return Retry::No(GhError::RateLimited { reset_at });
                }
                if body.contains("secondary rate limit") {
                    return Retry::After(Duration::from_secs(60).min(backoff(4)));
                }
                if body.contains("SAML") {
                    return Retry::No(self.sso_required(what, None));
                }
                Retry::No(GhError::api(format!("{status}: {}", truncate(body))))
            }
            s if s.is_server_error() => Retry::After(Duration::ZERO),
            _ => Retry::No(GhError::api(format!("{status}: {}", truncate(body)))),
        }
        .with_backoff(status)
    }
}

enum Retry {
    After(Duration),
    No(GhError),
}

impl Retry {
    /// Server errors carry no hint; replace the zero wait with jittered backoff.
    fn with_backoff(self, status: StatusCode) -> Self {
        match self {
            Retry::After(d) if d.is_zero() && status.is_server_error() => Retry::After(backoff(2)),
            other => other,
        }
    }
}

/// Exponential backoff with full jitter: random in [0, 500ms * 2^attempt).
fn backoff(attempt: u32) -> Duration {
    let cap_ms = 500u64 << attempt.min(6);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    Duration::from_millis(nanos % cap_ms)
}

fn truncate(s: &str) -> &str {
    let end = s.char_indices().nth(200).map(|(i, _)| i).unwrap_or(s.len());
    &s[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gh() -> GitHub {
        GitHub::new("github.com", "t", "octocat")
    }

    fn classify(status: u16, headers: &[(&'static str, &str)], body: &str) -> Retry {
        let mut map = HeaderMap::new();
        for (k, v) in headers {
            map.insert(*k, v.parse().unwrap());
        }
        gh().classify_status(StatusCode::from_u16(status).unwrap(), &map, body, "acme/api#7")
    }

    #[test]
    fn not_found_is_not_visible() {
        let Retry::No(GhError::NotVisible { what, login }) = classify(404, &[], "") else { panic!() };
        assert_eq!((what.as_str(), login.as_str()), ("acme/api#7", "octocat"));
    }

    #[test]
    fn sso_header_carries_url() {
        let r = classify(403, &[("x-github-sso", "required; url=https://github.com/orgs/acme/sso?authorization_request=x")], "");
        let Retry::No(GhError::SsoRequired { org, url }) = r else { panic!() };
        assert_eq!(org, "acme");
        assert_eq!(url, "https://github.com/orgs/acme/sso?authorization_request=x");
    }

    #[test]
    fn rate_limits() {
        assert!(matches!(classify(429, &[("retry-after", "3")], ""), Retry::After(d) if d == Duration::from_secs(3)));
        assert!(matches!(
            classify(403, &[("x-ratelimit-remaining", "0"), ("x-ratelimit-reset", "1700000000")], ""),
            Retry::No(GhError::RateLimited { reset_at: Some(1_700_000_000) })
        ));
        assert!(matches!(classify(403, &[], "You have exceeded a secondary rate limit"), Retry::After(_)));
        assert!(matches!(classify(502, &[], ""), Retry::After(_)));
    }

    #[test]
    fn graphql_errors() {
        let e = |kind: Option<&str>, msg: &str| GraphqlError { kind: kind.map(Into::into), message: msg.into(), path: vec![] };
        assert!(matches!(gh().classify_graphql(&e(Some("NOT_FOUND"), "x"), "acme/api"), GhError::NotVisible { .. }));
        let GhError::SsoRequired { org, url } = gh().classify_graphql(
            &e(Some("FORBIDDEN"), "Resource protected by organization SAML enforcement."),
            "acme/api#7",
        ) else {
            panic!()
        };
        assert_eq!(org, "acme");
        assert_eq!(url, "https://github.com/orgs/acme/sso");
    }

    #[test]
    fn ghes_endpoints() {
        let g = GitHub::new("git.corp.example", "t", "me");
        assert_eq!(g.rest_base, "https://git.corp.example/api/v3");
        assert_eq!(g.graphql_url, "https://git.corp.example/api/graphql");
    }
}
