mod accounts;
mod commands;
mod diffs;
mod error;
mod projects;
mod prs;
mod state;

use std::collections::HashMap;
use std::sync::{Mutex, RwLock};

use prism_repo::Mirrors;
use prism_store::{Db, Settings};
use tauri::Manager;
use tauri_specta::{Builder, ErrorHandlingMode, collect_commands, collect_events};

use crate::state::AppState;

const BINDINGS_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../src/bindings.ts");

fn specta_builder() -> Builder<tauri::Wry> {
    Builder::<tauri::Wry>::new()
        .error_handling(ErrorHandlingMode::Result)
        .commands(collect_commands![
            commands::app_info,
            commands::get_settings,
            commands::save_settings,
            accounts::accounts,
            accounts::refresh_accounts,
            accounts::set_default_account,
            projects::list_projects,
            projects::add_project,
            projects::remove_project,
            projects::project_status,
            projects::project_pr_counts,
            projects::project_prs,
            projects::checkout_pr,
            prs::search,
            prs::open_pr,
            prs::pr_sync_local,
            prs::pr_hunks,
            prs::pr_threads,
            prs::needs_you,
        ])
        .constant("HIGHLIGHT_CLASSES", prism_syntax::CLASSES)
        .events(collect_events![projects::ProjectStatusChanged, prs::PrUpdated])
}

fn export_bindings(builder: &Builder<tauri::Wry>) {
    builder
        .export(
            specta_typescript::Typescript::default().header("// @ts-nocheck\n"),
            BINDINGS_PATH,
        )
        .expect("failed to export TypeScript bindings");
}

pub fn run() {
    let builder = specta_builder();
    #[cfg(debug_assertions)]
    export_bindings(&builder);

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(builder.invoke_handler())
        .setup(move |app| {
            builder.mount_events(app);
            let paths = app.path();
            prism_env::init(paths.app_cache_dir()?.join("shell-env.json"));
            let data_dir = paths.app_data_dir()?;
            let db_path = data_dir.join("prism.db");
            let settings_path = paths.app_config_dir()?.join("settings.toml");
            let db = Db::open(&db_path)?;
            let settings = Settings::load_or_init(&settings_path)?;
            tracing::info!(db = %db_path.display(), settings = %settings_path.display(), "store ready");
            app.manage(AppState {
                db,
                db_path,
                settings: RwLock::new(settings),
                settings_path,
                mirrors: Mirrors::new(data_dir.join("mirrors")),
                worktrees_dir: data_dir.join("worktrees"),
                creds: tokio::sync::RwLock::new(None),
                watchers: Mutex::new(HashMap::new()),
                diffs: Default::default(),
            });

            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let creds = handle.state::<AppState>().credentials().await;
                tracing::info!(accounts = creds.len(), "accounts discovered");
                projects::watch_all(&handle).await;
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Prism");
}

#[cfg(test)]
mod tests {
    #[test]
    fn export_typescript_bindings() {
        super::export_bindings(&super::specta_builder());
    }
}
