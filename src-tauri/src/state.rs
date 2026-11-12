use std::path::PathBuf;
use std::sync::RwLock;

use prism_store::{Db, Settings};

pub struct AppState {
    pub db: Db,
    pub db_path: PathBuf,
    pub settings: RwLock<Settings>,
    pub settings_path: PathBuf,
}
