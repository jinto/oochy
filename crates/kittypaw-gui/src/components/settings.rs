use std::sync::Arc;

use dioxus::prelude::*;
use kittypaw_llm::claude::ClaudeProvider;
use kittypaw_llm::openai::OpenAiProvider;

use crate::state::AppState;

#[component]
pub fn SettingsPanel() -> Element {
    rsx! {
        SettingsDialog { on_close: move |_| {} }
    }
}

#[component]
pub fn SettingsDialog(on_close: EventHandler) -> Element {
    let app_state = use_context::<AppState>();
    let mut api_key = use_signal(String::new);
    let mut saved = use_signal(|| false);

    // Local model signals
    let mut local_url = use_signal(String::new);
    let mut local_model = use_signal(String::new);
    let mut local_saved = use_signal(|| false);

    // Load current key on mount
    {
        let app_state = app_state.clone();
        use_effect(move || {
            let key = app_state.api_key.lock().unwrap().clone();
            if !key.is_empty() {
                let len = key.len();
                let suffix = &key[len.saturating_sub(4)..];
                api_key.set(format!("sk-...{suffix}"));
            }

            // Load stored local model config
            if let Ok(Some(url)) = kittypaw_core::secrets::get_secret("local_model", "base_url") {
                local_url.set(url);
            }
            if let Ok(Some(model)) = kittypaw_core::secrets::get_secret("local_model", "model_name")
            {
                local_model.set(model);
            }
        });
    }

    let app_state_save = app_state.clone();
    let app_state_local_save = app_state.clone();

    rsx! {
        // Tab panel — fills the main content area
        div {
            style: "flex: 1; background: #F5F3F0; padding: 32px; overflow-y: auto;",

            // Page header
            div { style: "display: flex; justify-content: space-between; align-items: center; margin-bottom: 28px;",
                h1 { style: "font-size: 24px; font-weight: 600; color: #1C1917; margin: 0;", "Settings" }
                button {
                    style: "background: none; border: 1px solid #E7E5E4; border-radius: 6px; padding: 6px 14px; font-size: 13px; color: #78716C; cursor: pointer;",
                    onclick: move |_| on_close.call(()),
                    "Back"
                }
            }

            // ── Anthropic API Key section ──
            div {
                style: "background: #FFFFFF; border: 1px solid #E7E5E4; border-radius: 10px; padding: 24px; margin-bottom: 16px;",

                div { style: "display: flex; align-items: center; gap: 8px; margin-bottom: 16px;",
                    div { style: "flex: 1; height: 1px; background: #E7E5E4;" }
                    span { style: "font-size: 12px; font-weight: 600; color: #78716C; white-space: nowrap;", "Anthropic API Key" }
                    div { style: "flex: 1; height: 1px; background: #E7E5E4;" }
                }
                p { style: "font-size: 12px; color: #78716C; margin-bottom: 8px;", "Your API key is stored locally." }
                input {
                    style: "width: 100%; padding: 10px 12px; border: 1px solid #E7E5E4; border-radius: 6px; font-size: 13px; font-family: monospace; outline: none; box-sizing: border-box; background: #F5F3F0; color: #1C1917;",
                    r#type: "password",
                    placeholder: "sk-ant-...",
                    value: "{api_key}",
                    oninput: move |e| api_key.set(e.value()),
                }

                div { style: "display: flex; justify-content: flex-end; margin-top: 14px;",
                    button {
                        style: "padding: 8px 20px; background: #1C1917; color: #F5F3F0; border: none; border-radius: 6px; font-size: 13px; cursor: pointer;",
                        onclick: {
                            let state = app_state_save.clone();
                            move |_| {
                                let key = api_key.read().clone();
                                // Skip masked keys
                                if !key.starts_with("sk-...") {
                                    let _ = kittypaw_core::secrets::set_secret("settings", "api_key", &key);
                                    *state.api_key.lock().unwrap() = key.clone();
                                    let mut registry = state.llm_registry.lock().unwrap();
                                    registry.register(
                                        "claude-sonnet",
                                        Arc::new(ClaudeProvider::new(
                                            key,
                                            "claude-sonnet-4-20250514".into(),
                                            4096,
                                        )),
                                    );
                                }
                                saved.set(true);
                            }
                        },
                        if saved() { "Saved" } else { "Save" }
                    }
                }
            }

            // ── 로컬 모델 연결 section ──
            div {
                style: "background: #FFFFFF; border: 1px solid #E7E5E4; border-radius: 10px; padding: 24px;",

                div { style: "display: flex; align-items: center; gap: 8px; margin-bottom: 16px;",
                    div { style: "flex: 1; height: 1px; background: #E7E5E4;" }
                    span { style: "font-size: 12px; font-weight: 600; color: #78716C; white-space: nowrap;", "로컬 모델 연결" }
                    div { style: "flex: 1; height: 1px; background: #E7E5E4;" }
                }

                div { style: "margin-bottom: 12px;",
                    label { style: "display: block; font-size: 13px; font-weight: 600; color: #1C1917; margin-bottom: 6px;", "모델 서버 URL" }
                    input {
                        style: "width: 100%; padding: 10px 12px; border: 1px solid #E7E5E4; border-radius: 6px; font-size: 13px; outline: none; box-sizing: border-box; background: #F5F3F0; color: #1C1917;",
                        r#type: "text",
                        placeholder: "http://localhost:11434/v1",
                        value: "{local_url}",
                        oninput: move |e| local_url.set(e.value()),
                    }
                }

                div { style: "margin-bottom: 14px;",
                    label { style: "display: block; font-size: 13px; font-weight: 600; color: #1C1917; margin-bottom: 6px;", "모델 이름" }
                    input {
                        style: "width: 100%; padding: 10px 12px; border: 1px solid #E7E5E4; border-radius: 6px; font-size: 13px; outline: none; box-sizing: border-box; background: #F5F3F0; color: #1C1917;",
                        r#type: "text",
                        placeholder: "qwen3.5:27b",
                        value: "{local_model}",
                        oninput: move |e| local_model.set(e.value()),
                    }
                }

                div { style: "display: flex; justify-content: flex-end; margin-bottom: 14px;",
                    button {
                        style: "padding: 8px 20px; background: #1C1917; color: #F5F3F0; border: none; border-radius: 6px; font-size: 13px; cursor: pointer;",
                        onclick: {
                            let state = app_state_local_save.clone();
                            move |_| {
                                let url = local_url.read().clone();
                                let model = local_model.read().clone();
                                if !url.is_empty() && !model.is_empty() {
                                    let _ = kittypaw_core::secrets::set_secret("local_model", "base_url", &url);
                                    let _ = kittypaw_core::secrets::set_secret("local_model", "model_name", &model);
                                    let mut registry = state.llm_registry.lock().unwrap();
                                    registry.register(
                                        "local",
                                        Arc::new(OpenAiProvider::with_base_url(
                                            url,
                                            String::new(),
                                            model,
                                            4096,
                                        )),
                                    );
                                    registry.set_default("local");
                                }
                                local_saved.set(true);
                            }
                        },
                        if local_saved() { "저장 완료" } else { "저장" }
                    }
                }

                p { style: "font-size: 12px; color: #78716C; line-height: 1.5;",
                    "Ollama, LM Studio 등 OpenAI 호환 API 서버를 연결합니다."
                    br {}
                    "로컬 모델 연결 시 API 키 없이 무료로 사용할 수 있습니다."
                }
            }
        }
    }
}
