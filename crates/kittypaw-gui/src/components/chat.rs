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
                        let text = extract_reply_text(&turn.response_text);
                        messages.write().push(("assistant".into(), text));
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
        // Keep focus on input while thinking
        document::eval(r#"setTimeout(() => document.getElementById('chat-input')?.focus(), 50)"#);
    };

    // Auto-scroll + focus via MutationObserver (once, not every render)
    use_effect(move || {
        document::eval(
            r#"
            if (!window._kpObserver) {
                // Initial focus
                document.getElementById('chat-input')?.focus();

                // Watch for new messages and auto-scroll
                const el = document.getElementById('chat-messages');
                if (el) {
                    window._kpObserver = new MutationObserver(() => {
                        el.scrollTop = el.scrollHeight;
                    });
                    window._kpObserver.observe(el, { childList: true, subtree: true });
                }
            }
        "#,
        );
    });

    // Pre-request microphone + speech recognition permissions on mount
    use_effect(move || {
        spawn(async {
            let _ = tokio::process::Command::new("swift")
                .arg("-e")
                .arg(
                    r#"
                    import Speech; import AVFoundation; import Foundation
                    SFSpeechRecognizer.requestAuthorization { _ in }
                    AVCaptureDevice.requestAccess(for: .audio) { _ in exit(0) }
                    RunLoop.main.run(until: Date(timeIntervalSinceNow: 30))
                    "#,
                )
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .ok();
        });
    });

    // Register Cmd+Enter (send) and Cmd+R (mic) via JS on the input element
    use_effect(move || {
        document::eval(
            r#"
            const input = document.getElementById('chat-input');
            if (input && !input._kpShortcuts) {
                input._kpShortcuts = true;
                input.addEventListener('keydown', (e) => {
                    if (e.metaKey && e.key === 'Enter') {
                        e.preventDefault();
                        const sendBtn = document.getElementById('chat-send');
                        if (sendBtn && !sendBtn.disabled) sendBtn.click();
                    }
                    if (e.metaKey && e.key === 'r') {
                        e.preventDefault();
                        const micBtn = document.getElementById('chat-mic');
                        if (micBtn) micBtn.click();
                    }
                    if (e.metaKey && e.key === 'Backspace') {
                        e.preventDefault();
                        const clearBtn = document.getElementById('chat-clear');
                        if (clearBtn) clearBtn.click();
                    }
                });
            }
        "#,
        );
    });

    rsx! {

        div { style: "flex: 1; display: flex; flex-direction: column; overflow: hidden;",

            // Messages area
            div { id: "chat-messages", style: "flex: 1; overflow-y: auto; padding: 20px 24px;",
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
                div { style: "display: flex; gap: 8px; align-items: center;",
                    // Input with clear button
                    div { style: "flex: 1; position: relative;",
                        {
                            let recording = *is_recording.read();
                            let border = if recording { "2px solid #ef4444" } else { "1px solid #d1d5db" };
                            let placeholder = if recording { "🎙 듣고 있어요..." } else { "Message KittyPaw..." };
                            rsx! {
                                input {
                                    id: "chat-input",
                                    style: "width: 100%; padding: 10px 32px 10px 14px; border: {border}; border-radius: 10px; font-size: 14px; outline: none; box-sizing: border-box;",
                                    placeholder: "{placeholder}",
                            value: "{input_text}",
                            autofocus: true,
                            oninput: move |e| input_text.set(e.value()),
                            onkeypress: move |e| {
                                if e.key() == Key::Enter {
                                    send_message();
                                }
                            },
                        }
                            }
                        }
                        if !input_text.read().is_empty() {
                            button {
                                style: "position: absolute; right: 8px; top: 50%; transform: translateY(-50%); background: none; border: none; cursor: pointer; color: #94a3b8; font-size: 14px; padding: 2px 4px; line-height: 1;",
                                id: "chat-clear",
                                onclick: move |_| {
                                    input_text.set(String::new());
                                    document::eval(r#"document.getElementById('chat-input')?.focus()"#);
                                },
                                "✕"
                            }
                        }
                    }
                    // Mic button
                    {
                        let recording = *is_recording.read();
                        let mic_bg = if recording { "#ef4444" } else { "#f1f5f9" };
                        rsx! {
                            button {
                                id: "chat-mic",
                                style: "padding: 10px 12px; background: {mic_bg}; color: #1e293b; border: 1px solid #d1d5db; border-radius: 10px; cursor: pointer; font-size: 16px;",
                                title: "음성 입력 (⌘R)",
                                onclick: move |_| {
                                    let cur = *is_recording.read();
                                    if cur {
                                        is_recording.set(false);
                                    } else {
                                        is_recording.set(true);
                                        spawn(async move {
                                            match stream_transcribe(&mut input_text).await {
                                                Ok(()) => {}
                                                Err(e) => {
                                                    messages.write().push(("assistant".into(), format!("음성 입력 오류: {e}")));
                                                }
                                            }
                                            is_recording.set(false);
                                        });
                                    }
                                },
                                if recording { "⏹" } else { "🎤" }
                            }
                        }
                    }
                    // Send button
                    {
                        let loading = *is_loading.read();
                        let btn_bg = if loading { "#94a3b8" } else { "#2563eb" };
                        rsx! {
                            button {
                                id: "chat-send",
                                style: "padding: 10px 16px; background: {btn_bg}; color: #fff; border: none; border-radius: 10px; cursor: pointer; font-size: 14px;",
                                disabled: loading,
                                title: "전송 (⌘↵)",
                                onclick: move |_| {
                                    send_message();
                                },
                                if loading { "..." } else { "↵" }
                            }
                        }
                    }
                }
                // Shortcut hints
                div { style: "display: flex; justify-content: flex-end; gap: 12px; margin-top: 4px; padding-right: 4px;",
                    span { style: "font-size: 10px; color: #b0adb0;", "Enter 전송" }
                    span { style: "font-size: 10px; color: #b0adb0;", "⌘R 음성" }
                    span { style: "font-size: 10px; color: #b0adb0;", "⌘⌫ 삭제" }
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

/// Extract display text from LLM response.
/// Handles cases where response is raw JSON like `[{"action":"reply","text":"..."}]`
fn extract_reply_text(raw: &str) -> String {
    let trimmed = raw.trim();

    // Try JSON array: [{"action":"reply","text":"..."}]
    if trimmed.starts_with('[') {
        if let Ok(arr) = serde_json::from_str::<Vec<serde_json::Value>>(trimmed) {
            let texts: Vec<&str> = arr
                .iter()
                .filter_map(|v| {
                    if v.get("action")?.as_str()? == "reply" {
                        v.get("text")?.as_str()
                    } else {
                        None
                    }
                })
                .collect();
            if !texts.is_empty() {
                return texts.join("\n\n");
            }
        }
    }

    // Try single JSON object: {"action":"reply","text":"..."}
    if trimmed.starts_with('{') {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
            if v.get("action").and_then(|a| a.as_str()) == Some("reply") {
                if let Some(text) = v.get("text").and_then(|t| t.as_str()) {
                    return text.to_string();
                }
            }
        }
    }

    raw.to_string()
}

fn render_markdown(input: &str) -> String {
    use pulldown_cmark::{html, Options, Parser};
    let opts = Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES;
    let parser = Parser::new_ext(input, opts);
    let mut html_output = String::new();
    html::push_html(&mut html_output, parser);
    html_output
}

/// Stream-transcribe from microphone with real-time partial results.
/// Uses the bundled kittypaw-mic Swift helper for reliable performance.
/// Falls back to swift -e inline script if helper not found.
async fn stream_transcribe(input_text: &mut Signal<String>) -> Result<(), String> {
    use tokio::io::{AsyncBufReadExt, BufReader};

    // Try bundled helper first (compiled, fast startup)
    let mic_path = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("kittypaw-mic")));

    let mut child = if let Some(ref path) = mic_path {
        if path.exists() {
            tokio::process::Command::new(path)
                .args(["--lang", "ko-KR", "--duration", "10"])
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .ok()
        } else {
            None
        }
    } else {
        None
    };

    // Fallback: run Swift source directly
    if child.is_none() {
        let script = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../scripts/kittypaw-mic.swift"
        );
        child = tokio::process::Command::new("swift")
            .arg(script)
            .args(["--lang", "ko-KR", "--duration", "10"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .ok();
    }

    let mut child = child.ok_or("Failed to start speech recognition")?;

    let stdout = child
        .stdout
        .take()
        .ok_or("Failed to capture stdout".to_string())?;
    let mut reader = BufReader::new(stdout).lines();

    // Read partial results line by line, updating input_text in real-time
    while let Ok(Some(line)) = reader.next_line().await {
        let trimmed = line.trim().to_string();
        if !trimmed.is_empty() {
            input_text.set(trimmed);
        }
    }

    let _ = child.wait().await;
    Ok(())
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
