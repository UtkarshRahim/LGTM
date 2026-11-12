use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, RwLock};

use prism_gh::{Credential, GhError, GitHub, auth};
use prism_repo::Mirrors;
use prism_repo::watch::ProjectWatcher;
use prism_store::{Db, Settings};

use crate::diffs::DiffCache;

pub struct AppState {
    pub db: Db,
    pub db_path: PathBuf,
    pub settings: RwLock<Settings>,
    pub settings_path: PathBuf,
    pub mirrors: Mirrors,
    pub worktrees_dir: PathBuf,
    /// `None` until the first discovery finishes.
    pub creds: tokio::sync::RwLock<Option<Vec<Credential>>>,
    pub watchers: Mutex<HashMap<i32, ProjectWatcher>>,
    pub diffs: DiffCache,
    pub runs: crate::skills::Runs,
}

impl AppState {
    pub async fn credentials(&self) -> Vec<Credential> {
        if let Some(c) = self.creds.read().await.as_ref() {
            return c.clone();
        }
        self.refresh_credentials().await
    }

    pub async fn refresh_credentials(&self) -> Vec<Credential> {
        let mut guard = self.creds.write().await;
        let creds = auth::discover().await;
        *guard = Some(creds.clone());
        creds
    }

    pub fn preferred_account(&self) -> Option<String> {
        self.settings.read().unwrap().github.default_account.clone()
    }

    pub async fn client(&self, host: &str) -> Result<GitHub, GhError> {
        let creds = self.credentials().await;
        let c = auth::select(&creds, host, self.preferred_account().as_deref())
            .ok_or_else(|| GhError::NoAccount { host: host.into() })?;
        Ok(GitHub::new(host, &c.token, &c.account.login))
    }

    pub async fn token(&self, host: &str) -> Result<String, GhError> {
        let creds = self.credentials().await;
        auth::select(&creds, host, self.preferred_account().as_deref())
            .map(|c| c.token.clone())
            .ok_or_else(|| GhError::NoAccount { host: host.into() })
    }
}
