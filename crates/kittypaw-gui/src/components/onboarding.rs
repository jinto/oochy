use std::sync::Arc;

use dioxus::prelude::*;
use kittypaw_llm::claude::ClaudeProvider;
use kittypaw_llm::openai::OpenAiProvider;

use crate::state::AppState;

#[derive(Clone, PartialEq)]
enum LlmChoice {
    None,
    Local,
    OpenRouter,
    Claude,
}

#[component]
pub fn Onboarding(on_complete: EventHandler) -> Element {
    let mut step = use_signal(|| 1u8);

    match step() {
        1 => rsx! { StepWelcome { on_next: move |_| step.set(2) } },
        2 => rsx! { StepLlm { on_next: move |_| step.set(3) } },
        3 => rsx! { StepTelegram { on_next: move |_| step.set(4) } },
        _ => rsx! { StepComplete { on_complete } },
    }
}

// ── Step 1: Welcome ───────────────────────────────────────────────────────────

#[component]
fn StepWelcome(on_next: EventHandler) -> Element {
    rsx! {
        div {
            style: "position: fixed; inset: 0; background: #F5F3F0; display: flex; align-items: center; justify-content: center; font-family: 'Inter', -apple-system, BlinkMacSystemFont, sans-serif;",

            div {
                style: "background: #FFFFFF; border: 1px solid #E7E5E4; border-radius: 14px; padding: 56px 64px; max-width: 480px; width: 100%; text-align: center; box-shadow: 0 4px 24px rgba(0,0,0,0.06);",

                h1 {
                    style: "font-family: 'Fraunces', Georgia, serif; font-size: 40px; font-weight: 700; color: #1C1917; margin: 0 0 12px 0; letter-spacing: -0.5px;",
                    "Kitty"
                    span { style: "color: #166534;", "Paw" }
                }

                p {
                    style: "font-size: 16px; color: #78716C; margin: 0 0 40px 0; line-height: 1.6;",
                    "AI 자동화를 3분 안에 시작하세요"
                }

                button {
                    style: "padding: 14px 40px; background: #86EFAC; color: #166534; border: none; border-radius: 6px; font-size: 15px; font-weight: 600; cursor: pointer; transition: opacity 80ms ease-out;",
                    onclick: move |_| on_next.call(()),
                    "시작하기"
                }
            }
        }
    }
}

// ── Step 2: LLM Selection ─────────────────────────────────────────────────────

