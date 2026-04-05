#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod bundled_packages;
mod components;
mod i18n;
mod state;

use std::sync::Arc;

use kittypaw_llm::claude::ClaudeProvider;
use kittypaw_llm::openai::OpenAiProvider;
use kittypaw_llm::registry::LlmRegistry;
use kittypaw_store::Store;
use state::AppState;

fn main() {
    let data_dir = kittypaw_core::secrets::data_dir()
        .unwrap_or_else(|_| std::path::PathBuf::from(".kittypaw"));

    let db_path = data_dir.join("kittypaw.db");
    if let Some(parent) = db_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let packages_dir = data_dir.join("packages");
    let _ = std::fs::create_dir_all(&packages_dir);
    bundled_packages::install_bundled_packages(&packages_dir);

    let store =
        Store::open(db_path.to_str().unwrap_or("kittypaw.db")).expect("Failed to open database");

    let persisted_key = std::env::var("KITTYPAW_API_KEY").ok().unwrap_or_default();

    let mut llm_registry = LlmRegistry::new();
    if !persisted_key.is_empty() {
        llm_registry.register(
            "claude-sonnet",
            Arc::new(ClaudeProvider::new(
                persisted_key.clone(),
                "claude-sonnet-4-20250514".into(),
                4096,
            )),
        );
    }

    // If no env var, try the local secret store
    if persisted_key.is_empty() {
        if let Ok(Some(key)) = kittypaw_core::secrets::get_secret("settings", "api_key") {
            if !key.is_empty() {
                llm_registry.register(
                    "claude-sonnet",
                    Arc::new(ClaudeProvider::new(
                        key,
                        "claude-sonnet-4-20250514".into(),
                        4096,
                    )),
                );
            }
        }
    }

    let has_cloud_provider = llm_registry.default_provider().is_some();

    let local_url = std::env::var("KITTYPAW_LOCAL_URL").ok();
    let local_model = std::env::var("KITTYPAW_LOCAL_MODEL").ok();
    if let (Some(url), Some(model)) = (local_url, local_model) {
        llm_registry.register(
            "local",
            Arc::new(OpenAiProvider::with_base_url(
                url,
                String::new(),
                model,
                4096,
            )),
        );
        if !has_cloud_provider {
            llm_registry.set_default("local");
        }
    } else {
        // If no env vars, try the local secret store
        if let (Ok(Some(url)), Ok(Some(model))) = (
            kittypaw_core::secrets::get_secret("local_model", "base_url"),
            kittypaw_core::secrets::get_secret("local_model", "model_name"),
        ) {
            if !url.is_empty() && !model.is_empty() {
                llm_registry.register(
                    "local",
                    Arc::new(OpenAiProvider::with_base_url(
                        url,
                        String::new(),
                        model,
                        4096,
                    )),
                );
                if !has_cloud_provider {
                    llm_registry.set_default("local");
                }
            }
        }
    }

    let app_state = AppState::new(store, persisted_key, packages_dir, llm_registry);

    let window = dioxus::desktop::WindowBuilder::new()
        .with_title("KittyPaw")
        .with_inner_size(dioxus::desktop::tao::dpi::LogicalSize::new(1024.0, 720.0))
        .with_min_inner_size(dioxus::desktop::tao::dpi::LogicalSize::new(800.0, 500.0));

    let cfg = dioxus::desktop::Config::new().with_window(window);

    dioxus::LaunchBuilder::desktop()
        .with_cfg(cfg)
        .with_context(app_state)
        .launch(components::app::App);
}
