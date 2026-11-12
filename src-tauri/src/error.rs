use serde::Serialize;

/// Errors returned to the frontend. `kind` lets the UI pick a specific
/// treatment (e.g. Switch account vs Authorize SSO) without parsing messages.
#[derive(Debug, Serialize, specta::Type, thiserror::Error)]
#[serde(tag = "kind", content = "message", rename_all = "snake_case")]
pub enum AppError {
    #[error("{0}")]
    Store(String),
    #[error("{0}")]
    Settings(String),
}

impl From<prism_store::DbError> for AppError {
    fn from(e: prism_store::DbError) -> Self {
        Self::Store(e.to_string())
    }
}

impl From<prism_store::SettingsError> for AppError {
    fn from(e: prism_store::SettingsError) -> Self {
        Self::Settings(e.to_string())
    }
}

pub type AppResult<T> = Result<T, AppError>;
