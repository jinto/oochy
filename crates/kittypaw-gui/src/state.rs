use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use kittypaw_store::Store;
use kittypaw_workspace::WorkspaceManager;

#[derive(Clone)]
pub struct AppState {
    #[allow(dead_code)]
    pub store: Arc<Mutex<Store>>,
    pub api_key: Arc<Mutex<String>>,
    #[allow(dead_code)]
    pub workspace_manager: Arc<Mutex<WorkspaceManager>>,
    pub packages_dir: PathBuf,
}

impl AppState {
    pub fn new(store: Store, api_key: String, packages_dir: PathBuf) -> Self {
        Self {
            store: Arc::new(Mutex::new(store)),
            api_key: Arc::new(Mutex::new(api_key)),
            workspace_manager: Arc::new(Mutex::new(WorkspaceManager::new())),
            packages_dir,
        }
    }
}
