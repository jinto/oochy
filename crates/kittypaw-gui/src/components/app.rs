use dioxus::prelude::*;

use super::{chat, dashboard, onboarding, permission_dialog, settings, sidebar, skill_gallery};
use crate::state::{AppState, PermissionQueue};

#[component]
pub fn App() -> Element {
    let app_state = use_context::<AppState>();
    let mut active_tab = use_signal(|| "chat".to_string());
    let mut onboarding_done = use_signal(|| false);

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
