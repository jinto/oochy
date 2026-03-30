use crate::state::AppState;
use dioxus::prelude::*;
use kittypaw_core::package_manager::PackageManager;
use kittypaw_core::registry::{RegistryClient, RegistryEntry, RegistryIndex};
use std::collections::HashSet;

#[derive(Clone)]
enum StoreState {
    Loading,
    Loaded(RegistryIndex),
    Error(String),
}

#[component]
pub fn SkillStore() -> Element {
    let app_state = use_context::<AppState>();
    let mut store_state = use_signal(|| StoreState::Loading);
    let mut installed_ids = use_signal(HashSet::<String>::new);
    let mut filter = use_signal(String::new);

    use_effect(move || {
        let packages_dir = app_state.packages_dir.clone();
        spawn(async move {
            let mgr = PackageManager::new(packages_dir.clone());
            let installed: HashSet<String> = mgr
                .list_installed()
                .unwrap_or_default()
                .into_iter()
                .map(|p| p.meta.id)
                .collect();
            installed_ids.set(installed);

            let cache_dir = cache_dir_for(&packages_dir);
            let client = RegistryClient::new(&cache_dir);

            match client.fetch_index().await {
                Ok(index) => store_state.set(StoreState::Loaded(index)),
                Err(e) => store_state.set(StoreState::Error(e.to_string())),
            }
        });
    });

    rsx! {
        div { style: "flex: 1; display: flex; flex-direction: column; overflow: hidden;",

            div { style: "padding: 12px 16px;",
                input {
                    style: "width: 100%; padding: 10px 14px; border: 1px solid #E7E5E4; border-radius: 10px; font-size: 13px; outline: none; box-sizing: border-box; color: #1C1917;",
                    placeholder: "Search store...",
                    value: "{filter}",
                    oninput: move |e| filter.set(e.value()),
                }
            }

            div { style: "flex: 1; overflow-y: auto; padding: 0 16px 16px;",
                match store_state.read().clone() {
                    StoreState::Loading => rsx! {
                        div { style: "display: flex; align-items: center; justify-content: center; height: 200px; color: #78716C; font-size: 13px;",
                            "Loading store..."
                        }
                    },
                    StoreState::Error(msg) => rsx! {
                        div { style: "display: flex; flex-direction: column; align-items: center; justify-content: center; height: 200px; gap: 8px;",
                            p { style: "font-size: 15px; color: #1C1917; margin: 0;", "스토어에 연결할 수 없습니다" }
                            p { style: "font-size: 12px; color: #78716C; margin: 0;", "{msg}" }
                        }
                    },
                    StoreState::Loaded(index) => {
                        let ids_snapshot = installed_ids.read().clone();
                        let f = filter.read().to_lowercase();
                        let available: Vec<RegistryEntry> = index
                            .packages
                            .into_iter()
                            .filter(|e| !ids_snapshot.contains(&e.id))
                            .filter(|e| {
                                f.is_empty()
                                    || e.name.to_lowercase().contains(&f)
                                    || e.description.to_lowercase().contains(&f)
                                    || e.category.to_lowercase().contains(&f)
                            })
                            .collect();

                        rsx! {
                            if available.is_empty() {
                                div { style: "text-align: center; padding: 40px; color: #78716C;",
                                    h2 { style: "font-size: 18px; color: #1C1917;", "모두 설치됨" }
                                    p { style: "font-size: 13px;", "사용 가능한 새 스킬이 없습니다." }
                                }
                            } else {
                                div { style: "display: grid; grid-template-columns: repeat(auto-fill, minmax(280px, 1fr)); gap: 12px;",
                                    for entry in available.iter() {
                                        StoreCard {
                                            key: "{entry.id}",
                                            entry: entry.clone(),
                                            on_installed: move |id: String| {
                                                installed_ids.write().insert(id);
                                            },
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn cache_dir_for(packages_dir: &std::path::Path) -> std::path::PathBuf {
    packages_dir.parent().unwrap_or(packages_dir).join("cache")
}

#[component]
fn StoreCard(entry: RegistryEntry, on_installed: EventHandler<String>) -> Element {
    let app_state = use_context::<AppState>();
    let mut installing = use_signal(|| false);
    let mut install_error = use_signal(String::new);

    let entry_for_install = entry.clone();
    let install_id = entry.id.clone();

    rsx! {
        div {
            style: "border: 1px solid #E7E5E4; border-radius: 10px; padding: 16px; display: flex; flex-direction: column; gap: 8px; background: #FFFFFF;",

            div { style: "display: flex; justify-content: space-between; align-items: start;",
                h3 { style: "font-size: 15px; font-weight: 600; color: #1C1917; margin: 0;",
                    "{entry.name}"
                }
                span { style: "font-size: 11px; padding: 2px 8px; background: #F5F5F4; border-radius: 9999px; color: #78716C; white-space: nowrap;",
                    "{entry.category}"
                }
            }

            p { style: "font-size: 13px; color: #78716C; margin: 0; line-height: 1.4; flex: 1;",
                "{entry.description}"
            }

            p { style: "font-size: 11px; color: #78716C; margin: 0;",
                "by {entry.author}  ·  v{entry.version}"
            }

            if !install_error.read().is_empty() {
                p { style: "font-size: 12px; color: #EF4444; margin: 0;",
                    "{install_error}"
                }
            }

            button {
                style: {
                    let base = "width: 100%; padding: 8px 0; border: none; border-radius: 6px; font-size: 13px; font-weight: 500; cursor: pointer;";
                    if *installing.read() {
                        format!("{base} background: #D1FAE5; color: #166534; cursor: not-allowed;")
                    } else {
                        format!("{base} background: #86EFAC; color: #166534;")
                    }
                },
                disabled: *installing.read(),
                onclick: move |_| {
                    let packages_dir = app_state.packages_dir.clone();
                    let entry = entry_for_install.clone();
                    let id_for_signal = install_id.clone();
                    installing.set(true);
                    install_error.set(String::new());
                    spawn(async move {
                        let mgr = PackageManager::new(packages_dir.clone());
                        let cache_dir = cache_dir_for(&packages_dir);
                        let client = RegistryClient::new(&cache_dir);
                        match mgr.install_from_registry(&client, &entry).await {
                            Ok(_) => {
                                on_installed.call(id_for_signal);
                            }
                            Err(e) => {
                                install_error.set(e.to_string());
                                installing.set(false);
                            }
                        }
                    });
                },
                if *installing.read() { "Installing..." } else { "Install" }
            }
        }
    }
}
