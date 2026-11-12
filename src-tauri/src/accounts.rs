use prism_gh::{Account, auth};
use serde::Serialize;
use tauri::State;

use crate::error::AppResult;
use crate::state::AppState;

#[derive(Serialize, specta::Type)]
pub struct Accounts {
    pub accounts: Vec<Account>,
    /// `host/login` used for github.com.
    pub active: Option<String>,
}

async fn view(state: &AppState, creds: Vec<prism_gh::Credential>) -> Accounts {
    let active = auth::select(&creds, "github.com", state.preferred_account().as_deref()).map(|c| c.account.key());
    Accounts { accounts: creds.into_iter().map(|c| c.account).collect(), active }
}

#[tauri::command]
#[specta::specta]
pub async fn accounts(state: State<'_, AppState>) -> AppResult<Accounts> {
    let creds = state.credentials().await;
    Ok(view(&state, creds).await)
}

#[tauri::command]
#[specta::specta]
pub async fn refresh_accounts(state: State<'_, AppState>) -> AppResult<Accounts> {
    let creds = state.refresh_credentials().await;
    Ok(view(&state, creds).await)
}

#[tauri::command]
#[specta::specta]
pub async fn set_default_account(state: State<'_, AppState>, key: String) -> AppResult<Accounts> {
    let settings = {
        let mut s = state.settings.write().unwrap();
        s.github.default_account = Some(key);
        s.clone()
    };
    settings.save(&state.settings_path)?;
    let creds = state.credentials().await;
    Ok(view(&state, creds).await)
}
