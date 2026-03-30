use dioxus::prelude::*;

#[component]
pub fn Sidebar(active_tab: String, on_tab_change: EventHandler<String>) -> Element {
    rsx! {
        div {
            style: "width: 220px; min-width: 220px; background: #1C1917; color: #D6D3D1; display: flex; flex-direction: column; padding: 12px 16px; font-family: Inter, -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif; overflow: hidden; flex-shrink: 0;",

            // Logo
            div {
                style: "margin-top: 12px; margin-bottom: 16px;",
                span {
                    style: "font-family: 'Fraunces', Georgia, serif; font-weight: 700; font-size: 17px; color: #FFFFFF;",
                    "Kitty"
                }
                span {
                    style: "font-family: 'Fraunces', Georgia, serif; font-weight: 700; font-size: 17px; color: #86EFAC;",
                    "Paw"
                }
            }

            // Nav items
            div {
                style: "display: flex; flex-direction: column; gap: 2px; flex: 1;",

                NavItem {
                    label: "Dashboard",
                    icon: "📊",
                    tab: "dashboard",
                    active: active_tab == "dashboard",
                    on_click: move |_| on_tab_change.call("dashboard".into()),
                }
                NavItem {
                    label: "Skills",
                    icon: "🧩",
                    tab: "skills",
                    active: active_tab == "skills",
                    on_click: move |_| on_tab_change.call("skills".into()),
                }
                NavItem {
                    label: "Chat",
                    icon: "💬",
                    tab: "chat",
                    active: active_tab == "chat",
                    on_click: move |_| on_tab_change.call("chat".into()),
                }
                NavItem {
                    label: "Settings",
                    icon: "⚙️",
                    tab: "settings",
                    active: active_tab == "settings",
                    on_click: move |_| on_tab_change.call("settings".into()),
                }
            }

            // Footer
            div {
                style: "color: #57534E; font-size: 11px; padding-top: 4px;",
                "v0.1.0"
            }
        }
    }
}

#[component]
fn NavItem(
    label: &'static str,
    icon: &'static str,
    tab: &'static str,
    active: bool,
    on_click: EventHandler,
) -> Element {
    let bg = if active { "#292524" } else { "transparent" };
    let color = if active { "#FFFFFF" } else { "#D6D3D1" };

    rsx! {
        button {
            style: "display: flex; align-items: center; gap: 10px; background: {bg}; border: none; color: {color}; padding: 8px 10px; border-radius: 6px; cursor: pointer; font-size: 13px; text-align: left; width: 100%;",
            onclick: move |_| on_click.call(()),
            span { style: "font-size: 15px; line-height: 1;", "{icon}" }
            span { "{label}" }
        }
    }
}
