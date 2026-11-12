use prism_gh::GhError;
use serde::Serialize;

/// Errors returned to the frontend. `kind` (and `error.kind` for GitHub)
/// lets the UI pick a specific treatment, e.g. Switch account vs Authorize SSO.
#[derive(Debug, Serialize, specta::Type, thiserror::Error)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AppError {
    #[error("{error}")]
    Github { error: GhError },
    #[error("{message}")]
    Git { message: String },
    #[error("{message}")]
    Store { message: String },
    #[error("{message}")]
    Settings { message: String },
    #[error("{message}")]
    Invalid { message: String },
    #[error("{message}")]
    Agent { message: String },
}

impl AppError {
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::Invalid { message: message.into() }
    }
}

impl From<GhError> for AppError {
    fn from(error: GhError) -> Self {
        Self::Github { error }
    }
}

impl From<prism_repo::GitError> for AppError {
    fn from(e: prism_repo::GitError) -> Self {
        Self::Git { message: e.to_string() }
    }
}

impl From<prism_store::DbError> for AppError {
    fn from(e: prism_store::DbError) -> Self {
        Self::Store { message: e.to_string() }
    }
}

impl From<prism_store::SettingsError> for AppError {
    fn from(e: prism_store::SettingsError) -> Self {
        Self::Settings { message: e.to_string() }
    }
}

impl From<serde_json::Error> for AppError {
    fn from(e: serde_json::Error) -> Self {
        Self::Store { message: format!("corrupt cache entry: {e}") }
    }
}

pub type AppResult<T> = Result<T, AppError>;
