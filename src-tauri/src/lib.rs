mod accounts;
mod commands;
mod diffs;
mod error;
mod projects;
mod prs;
mod review;
mod skills;
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
            review::pr_local_review,
            review::add_draft,
            review::update_draft,
            review::delete_draft,
            review::set_viewed,
            review::submit_review,
            review::set_thread_resolved,
            review::pr_since_review,
            skills::list_skills,
            skills::set_skill_prefs,
            skills::pr_skill_state,
            skills::skill_run_log,
            skills::run_skill,
            skills::cancel_skill_run,
            skills::running_reviews,
            skills::accept_finding,
            skills::dismiss_finding,
        ])
        .constant("HIGHLIGHT_CLASSES", prism_syntax::CLASSES)
        .events(collect_events![projects::ProjectStatusChanged, prs::PrUpdated, skills::SkillRunEvent])
}

fn export_bindings(builder: &Builder<tauri::Wry>) {
    builder
        .export(
            specta_typescript::Typescript::default().header("// @ts-nocheck\n"),
            BINDINGS_PATH,
        )
        .expect("failed to export TypeScript bindings");
}

const QUIT_ID: &str = "quit";

/// The default menu, with Quit routed through `request_quit` so it can ask
/// before stopping running auto reviews.
fn app_menu(app: &tauri::AppHandle) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    use tauri::menu::{Menu, MenuItem, MenuItemKind};
    let menu = Menu::default(app)?;
    let quit = MenuItem::with_id(app, QUIT_ID, "Quit LGTM", true, Some("CmdOrCtrl+Q"))?;
    for item in menu.items()? {
        let MenuItemKind::Submenu(sub) = item else { continue };
        for (i, entry) in sub.items()?.into_iter().enumerate() {
            if let MenuItemKind::Predefined(p) = &entry
                && p.text()?.starts_with("Quit")
            {
                sub.remove_at(i)?;
                sub.insert(&quit, i)?;
                return Ok(menu);
            }
        }
    }
    tracing::warn!("no Quit item in the default menu; quitting won't ask about running reviews");
    Ok(menu)
}

pub fn run() {
    let builder = specta_builder();
    #[cfg(debug_assertions)]
    export_bindings(&builder);

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(builder.invoke_handler())
        .menu(app_menu)
        .on_menu_event(|app, e| {
            if e.id() == QUIT_ID {
                skills::request_quit(app);
            }
        })
        .on_window_event(|window, e| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = e
                && !window.state::<AppState>().runs.running().is_empty()
            {
                api.prevent_close();
                skills::request_quit(window.app_handle());
            }
        })
        .setup(move |app| {
            builder.mount_events(app);
            let paths = app.path();
            prism_env::init(paths.app_cache_dir()?.join("shell-env.json"));
            let data_dir = paths.app_data_dir()?;
            let db_path = data_dir.join("prism.db");
            let settings_path = paths.app_config_dir()?.join("settings.toml");
            let db = Db::open(&db_path)?;
            db.fail_unfinished_runs()?;
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
                runs: Default::default(),
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
        .expect("error while running LGTM");
}

#[cfg(test)]
mod tests {
    #[test]
    fn export_typescript_bindings() {
        super::export_bindings(&super::specta_builder());
    }
}
