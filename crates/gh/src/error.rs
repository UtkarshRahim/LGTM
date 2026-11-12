use serde::Serialize;

#[derive(Debug, Clone, Serialize, specta::Type, thiserror::Error)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GhError {
    /// GitHub says "not found" both for missing PRs and for private repos the
    /// account can't see, so we can't tell them apart.
    #[error("{what} isn't visible to {login}")]
    NotVisible { what: String, login: String },
    #[error("{org} requires SAML SSO authorization")]
    SsoRequired { org: String, url: String },
    #[error("the token for {host} was rejected")]
    Unauthorized { host: String },
    #[error("no GitHub account is signed in for {host}")]
    NoAccount { host: String },
    #[error("GitHub rate limit reached")]
    RateLimited { reset_at: Option<u32> },
    #[error("GitHub API error: {message}")]
    Api { message: String },
    #[error("network error: {message}")]
    Network { message: String },
}

impl GhError {
    pub fn api(message: impl Into<String>) -> Self {
        Self::Api { message: message.into() }
    }
}

impl From<reqwest::Error> for GhError {
    fn from(e: reqwest::Error) -> Self {
        Self::Network { message: e.to_string() }
    }
}

pub type GhResult<T> = Result<T, GhError>;