#[component]
fn StepLlm(on_next: EventHandler) -> Element {
    let app_state = use_context::<AppState>();

    let mut choice = use_signal(|| LlmChoice::None);
    let mut local_url = use_signal(|| "http://localhost:11434/v1".to_string());
    let mut local_model = use_signal(|| "qwen3.5:27b".to_string());
    let mut api_key = use_signal(String::new);

    let can_proceed = move || match choice() {
        LlmChoice::None => false,
        LlmChoice::Local => !local_url().is_empty() && !local_model().is_empty(),
        LlmChoice::OpenRouter => !api_key().is_empty(),
        LlmChoice::Claude => !api_key().is_empty(),
    };

    let card_base = "border: 2px solid; border-radius: 10px; padding: 20px; cursor: pointer; text-align: left; width: 100%; background: #FFFFFF; box-sizing: border-box;";

    rsx! {
        div {
            style: "position: fixed; inset: 0; background: #F5F3F0; display: flex; align-items: center; justify-content: center; font-family: 'Inter', -apple-system, BlinkMacSystemFont, sans-serif;",

            div {
                style: "background: #FFFFFF; border: 1px solid #E7E5E4; border-radius: 14px; padding: 48px 56px; max-width: 520px; width: 100%; box-shadow: 0 4px 24px rgba(0,0,0,0.06);",

                h2 {
                    style: "font-family: 'Fraunces', Georgia, serif; font-size: 28px; font-weight: 700; color: #1C1917; margin: 0 0 28px 0;",
                    "AI 모델을 선택하세요"
                }

                // Local LLM card
                div {
                    style: if choice() == LlmChoice::Local {
                        format!("{card_base} border-color: #86EFAC;")
                    } else {
                        format!("{card_base} border-color: #E7E5E4;")
                    },
                    onclick: move |_| choice.set(LlmChoice::Local),

                    div { style: "display: flex; align-items: center; gap: 10px; margin-bottom: 4px;",
                        div {
                            style: if choice() == LlmChoice::Local {
                                "width: 16px; height: 16px; border-radius: 50%; border: 2px solid #86EFAC; background: #86EFAC; flex-shrink: 0;"
                            } else {
                                "width: 16px; height: 16px; border-radius: 50%; border: 2px solid #E7E5E4; background: transparent; flex-shrink: 0;"
                            }
                        }
                        span { style: "font-size: 14px; font-weight: 600; color: #1C1917;", "로컬 LLM (Ollama)" }
                    }
                    p { style: "font-size: 12px; color: #78716C; margin: 0 0 0 26px;", "무료, 내 컴퓨터에서 실행" }

                    if choice() == LlmChoice::Local {
                        div { style: "margin-top: 16px; display: flex; flex-direction: column; gap: 10px;",
                            div {
                                label { style: "display: block; font-size: 12px; font-weight: 600; color: #1C1917; margin-bottom: 4px;", "서버 URL" }
                                input {
                                    style: "width: 100%; padding: 10px 12px; border: 1px solid #E7E5E4; border-radius: 6px; font-size: 13px; outline: none; box-sizing: border-box; background: #F5F3F0; color: #1C1917;",
                                    r#type: "text",
                                    value: "{local_url}",
                                    oninput: move |e| local_url.set(e.value()),
                                }
                            }
                            div {
                                label { style: "display: block; font-size: 12px; font-weight: 600; color: #1C1917; margin-bottom: 4px;", "모델 이름" }
                                input {
                                    style: "width: 100%; padding: 10px 12px; border: 1px solid #E7E5E4; border-radius: 6px; font-size: 13px; outline: none; box-sizing: border-box; background: #F5F3F0; color: #1C1917;",
                                    r#type: "text",
                                    value: "{local_model}",
                                    oninput: move |e| local_model.set(e.value()),
                                }
                            }
                        }
                    }
                }

                div { style: "height: 12px;" }

                // OpenRouter card
                div {
                    style: if choice() == LlmChoice::OpenRouter {
                        format!("{card_base} border-color: #86EFAC;")
                    } else {
                        format!("{card_base} border-color: #E7E5E4;")
                    },
                    onclick: move |_| choice.set(LlmChoice::OpenRouter),

                    div { style: "display: flex; align-items: center; gap: 10px; margin-bottom: 4px;",
                        div {
                            style: if choice() == LlmChoice::OpenRouter {
                                "width: 16px; height: 16px; border-radius: 50%; border: 2px solid #86EFAC; background: #86EFAC; flex-shrink: 0;"
                            } else {
                                "width: 16px; height: 16px; border-radius: 50%; border: 2px solid #E7E5E4; background: transparent; flex-shrink: 0;"
                            }
                        }
                        span { style: "font-size: 14px; font-weight: 600; color: #1C1917;", "OpenRouter (무료)" }
                    }
                    p { style: "font-size: 12px; color: #78716C; margin: 0 0 0 26px;", "무료 AI 모델로 바로 시작하세요. OpenRouter에서 API 키만 발급받으면 됩니다." }

                    if choice() == LlmChoice::OpenRouter {
                        div { style: "margin-top: 16px;",
                            label { style: "display: block; font-size: 12px; font-weight: 600; color: #1C1917; margin-bottom: 4px;", "API 키" }
                            input {
                                style: "width: 100%; padding: 10px 12px; border: 1px solid #E7E5E4; border-radius: 6px; font-size: 13px; font-family: monospace; outline: none; box-sizing: border-box; background: #F5F3F0; color: #1C1917;",
                                r#type: "password",
                                placeholder: "sk-or-...",
                                value: "{api_key}",
                                oninput: move |e| api_key.set(e.value()),
                            }
                            div { style: "font-size: 11px; color: #A8A29E; margin: 10px 0 0 0; line-height: 1.8;",
                                "1. "
                                a {
                                    href: "https://openrouter.ai/settings/keys",
                                    style: "color: #86EFAC; text-decoration: underline; cursor: pointer;",
                                    onclick: move |e| {
                                        e.prevent_default();
                                        document::eval(r#"window.open('https://openrouter.ai/settings/keys', '_blank')"#);
                                    },
                                    "openrouter.ai"
                                }
                                " 에서 무료 가입"
                                br {}
                                "2. API Keys → Create Key"
                                br {}
                                "3. 발급된 키를 여기에 붙여넣기"
                            }
                        }
                    }
                }

                div { style: "height: 12px;" }

                // Claude API card
                div {
                    style: if choice() == LlmChoice::Claude {
                        format!("{card_base} border-color: #86EFAC;")
                    } else {
                        format!("{card_base} border-color: #E7E5E4;")
                    },
                    onclick: move |_| choice.set(LlmChoice::Claude),

                    div { style: "display: flex; align-items: center; gap: 10px; margin-bottom: 4px;",
                        div {
                            style: if choice() == LlmChoice::Claude {
                                "width: 16px; height: 16px; border-radius: 50%; border: 2px solid #86EFAC; background: #86EFAC; flex-shrink: 0;"
                            } else {
                                "width: 16px; height: 16px; border-radius: 50%; border: 2px solid #E7E5E4; background: transparent; flex-shrink: 0;"
                            }
                        }
                        span { style: "font-size: 14px; font-weight: 600; color: #1C1917;", "Claude API" }
                    }
                    p { style: "font-size: 12px; color: #78716C; margin: 0 0 0 26px;", "고품질, API 키 필요" }

                    if choice() == LlmChoice::Claude {
                        div { style: "margin-top: 16px;",
                            label { style: "display: block; font-size: 12px; font-weight: 600; color: #1C1917; margin-bottom: 4px;", "API 키" }
                            input {
                                style: "width: 100%; padding: 10px 12px; border: 1px solid #E7E5E4; border-radius: 6px; font-size: 13px; font-family: monospace; outline: none; box-sizing: border-box; background: #F5F3F0; color: #1C1917;",
                                r#type: "password",
                                placeholder: "sk-ant-...",
                                value: "{api_key}",
                                oninput: move |e| api_key.set(e.value()),
                            }
                        }
                    }
                }

                div { style: "display: flex; justify-content: flex-end; margin-top: 28px;",
                    button {
                        style: if can_proceed() {
                            "padding: 12px 32px; background: #86EFAC; color: #166534; border: none; border-radius: 6px; font-size: 14px; font-weight: 600; cursor: pointer;"
                        } else {
                            "padding: 12px 32px; background: #E7E5E4; color: #78716C; border: none; border-radius: 6px; font-size: 14px; font-weight: 600; cursor: not-allowed;"
                        },
                        disabled: !can_proceed(),
                        onclick: {
                            let app_state = app_state.clone();
                            move |_| {
                                if !can_proceed() {
                                    return;
                                }
                                match choice() {
                                    LlmChoice::Local => {
                                        let url = local_url.read().clone();
                                        let model = local_model.read().clone();
                                        let _ = kittypaw_core::secrets::set_secret("local_model", "base_url", &url);
                                        let _ = kittypaw_core::secrets::set_secret("local_model", "model_name", &model);
                                        let mut registry = app_state.llm_registry.lock().unwrap();
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
                                    LlmChoice::OpenRouter => {
                                        let key = api_key.read().clone();
                                        let _ = kittypaw_core::secrets::set_secret("models", "openrouter", &key);
                                        let mut registry = app_state.llm_registry.lock().unwrap();
                                        registry.register(
                                            "openrouter",
                                            Arc::new(OpenAiProvider::with_base_url(
                                                "https://openrouter.ai/api/v1".into(),
                                                key,
                                                "qwen/qwen3-235b-a22b:free".into(),
                                                4096,
                                            )),
                                        );
                                    }
                                    LlmChoice::Claude => {
                                        let key = api_key.read().clone();
                                        let _ = kittypaw_core::secrets::set_secret("settings", "api_key", &key);
                                        let mut registry = app_state.llm_registry.lock().unwrap();
                                        registry.register(
                                            "claude-sonnet",
                                            Arc::new(ClaudeProvider::new(
                                                key,
                                                "claude-sonnet-4-20250514".into(),
                                                4096,
                                            )),
                                        );
                                    }
                                    LlmChoice::None => {}
                                }
                                on_next.call(());
                            }
                        },
                        "다음"
                    }
                }
            }
        }
    }
}

// ── Step 3: Telegram ─────────────────────────────────────────────────────────

#[component]
fn StepTelegram(on_next: EventHandler) -> Element {
    let mut want_telegram = use_signal(|| false);
    let mut bot_token = use_signal(String::new);
    let mut chat_id = use_signal(String::new);
    let mut fetching_id = use_signal(|| false);
    let mut saved = use_signal(|| false);

    rsx! {
        div {
            style: "position: fixed; inset: 0; background: #F5F3F0; display: flex; align-items: center; justify-content: center; font-family: 'Inter', -apple-system, BlinkMacSystemFont, sans-serif;",

            div {
                style: "background: #FFFFFF; border: 1px solid #E7E5E4; border-radius: 14px; padding: 48px 56px; max-width: 520px; width: 100%; box-shadow: 0 4px 24px rgba(0,0,0,0.06);",

                // Header
                div { style: "text-align: center; margin-bottom: 32px;",
                    div { style: "font-size: 40px; margin-bottom: 12px;", "📱" }
                    h2 {
                        style: "font-family: 'Fraunces', Georgia, serif; font-size: 26px; font-weight: 700; color: #1C1917; margin: 0 0 8px 0;",
                        "텔레그램을 연결할까요?"
                    }
                    p {
                        style: "font-size: 14px; color: #78716C; margin: 0; line-height: 1.5;",
                        "스킬 실행 결과를 텔레그램으로 받아보세요"
                    }
                }

                if !want_telegram() {
                    // Choice buttons
                    div { style: "display: flex; flex-direction: column; gap: 12px;",
                        button {
                            style: "padding: 16px 20px; background: #F5F3F0; border: 1px solid #E7E5E4; border-radius: 10px; cursor: pointer; text-align: left; font-size: 14px; color: #1C1917;",
                            onclick: move |_| want_telegram.set(true),
                            div { style: "font-weight: 600; margin-bottom: 4px;", "네, 연결할게요" }
                            div { style: "font-size: 12px; color: #78716C;", "BotFather에서 봇을 만들고 토큰을 입력합니다" }
                        }
                        button {
                            style: "padding: 16px 20px; background: #F5F3F0; border: 1px solid #E7E5E4; border-radius: 10px; cursor: pointer; text-align: left; font-size: 14px; color: #78716C;",
                            onclick: move |_| on_next.call(()),
                            div { style: "font-weight: 600; margin-bottom: 4px;", "나중에 할게요" }
                            div { style: "font-size: 12px;", "설정에서 언제든 연결할 수 있어요" }
                        }
                    }
                } else {
                    // Telegram setup guide (compact)
                    div {
                        style: "background: #FFFBEB; border: 1px solid #FDE68A; border-radius: 8px; padding: 14px; margin-bottom: 20px; font-size: 13px; color: #92400E; line-height: 1.6;",
                        ol { style: "margin: 0; padding-left: 20px;",
                            li { "텔레그램에서 " strong { "@BotFather" } " → " strong { "/newbot" } }
                            li { "발급된 토큰을 아래에 붙여넣기" }
                        }
                    }

                    div { style: "display: flex; flex-direction: column; gap: 12px; margin-bottom: 20px;",
                        // 봇 토큰
                        div {
                            label { style: "font-size: 12px; font-weight: 600; color: #78716C; display: block; margin-bottom: 4px;", "봇 토큰" }
                            input {
                                style: "width: 100%; padding: 10px 12px; border: 1px solid #E7E5E4; border-radius: 6px; font-size: 13px; font-family: monospace; outline: none; box-sizing: border-box;",
                                placeholder: "1234567890:ABCdefGHIjklMNOpqrSTUvwxYZ",
                                value: "{bot_token}",
                                oninput: move |e| bot_token.set(e.value()),
                            }
                            if !bot_token.read().is_empty() {
                                p {
                                    style: "font-size: 12px; color: #2563eb; margin: 8px 0 0; line-height: 1.5;",
                                    "👉 텔레그램에서 만든 봇에게 아무 메시지를 하나 보낸 후, 아래 버튼을 눌러주세요"
                                }
                            }
                        }
                        // 채팅 ID
                        div {
                            label { style: "font-size: 12px; font-weight: 600; color: #78716C; display: block; margin-bottom: 4px;", "채팅 ID" }
                            div { style: "display: flex; gap: 8px;",
                                input {
                                    style: "flex: 1; padding: 10px 12px; border: 1px solid #E7E5E4; border-radius: 6px; font-size: 13px; font-family: monospace; outline: none; box-sizing: border-box;",
                                    placeholder: "자동으로 가져옵니다",
                                    value: "{chat_id}",
                                    oninput: move |e| chat_id.set(e.value()),
                                }
                                {
                                    let is_fetching = *fetching_id.read();
                                    let btn_bg = if is_fetching { "#94a3b8" } else { "#2563eb" };
                                    rsx! {
                                        button {
                                            style: "padding: 10px 14px; background: {btn_bg}; color: #fff; border: none; border-radius: 6px; font-size: 12px; font-weight: 600; cursor: pointer; white-space: nowrap; min-width: 120px;",
                                            disabled: bot_token.read().is_empty() || is_fetching,
                                            onclick: move |_| {
                                                let token = bot_token.read().clone();
                                                fetching_id.set(true);
                                                spawn(async move {
                                                    match kittypaw_core::telegram::fetch_chat_id(&token).await {
                                                        Ok(id) => chat_id.set(id),
                                                        Err(e) => chat_id.set(format!("오류: {e}")),
                                                    }
                                                    fetching_id.set(false);
                                                });
                                            },
                                            if is_fetching { "가져오는 중..." } else { "채팅 ID 가져오기" }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    div { style: "display: flex; justify-content: flex-end; gap: 8px;",
                        button {
                            style: "padding: 10px 20px; background: transparent; color: #78716C; border: 1px solid #E7E5E4; border-radius: 6px; font-size: 13px; cursor: pointer;",
                            onclick: move |_| on_next.call(()),
                            "건너뛰기"
                        }
                        button {
                            style: "padding: 10px 24px; background: #1C1917; color: #F5F3F0; border: none; border-radius: 6px; font-size: 13px; font-weight: 600; cursor: pointer;",
                            disabled: bot_token.read().is_empty() || chat_id.read().is_empty(),
                            onclick: move |_| {
                                let token = bot_token.read().clone();
                                let cid = chat_id.read().clone();
                                let _ = kittypaw_core::secrets::set_secret("telegram", "bot_token", &token);
                                let _ = kittypaw_core::secrets::set_secret("telegram", "chat_id", &cid);
                                saved.set(true);
                                on_next.call(());
                            },
                            if *saved.read() { "저장 완료" } else { "저장 후 다음" }
                        }
                    }
                }
            }
        }
    }
}

// ── Step 4: Complete ──────────────────────────────────────────────────────────

#[component]
fn StepComplete(on_complete: EventHandler) -> Element {
    let app_state = use_context::<AppState>();

    rsx! {
        div {
            style: "position: fixed; inset: 0; background: #F5F3F0; display: flex; align-items: center; justify-content: center; font-family: 'Inter', -apple-system, BlinkMacSystemFont, sans-serif;",

            div {
                style: "background: #FFFFFF; border: 1px solid #E7E5E4; border-radius: 14px; padding: 56px 64px; max-width: 480px; width: 100%; text-align: center; box-shadow: 0 4px 24px rgba(0,0,0,0.06);",

                div { style: "font-size: 48px; margin-bottom: 20px;", "" }

                h2 {
                    style: "font-family: 'Fraunces', Georgia, serif; font-size: 32px; font-weight: 700; color: #1C1917; margin: 0 0 12px 0;",
                    "준비 완료!"
                }

                p {
                    style: "font-size: 15px; color: #78716C; margin: 0 0 40px 0; line-height: 1.6;",
                    "채팅에서 자유롭게 대화하거나, 스킬을 설치하고 실행해보세요"
                }

                button {
                    style: "padding: 14px 40px; background: #86EFAC; color: #166534; border: none; border-radius: 6px; font-size: 15px; font-weight: 600; cursor: pointer;",
                    onclick: move |_| {
                        let store = app_state.store.clone();
                        spawn(async move {
                            let s = store.lock().await;
                            let _ = s.set_user_context("onboarding_completed", "true", "system");
                        });
                        on_complete.call(());
                    },
                    "시작하기"
                }
            }
        }
    }
}
