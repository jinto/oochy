use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use dioxus::prelude::*;
use kittypaw_llm::registry::LlmRegistry;
use kittypaw_store::Store;

/// A permission request awaiting user decision, paired with the channel to
/// deliver the verdict back to the requester.
pub struct PendingPermission {
    pub request: kittypaw_core::permission::PermissionRequest,
    pub responder: tokio::sync::oneshot::Sender<kittypaw_core::permission::PermissionDecision>,
}

/// Reactive queue of pending permission requests.
///
/// Provided as a separate Dioxus context (via `use_context_provider` in the
/// root component) because `Signal` requires a live reactive scope that only
/// exists inside the component tree.
#[derive(Clone, Copy)]
pub struct PermissionQueue {
    pub requests: Signal<Vec<PendingPermission>>,
}

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<tokio::sync::Mutex<Store>>,
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
            store: Arc::new(tokio::sync::Mutex::new(store)),
            api_key: Arc::new(Mutex::new(api_key)),
            packages_dir,
            llm_registry: Arc::new(Mutex::new(llm_registry)),
        }
    }
}
