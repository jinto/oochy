use dioxus::prelude::*;

use super::{chat, dashboard, onboarding, permission_dialog, settings, sidebar, skill_gallery};
use crate::i18n::{I18n, Locale};
use crate::state::{AppState, PermissionQueue};

#[component]
pub fn App() -> Element {
    let app_state = use_context::<AppState>();
    let mut active_tab = use_signal(|| "chat".to_string());
    let mut onboarding_done = use_signal(|| false);

    // I18n: provide locale as context, default to system locale or Korean
    use_context_provider(|| Signal::new(I18n::new(Locale::Ko)));

    // Load saved locale from store
    {
        let store_arc = app_state.store.clone();
        use_effect(move || {
            let store_arc = store_arc.clone();
            spawn(async move {
                let store = store_arc.lock().await;
                if let Ok(Some(lang)) = store.get_user_context("locale") {
                    let mut i18n = use_context::<Signal<I18n>>();
                    i18n.set(I18n::new(Locale::from_str(&lang)));
                }
            });
        });
    }

    // Start channel polling in background (so bot responds even without `kittypaw serve`)
    {
        let state = app_state.clone();
        use_effect(move || {
            let state = state.clone();
            spawn(async move {
                let config = kittypaw_core::config::Config::load().unwrap_or_default();
                let channels =
                    kittypaw_channels::registry::ChannelRegistry::create_all(&config.channels);
                if channels.is_empty() {
                    return;
                }

                tracing::info!(
                    count = channels.len(),
                    "Starting background channel polling from GUI"
                );
                let (event_tx, mut event_rx) =
                    tokio::sync::mpsc::channel::<kittypaw_core::types::Event>(64);

                // Spawn polling for each configured channel
                for channel in channels {
                    let tx = event_tx.clone();
                    tokio::spawn(async move {
                        if let Err(e) = channel.start(tx).await {
                            tracing::error!("Channel polling error: {e}");
                        }
                    });
                }

                // Process incoming Telegram messages
                while let Some(event) = event_rx.recv().await {
                    let text = event
                        .payload
                        .get("text")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let chat_id = event
                        .payload
                        .get("chat_id")
                        .map(|v| {
                            v.as_str()
                                .map(|s| s.to_string())
                                .unwrap_or_else(|| v.to_string())
                        })
                        .unwrap_or_default();

                    if text.is_empty() {
                        continue;
                    }

                    // Run agent loop
                    let provider = {
                        let registry = state.llm_registry.lock().unwrap();
                        registry.default_provider()
                    };
                    let provider = match provider {
                        Some(p) => p,
                        None => continue,
                    };
                    let sandbox =
                        kittypaw_sandbox::sandbox::Sandbox::new_threaded(config.sandbox.clone());
                    let session = kittypaw_cli::agent_loop::AgentSession {
                        provider: provider.as_ref(),
                        fallback_provider: None,
                        sandbox: &sandbox,
                        store: state.store.clone(),
                        config: &config,
                        on_token: None,
                        on_permission_request: None,
                    };

                    let response = match session.run(event).await {
                        Ok(text) => text,
                        Err(e) => format!("Error: {e}"),
                    };

                    // Send response back to Telegram
                    if let Ok(Some(token)) =
                        kittypaw_core::secrets::get_secret("telegram", "bot_token")
                    {
                        let _ = kittypaw_core::telegram::send_message(&token, &chat_id, &response)
                            .await;
                    }
                }
            });
        });
    }

    // Provide the reactive permission queue as context so that both the
    // dialog component and any async tasks can push / pop requests.
    use_context_provider(|| PermissionQueue {
        requests: Signal::new(Vec::new()),
    });

    // Check on mount whether onboarding was already completed
    {
        let store_arc = app_state.store.clone();
        use_effect(move || {
            let store_arc = store_arc.clone();
            spawn(async move {
                let store = store_arc.lock().await;
                if store
                    .get_user_context("onboarding_completed")
                    .ok()
                    .flatten()
                    .is_some()
                {
                    onboarding_done.set(true);
                }
            });
        });
    }

    rsx! {
        style { r#"
            html, body {{ margin: 0; padding: 0; height: 100%; overflow: hidden; }}
            ::-webkit-scrollbar {{ display: none; }}
            * {{ scrollbar-width: none; }}
            .markdown-body h1 {{ font-size: 1.4em; font-weight: 700; margin: 12px 0 6px; }}
            .markdown-body h2 {{ font-size: 1.2em; font-weight: 700; margin: 10px 0 4px; }}
            .markdown-body h3 {{ font-size: 1.05em; font-weight: 600; margin: 8px 0 4px; }}
            .markdown-body p {{ margin: 4px 0; }}
            .markdown-body ul, .markdown-body ol {{ margin: 4px 0; padding-left: 20px; }}
            .markdown-body li {{ margin: 2px 0; }}
            .markdown-body code {{ background: #f1f5f9; padding: 2px 5px; border-radius: 4px; font-size: 0.9em; }}
            .markdown-body pre {{ background: #1e293b; color: #e2e8f0; padding: 12px; border-radius: 8px; overflow-x: auto; margin: 8px 0; }}
            .markdown-body pre code {{ background: none; padding: 0; color: inherit; }}
            .markdown-body strong {{ font-weight: 700; }}
            .markdown-body table {{ border-collapse: collapse; margin: 8px 0; }}
            .markdown-body th, .markdown-body td {{ border: 1px solid #e2e8f0; padding: 6px 10px; font-size: 13px; }}
            .markdown-body th {{ background: #f8fafc; font-weight: 600; }}
        "# }

        if !onboarding_done() {
            onboarding::Onboarding {
                on_complete: move |_| onboarding_done.set(true),
            }
        } else {
            div { class: "app",
                style: "display: flex; height: 100vh; font-family: 'Inter', -apple-system, BlinkMacSystemFont, sans-serif; overflow: hidden;",

                sidebar::Sidebar {
                    active_tab: active_tab(),
                    on_tab_change: move |tab: String| active_tab.set(tab),
                    on_reset_onboarding: move |_| onboarding_done.set(false),
                }

                div { class: "main",
                    style: "flex: 1; display: flex; flex-direction: column; background: #F5F3F0; overflow: hidden; min-height: 0;",

                    match active_tab().as_str() {
                        "dashboard" => rsx! { dashboard::Dashboard {} },
                        "skills" => rsx! { skill_gallery::SkillGallery {} },
                        "chat" => rsx! { chat::ChatPanel {} },
                        "settings" => rsx! { settings::SettingsPanel {} },
                        _ => rsx! { dashboard::Dashboard {} },
                    }
                }
            }
        }

        // Permission modal — renders on top of everything when requests are pending.
        permission_dialog::PermissionDialog {}
    }
}
