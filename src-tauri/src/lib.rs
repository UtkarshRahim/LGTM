mod commands;
mod error;
mod state;

use std::sync::RwLock;

use prism_store::{Db, Settings};
use tauri::Manager;
use tauri_specta::{Builder, ErrorHandlingMode, collect_commands};

use crate::state::AppState;

const BINDINGS_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../src/bindings.ts");

fn specta_builder() -> Builder<tauri::Wry> {
    Builder::<tauri::Wry>::new()
        .error_handling(ErrorHandlingMode::Result)
        .commands(collect_commands![
            commands::app_info,
            commands::get_settings,
            commands::save_settings,
        ])
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
        .invoke_handler(builder.invoke_handler())
        .setup(move |app| {
            builder.mount_events(app);
            let paths = app.path();
            prism_env::init(paths.app_cache_dir()?.join("shell-env.json"));
            let db_path = paths.app_data_dir()?.join("prism.db");
            let settings_path = paths.app_config_dir()?.join("settings.toml");
            let db = Db::open(&db_path)?;
            let settings = Settings::load_or_init(&settings_path)?;
            tracing::info!(db = %db_path.display(), settings = %settings_path.display(), "store ready");
            app.manage(AppState {
                db,
                db_path,
                settings: RwLock::new(settings),
                settings_path,
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
