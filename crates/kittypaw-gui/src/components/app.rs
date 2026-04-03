use dioxus::prelude::*;

use super::{chat, dashboard, onboarding, permission_dialog, settings, sidebar, skill_gallery};
use crate::state::{AppState, PermissionQueue};

#[component]
pub fn App() -> Element {
    let app_state = use_context::<AppState>();
    let mut active_tab = use_signal(|| "dashboard".to_string());
    let mut onboarding_done = use_signal(|| false);

    // Provide the reactive permission queue as context so that both the
    // dialog component and any async tasks can push / pop requests.
    use_context_provider(|| PermissionQueue {
        requests: Signal::new(Vec::new()),
    });

    // Check on mount whether onboarding was already completed
    {
        let app_state = app_state.clone();
        use_effect(move || {
            let store = app_state.store.blocking_lock();
            if store
                .get_user_context("onboarding_completed")
                .ok()
                .flatten()
                .is_some()
            {
                onboarding_done.set(true);
            }
        });
    }

    rsx! {
        style { r#"
            html, body {{ margin: 0; padding: 0; height: 100%; overflow: hidden; }}
            ::-webkit-scrollbar {{ display: none; }}
            * {{ scrollbar-width: none; }}
        "# }

        if !onboarding_done() {
            onboarding::Onboarding {
                on_complete: move |_| onboarding_done.set(true),
            }
        } else {
            div { class: "app",
                style: "display: flex; height: 100vh; font-family: 'Inter', -apple-system, BlinkMacSystemFont, sans-serif; overflow: hidden; position: fixed; top: 0; left: 0; right: 0; bottom: 0;",

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
