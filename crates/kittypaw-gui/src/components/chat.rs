use dioxus::prelude::*;
use futures_util::StreamExt;
use kittypaw_cli::assistant::{run_assistant_turn, AssistantContext};
use kittypaw_core::config::Config;
use kittypaw_core::types::{Event, EventType};
use kittypaw_sandbox::sandbox::Sandbox;

use crate::state::AppState;

#[component]
pub fn ChatPanel() -> Element {
    let app_state = use_context::<AppState>();
    let mut messages = use_signal::<Vec<(String, String)>>(Vec::new);
    let mut input_text = use_signal(String::new);
    let mut is_loading = use_signal(|| false);
    let mut is_recording = use_signal(|| false);

    let chat_coroutine = use_coroutine(move |mut rx: UnboundedReceiver<String>| {
        let state = app_state.clone();
        async move {
            while let Some(user_msg) = rx.next().await {
                is_loading.set(true);

                // Get provider from registry
                let provider = {
                    let registry = state.llm_registry.lock().unwrap();
                    registry.default_provider()
                };
                let provider = match provider {
                    Some(p) => p,
                    None => {
                        messages.write().push((
                            "assistant".into(),
                            "No LLM configured. Please set your API key in Settings.".into(),
                        ));
                        is_loading.set(false);
                        continue;
                    }
                };

                // Construct a Desktop Event from the user message
                let event = Event {
                    event_type: EventType::Desktop,
                    payload: serde_json::json!({ "text": user_msg }),
                };

                let config = Config::load().unwrap_or_default();
                let sandbox = Sandbox::new_threaded(config.sandbox.clone());
                let assistant_ctx = AssistantContext {
                    event: &event,
                    provider: provider.as_ref(),
                    store: state.store.clone(),
                    registry_entries: &[],
                    sandbox: &sandbox,
                    config: &config,
                    on_token: None,
                };
                match run_assistant_turn(&assistant_ctx).await {
                    Ok(turn) => {
                        messages
                            .write()
                            .push(("assistant".into(), turn.response_text));
                    }
                    Err(e) => {
                        messages
                            .write()
                            .push(("assistant".into(), format!("Error: {e}")));
                    }
                }

                is_loading.set(false);
                // Refocus input after response
                document::eval(r#"document.getElementById('chat-input')?.focus()"#);
            }
        }
    });

    let mut send_message = move || {
        let msg = input_text.read().clone();
        if msg.is_empty() || *is_loading.read() {
            return;
        }
        messages.write().push(("user".into(), msg.clone()));
        input_text.set(String::new());
        chat_coroutine.send(msg);
    };

    rsx! {
        div { style: "flex: 1; display: flex; flex-direction: column; overflow: hidden;",

            // Messages area
            div { style: "flex: 1; overflow-y: auto; padding: 20px 24px;",
                if messages.read().is_empty() {
                    div { style: "display: flex; flex-direction: column; align-items: center; justify-content: center; height: 100%; text-align: center;",
                        h1 { style: "font-size: 24px; font-weight: 600; color: #1e293b; margin: 0 0 10px;",
                            "무엇을 도와드릴까요?"
                        }
                        p { style: "font-size: 15px; color: #64748b; margin-bottom: 24px;",
                            "KittyPaw는 당신의 AI 에이전트입니다."
                        }

                        div { style: "display: flex; flex-wrap: wrap; gap: 10px; justify-content: center; max-width: 480px;",
                            QuickPrompt {
                                label: "🐱 너는 누구니?",
                                prompt: "너는 누구니? 어떤 에이전트인지 소개해줘.",
                                on_click: move |msg: String| {
                                    messages.write().push(("user".into(), msg.clone()));
                                    chat_coroutine.send(msg);
                                },
                            }
                            QuickPrompt {
                                label: "🛠 어떤 일을 할 수 있어?",
                                prompt: "너는 어떤 일을 할 수 있어? 구체적으로 알려줘.",
                                on_click: move |msg: String| {
                                    messages.write().push(("user".into(), msg.clone()));
                                    chat_coroutine.send(msg);
                                },
                            }
                            QuickPrompt {
                                label: "📋 지금 무슨 일을 하고 있어?",
                                prompt: "지금 어떤 스킬들이 등록되어 있고, 무슨 일을 하고 있어?",
                                on_click: move |msg: String| {
                                    messages.write().push(("user".into(), msg.clone()));
                                    chat_coroutine.send(msg);
                                },
                            }
                            QuickPrompt {
                                label: "✨ 새 스킬 만들어줘",
                                prompt: "새로운 스킬을 만들고 싶어. 어떻게 시작하면 돼?",
                                on_click: move |msg: String| {
                                    messages.write().push(("user".into(), msg.clone()));
                                    chat_coroutine.send(msg);
                                },
                            }
                        }
                    }
                } else {
                    for (i, (role, content)) in messages.read().iter().enumerate() {
                        ChatMessage { key: "{i}", role: role.clone(), content: content.clone() }
                    }
                    if *is_loading.read() {
                        div { style: "display: flex; align-items: center; gap: 8px; color: #64748b; font-size: 13px; padding: 8px 0;",
                            "KittyPaw is thinking..."
                        }
                    }
                }
            }

            // Input area
            div { style: "padding: 12px 16px; border-top: 1px solid #e2e8f0;",
                div { style: "display: flex; gap: 8px;",
                    input {
                        id: "chat-input",
                        style: "flex: 1; padding: 10px 14px; border: 1px solid #d1d5db; border-radius: 10px; font-size: 14px; outline: none;",
                        placeholder: "Message KittyPaw...",
                        value: "{input_text}",
                        autofocus: true,
                        oninput: move |e| input_text.set(e.value()),
                        onkeypress: move |e| {
                            if e.key() == Key::Enter {
                                send_message();
                            }
                        },
                    }
                    {
                        let recording = *is_recording.read();
                        let mic_bg = if recording { "#ef4444" } else { "#f1f5f9" };
                        let mic_label = if recording { "⏹" } else { "🎤" };
                        rsx! {
                            button {
                                style: "padding: 10px 12px; background: {mic_bg}; color: #1e293b; border: 1px solid #d1d5db; border-radius: 10px; cursor: pointer; font-size: 16px;",
                                onclick: move |_| {
                                    let cur = *is_recording.read();
                                    if cur {
                                        // Stop recording
                                        is_recording.set(false);
                                        document::eval(r#"
                                            if (window._kpRecognition) {
                                                window._kpRecognition.stop();
                                            }
                                        "#);
                                    } else {
                                        // Start recording
                                        is_recording.set(true);
                                        document::eval(r#"
                                            try {
                                                const SR = window.SpeechRecognition || window.webkitSpeechRecognition;
                                                if (!SR) { throw new Error('not supported'); }
                                                const recognition = new SR();
                                                recognition.lang = navigator.language || 'ko-KR';
                                                recognition.interimResults = false;
                                                recognition.maxAlternatives = 1;
                                                window._kpRecognition = recognition;
                                                recognition.onresult = (e) => {
                                                    const text = e.results[0][0].transcript;
                                                    const input = document.getElementById('chat-input');
                                                    if (input) {
                                                        const nativeSet = Object.getOwnPropertyDescriptor(
                                                            window.HTMLInputElement.prototype, 'value'
                                                        ).set;
                                                        nativeSet.call(input, (input.value ? input.value + ' ' : '') + text);
                                                        input.dispatchEvent(new Event('input', { bubbles: true }));
                                                    }
                                                };
                                                recognition.onerror = () => {};
                                                recognition.onend = () => {
                                                    window._kpRecognition = null;
                                                };
                                                recognition.start();
                                            } catch(e) {
                                                // Speech API not available in this WebView
                                            }
                                        "#);
                                    }
                                },
                                "{mic_label}"
                            }
                        }
                    }
                    {
                        let loading = *is_loading.read();
                        let btn_bg = if loading { "#94a3b8" } else { "#2563eb" };
                        let btn_label = if loading { "..." } else { "Send" };
                        rsx! {
                            button {
                                style: "padding: 10px 16px; background: {btn_bg}; color: #fff; border: none; border-radius: 10px; cursor: pointer; font-size: 14px;",
                                disabled: loading,
                                onclick: move |_| {
                                    send_message();
                                },
                                "{btn_label}"
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn ChatMessage(role: String, content: String) -> Element {
    let is_user = role == "user";
    let bg = if is_user { "#f1f5f9" } else { "#fff" };
    let align = if is_user { "flex-end" } else { "flex-start" };
    let label = if is_user { "You" } else { "KittyPaw" };
    let label_color = if is_user { "#64748b" } else { "#2563eb" };

    rsx! {
        div { style: "display: flex; flex-direction: column; align-items: {align}; margin-bottom: 16px;",
            span { style: "font-size: 11px; font-weight: 600; color: {label_color}; margin-bottom: 4px;", "{label}" }
            if is_user {
                div { style: "max-width: 80%; padding: 10px 14px; background: {bg}; border-radius: 12px; font-size: 14px; color: #1e293b; line-height: 1.5; white-space: pre-wrap;",
                    "{content}"
                }
            } else {
                div {
                    class: "markdown-body",
                    style: "max-width: 80%; padding: 10px 14px; background: {bg}; border-radius: 12px; font-size: 14px; color: #1e293b; line-height: 1.6;",
                    dangerous_inner_html: render_markdown(&content),
                }
            }
        }
    }
}

fn render_markdown(input: &str) -> String {
    use pulldown_cmark::{html, Options, Parser};
    let opts = Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES;
    let parser = Parser::new_ext(input, opts);
    let mut html_output = String::new();
    html::push_html(&mut html_output, parser);
    html_output
}

#[component]
fn QuickPrompt(
    label: &'static str,
    prompt: &'static str,
    on_click: EventHandler<String>,
) -> Element {
    rsx! {
        button {
            style: "padding: 10px 16px; background: #fff; border: 1px solid #e2e8f0; border-radius: 12px; cursor: pointer; font-size: 13px; color: #334155; transition: all 0.15s; box-shadow: 0 1px 2px rgba(0,0,0,0.05);",
            onclick: move |_| on_click.call(prompt.to_string()),
            "{label}"
        }
    }
}
