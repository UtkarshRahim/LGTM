//! Token discovery, in the order shown in Settings: environment variables,
//! then every account the `gh` CLI knows about.

use std::fmt;

use futures_util::future::join_all;
use serde::{Deserialize, Serialize};

use crate::client::GitHub;
use crate::query::DEFAULT_HOST;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TokenSource {
    Env { var: String },
    GhCli,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub struct Account {
    pub host: String,
    pub login: String,
    pub source: TokenSource,
    /// The account `gh` itself would use for this host.
    pub gh_active: bool,
}

impl Account {
    /// `host/login`, the form stored in settings.
    pub fn key(&self) -> String {
        format!("{}/{}", self.host, self.login)
    }
}

#[derive(Clone)]
pub struct Credential {
    pub account: Account,
    pub token: String,
}

impl fmt::Debug for Credential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credential").field("account", &self.account).field("token", &"***").finish()
    }
}

/// Finds every usable credential. Env tokens come first; their login is
/// resolved with one `/user` call. Unusable sources are skipped.
pub async fn discover() -> Vec<Credential> {
    let (env, gh) = tokio::join!(from_env(), from_gh_cli());
    let mut creds = env;
    for c in gh {
        if !creds.iter().any(|e| e.account.key() == c.account.key()) {
            creds.push(c);
        }
    }
    creds
}

async fn from_env() -> Vec<Credential> {
    for var in ["GH_TOKEN", "GITHUB_TOKEN"] {
        let Some(token) = prism_env::var(var) else { continue };
        match GitHub::new(DEFAULT_HOST, &token, "env").viewer_login().await {
            Ok(login) => {
                return vec![Credential {
                    account: Account {
                        host: DEFAULT_HOST.into(),
                        login,
                        source: TokenSource::Env { var: var.into() },
                        gh_active: false,
                    },
                    token,
                }];
            }
            Err(e) => tracing::warn!(var, error = %e, "ignoring env token"),
        }
    }
    Vec::new()
}

#[derive(Deserialize)]
struct GhStatus {
    hosts: std::collections::HashMap<String, Vec<GhHostAccount>>,
}

#[derive(Deserialize)]
struct GhHostAccount {
    state: String,
    active: bool,
    host: String,
    login: String,
}

async fn gh_output(args: &[&str]) -> Option<String> {
    let out = prism_env::tokio_command("gh").await.args(args).output().await.ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8(out.stdout).ok()
}

async fn from_gh_cli() -> Vec<Credential> {
    let Some(json) = gh_output(&["auth", "status", "--json", "hosts"]).await else {
        return Vec::new();
    };
    let status: GhStatus = match serde_json::from_str(&json) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, "unexpected `gh auth status` output");
            return Vec::new();
        }
    };
    let accounts: Vec<GhHostAccount> =
        status.hosts.into_values().flatten().filter(|a| a.state == "success").collect();

    let tokens = join_all(accounts.iter().map(|a| async move {
        gh_output(&["auth", "token", "--hostname", &a.host, "--user", &a.login]).await
    }))
    .await;

    let mut creds: Vec<Credential> = accounts
        .into_iter()
        .zip(tokens)
        .filter_map(|(a, token)| {
            Some(Credential {
                token: token?.trim().to_owned(),
                account: Account { host: a.host, login: a.login, source: TokenSource::GhCli, gh_active: a.active },
            })
        })
        .collect();
    creds.sort_by(|a, b| b.account.gh_active.cmp(&a.account.gh_active));
    creds
}

/// Picks the credential for `host`: the preferred `host/login` if present,
/// otherwise the first discovered.
pub fn select<'a>(creds: &'a [Credential], host: &str, preferred: Option<&str>) -> Option<&'a Credential> {
    let for_host = || creds.iter().filter(move |c| c.account.host == host);
    preferred
        .and_then(|key| for_host().find(|c| c.account.key() == key))
        .or_else(|| for_host().next())
}
