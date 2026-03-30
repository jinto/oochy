use dioxus::prelude::*;

use super::{chat, dashboard, settings, sidebar, skill_gallery};

#[component]
pub fn App() -> Element {
    let mut active_tab = use_signal(|| "dashboard".to_string());

    rsx! {
        style { r#"
            html, body {{ margin: 0; padding: 0; height: 100%; overflow: hidden; }}
            ::-webkit-scrollbar {{ display: none; }}
            * {{ scrollbar-width: none; }}
        "# }
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
}
