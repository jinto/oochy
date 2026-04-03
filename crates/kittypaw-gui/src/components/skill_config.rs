use std::collections::HashMap;
use std::sync::Arc;

use crate::state::AppState;
use dioxus::prelude::*;
use kittypaw_core::config::SandboxConfig;
use kittypaw_core::package::{ConfigFieldType, SkillPackage};
use kittypaw_core::package_manager::PackageManager;

#[component]
pub fn SkillConfig(package: SkillPackage, on_close: EventHandler) -> Element {
    let app_state = use_context::<AppState>();
    let mut config_values = use_signal::<HashMap<String, String>>(HashMap::new);
    let mut saved = use_signal(|| false);
    let mut test_output = use_signal(String::new);
    let mut testing = use_signal(|| false);
    let pkg_id = package.meta.id.clone();

    // Load config
    {
        let app_state = app_state.clone();
        let pkg_id = pkg_id.clone();
        use_effect(move || {
            let mgr = PackageManager::new(app_state.packages_dir.clone());
            if let Ok(cfg) = mgr.get_config_with_defaults(&pkg_id) {
                config_values.set(cfg);
            }
        });
    }

    let pkg_id_save = package.meta.id.clone();
    let app_state_save = app_state.clone();

    let pkg_id_test = package.meta.id.clone();
    let app_state_test = app_state.clone();

    rsx! {
        div {
            style: "position: fixed; inset: 0; background: rgba(0,0,0,0.4); display: flex; align-items: center; justify-content: center; z-index: 100;",
            onclick: move |_| on_close.call(()),

            div {
                style: "background: #fff; border-radius: 16px; padding: 28px; width: 520px; max-width: 94vw; max-height: 90vh; overflow-y: auto; box-shadow: 0 20px 60px rgba(0,0,0,0.2);",
                onclick: move |e| e.stop_propagation(),

                div { style: "display: flex; justify-content: space-between; align-items: center; margin-bottom: 20px;",
                    h2 { style: "font-size: 18px; font-weight: 600; color: #1e293b; margin: 0;",
                        "{package.meta.name}"
                    }
                    button {
                        style: "background: none; border: none; font-size: 18px; color: #94a3b8; cursor: pointer;",
                        onclick: move |_| on_close.call(()),
                        "X"
                    }
                }

                p { style: "font-size: 13px; color: #64748b; margin-bottom: 20px;",
                    "{package.meta.description}"
                }

                // Config fields
                for field in package.config_schema.iter() {
                    div { style: "margin-bottom: 16px;",
                        label { style: "display: block; font-size: 13px; font-weight: 600; color: #374151; margin-bottom: 4px;",
                            "{field.label}"
                            if field.required {
                                span { style: "color: #ef4444; margin-left: 2px;", " *" }
                            }
                        }
                        if let Some(hint) = &field.hint {
                            p { style: "font-size: 11px; color: #9ca3af; margin: 0 0 6px;", "{hint}" }
                        }
                        if field.key == "telegram_token" || field.key == "chat_id" {
                            if let Ok(Some(_)) = kittypaw_core::secrets::get_secret("channels", &field.key) {
                                p { style: "font-size: 11px; color: #86EFAC; margin: 0 0 6px;",
                                    "Settings에서 자동 입력됨"
                                }
                            }
                        }
                        {
                            let key = field.key.clone();
                            let current_val = config_values.read().get(&key).cloned().unwrap_or_default();
                            let is_secret = matches!(field.field_type, ConfigFieldType::Secret);
                            rsx! {
                                input {
                                    style: "width: 100%; padding: 8px 12px; border: 1px solid #d1d5db; border-radius: 8px; font-size: 13px; outline: none; box-sizing: border-box;",
                                    r#type: if is_secret { "password" } else { "text" },
                                    value: "{current_val}",
                                    oninput: move |e| {
                                        config_values.write().insert(key.clone(), e.value());
                                    },
                                }
                            }
                        }
                    }
                }

                // Model selector
                {
                    let model_names: Vec<String> = app_state
                        .llm_registry
                        .lock()
                        .map(|r| r.list())
                        .unwrap_or_default();
                    let current_model = config_values.read().get("_model").cloned().unwrap_or_default();
                    if !model_names.is_empty() {
                        rsx! {
                            div { style: "margin-bottom: 16px;",
                                label { style: "display: block; font-size: 13px; font-weight: 600; color: #374151; margin-bottom: 4px;",
                                    "Model"
                                }
                                p { style: "font-size: 11px; color: #9ca3af; margin: 0 0 6px;",
                                    "Override the LLM model used for this skill's Llm.generate calls."
                                }
                                select {
                                    style: "width: 100%; padding: 8px 12px; border: 1px solid #d1d5db; border-radius: 8px; font-size: 13px; outline: none; box-sizing: border-box; background: #fff;",
                                    value: "{current_model}",
                                    onchange: move |e| {
                                        let val = e.value();
                                        if val.is_empty() {
                                            config_values.write().remove("_model");
                                        } else {
                                            config_values.write().insert("_model".to_string(), val);
                                        }
                                    },
                                    option { value: "", "Default" }
                                    for name in &model_names {
                                        option {
                                            value: "{name}",
                                            selected: *name == current_model,
                                            "{name}"
                                        }
                                    }
                                }
                            }
                        }
                    } else {
                        rsx! {}
                    }
                }

                // Save and Test Run buttons
                div { style: "display: flex; justify-content: flex-end; gap: 8px; margin-top: 20px;",
                    button {
                        style: "padding: 10px 24px; background: #2563eb; color: #fff; border: none; border-radius: 8px; font-size: 14px; cursor: pointer;",
                        onclick: {
                            let pkg_id = pkg_id_save.clone();
                            let state = app_state_save.clone();
                            move |_| {
                                let mgr = PackageManager::new(state.packages_dir.clone());
                                for (key, value) in config_values.read().iter() {
                                    let _ = mgr.set_config(&pkg_id, key, value);
                                }
                                saved.set(true);
                            }
                        },
                        if saved() { "Saved" } else { "Save" }
                    }
                    {
                        let is_testing = testing();
                        let test_bg = if is_testing { "#94a3b8" } else { "#059669" };
                        let test_label = if is_testing { "Running..." } else { "Test Run" };
                        rsx! {
                    button {
                        style: "padding: 10px 24px; background: {test_bg}; color: #fff; border: none; border-radius: 8px; font-size: 14px; cursor: pointer;",
                        disabled: is_testing,
                        onclick: {
                            let pkg_id = pkg_id_test.clone();
                            let state = app_state_test.clone();
                            move |_| {
                                testing.set(true);
                                test_output.set(String::new());

                                let pkg_id = pkg_id.clone();
                                let packages_dir = state.packages_dir.clone();
                                let config = config_values.read().clone();
                                let store = state.store.clone();

                                spawn(async move {
                                    let result = run_skill_test(pkg_id, packages_dir, config, store).await;
                                    test_output.set(result);
                                    testing.set(false);
                                });
                            }
                        },
                        "{test_label}"
                    }
                        }
                    }
                }

                // Test output area
                if !test_output.read().is_empty() || testing() {
                    div { style: "margin-top: 16px; padding: 12px; background: #f8fafc; border: 1px solid #e2e8f0; border-radius: 8px;",
                        label { style: "display: block; font-size: 12px; font-weight: 600; color: #475569; margin-bottom: 6px;",
                            "Test Output"
                        }
                        if testing() {
                            p { style: "font-size: 13px; color: #64748b; margin: 0;", "Running skill..." }
                        } else {
                            pre { style: "font-size: 12px; color: #1e293b; margin: 0; white-space: pre-wrap; word-break: break-all; font-family: 'SF Mono', 'Fira Code', monospace;",
                                "{test_output}"
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Run skill test in background. Returns the output string.
async fn run_skill_test(
    pkg_id: String,
    packages_dir: std::path::PathBuf,
    config: HashMap<String, String>,
    store: Arc<tokio::sync::Mutex<kittypaw_store::Store>>,
) -> String {
    let js_path = packages_dir.join(&pkg_id).join("main.js");
    let js_code = match std::fs::read_to_string(&js_path) {
        Ok(code) => code,
        Err(e) => return format!("Error reading main.js: {e}"),
    };

    let sandbox = kittypaw_sandbox::Sandbox::new_threaded(SandboxConfig {
        timeout_secs: 30,
        memory_limit_mb: 128,
        allowed_paths: vec![],
        allowed_hosts: vec![],
    });

    // Build resolver for real data
    let config_for_resolver = kittypaw_core::config::Config::default();
    let store_for_resolver = store.clone();
    let resolver: Option<kittypaw_sandbox::SkillResolver> = Some(Arc::new(move |call| {
        let store = store_for_resolver.clone();
        let config = config_for_resolver.clone();
        Box::pin(async move {
            kittypaw_cli::skill_executor::resolve_skill_call(&call, &config, &store, None, None)
                .await
        })
    }));

    let context = serde_json::json!({
        "config": config,
        "package_id": pkg_id,
        "user": {},
    });
    let wrapped = format!("const ctx = JSON.parse(__context__);\n{js_code}");

    match sandbox
        .execute_with_resolver(&wrapped, context, resolver)
        .await
    {
        Ok(result) => {
            if result.success {
                if result.output.is_empty() {
                    "(no output)".into()
                } else {
                    result.output
                }
            } else {
                format!("Error: {}", result.error.unwrap_or_default())
            }
        }
        Err(e) => format!("Execution error: {e}"),
    }
}
