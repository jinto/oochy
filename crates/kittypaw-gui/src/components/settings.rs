use crate::state::AppState;
use dioxus::prelude::*;

#[component]
pub fn SettingsDialog(on_close: EventHandler) -> Element {
    let app_state = use_context::<AppState>();
    let mut api_key = use_signal(String::new);
    let mut saved = use_signal(|| false);

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
        });
    }

    let app_state_save = app_state.clone();

    rsx! {
        // Overlay
        div {
            style: "position: fixed; inset: 0; background: rgba(0,0,0,0.4); display: flex; align-items: center; justify-content: center; z-index: 100;",
            onclick: move |_| on_close.call(()),

            // Panel (stop propagation)
            div {
                style: "background: #fff; border-radius: 16px; padding: 28px; width: 520px; max-width: 94vw; box-shadow: 0 20px 60px rgba(0,0,0,0.2);",
                onclick: move |e| e.stop_propagation(),

                div { style: "display: flex; justify-content: space-between; align-items: center; margin-bottom: 24px;",
                    h2 { style: "font-size: 18px; font-weight: 600; color: #1e293b; margin: 0;", "Settings" }
                    button {
                        style: "background: none; border: none; font-size: 18px; color: #94a3b8; cursor: pointer;",
                        onclick: move |_| on_close.call(()),
                        "X"
                    }
                }

                div { style: "margin-bottom: 20px;",
                    label { style: "display: block; font-size: 13px; font-weight: 600; color: #374151; margin-bottom: 6px;", "Anthropic API Key" }
                    p { style: "font-size: 12px; color: #6b7280; margin-bottom: 8px;", "Your API key is stored locally." }
                    input {
                        style: "width: 100%; padding: 10px 12px; border: 1px solid #d1d5db; border-radius: 8px; font-size: 14px; font-family: monospace; outline: none; box-sizing: border-box;",
                        r#type: "password",
                        placeholder: "sk-ant-...",
                        value: "{api_key}",
                        oninput: move |e| api_key.set(e.value()),
                    }
                }

                div { style: "display: flex; justify-content: flex-end;",
                    button {
                        style: "padding: 10px 24px; background: #2563eb; color: #fff; border: none; border-radius: 8px; font-size: 14px; cursor: pointer;",
                        onclick: {
                            let state = app_state_save.clone();
                            move |_| {
                                let key = api_key.read().clone();
                                // Skip masked keys
                                if !key.starts_with("sk-...") {
                                    let _ = kittypaw_core::secrets::set_secret("settings", "api_key", &key);
                                    *state.api_key.lock().unwrap() = key;
                                }
                                saved.set(true);
                            }
                        },
                        if saved() { "Saved" } else { "Save" }
                    }
                }
            }
        }
    }
}
