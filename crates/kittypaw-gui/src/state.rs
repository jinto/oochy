use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use kittypaw_llm::registry::LlmRegistry;
use kittypaw_store::Store;

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<Mutex<Store>>,
    pub api_key: Arc<Mutex<String>>,
    pub packages_dir: PathBuf,
    pub llm_registry: Arc<Mutex<LlmRegistry>>,
}

impl AppState {
    pub fn new(
        store: Store,
        api_key: String,
        packages_dir: PathBuf,
        llm_registry: LlmRegistry,
    ) -> Self {
        Self {
            store: Arc::new(Mutex::new(store)),
            api_key: Arc::new(Mutex::new(api_key)),
            packages_dir,
            llm_registry: Arc::new(Mutex::new(llm_registry)),
        }
    }
}
