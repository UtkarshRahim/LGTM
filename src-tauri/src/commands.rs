use prism_env::ShellEnvStatus;
use prism_store::Settings;
use serde::Serialize;
use tauri::State;

use crate::error::AppResult;
use crate::state::AppState;

#[derive(Serialize, specta::Type)]
pub struct AppInfo {
    pub version: String,
    pub db_path: String,
    pub settings_path: String,
    pub schema_version: i32,
    pub shell_env: ShellEnvStatus,
}

#[tauri::command]
#[specta::specta]
pub fn app_info(state: State<'_, AppState>) -> AppResult<AppInfo> {
    Ok(AppInfo {
        version: env!("CARGO_PKG_VERSION").into(),
        db_path: state.db_path.display().to_string(),
        settings_path: state.settings_path.display().to_string(),
        schema_version: state.db.schema_version()? as i32,
        shell_env: prism_env::status(),
    })
}

#[tauri::command]
#[specta::specta]
pub fn get_settings(state: State<'_, AppState>) -> Settings {
    state.settings.read().unwrap().clone()
}

#[tauri::command]
#[specta::specta]
pub fn save_settings(state: State<'_, AppState>, settings: Settings) -> AppResult<()> {
    settings.save(&state.settings_path)?;
    *state.settings.write().unwrap() = settings;
    Ok(())
}
