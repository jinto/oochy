use std::sync::{Arc, Mutex};

use kittypaw_core::capability::CapabilityChecker;
use kittypaw_core::error::{KittypawError, Result};
use kittypaw_core::permission::{PermissionDecision, PermissionRequest};
use kittypaw_core::types::{
    now_timestamp, AgentState, ConversationTurn, Event, EventType, ExecutionResult, LlmMessage,
    Role,
};
use kittypaw_llm::provider::LlmProvider;
use kittypaw_sandbox::sandbox::Sandbox;
use tracing::{info_span, Instrument};

use kittypaw_store::Store;

const SYSTEM_PROMPT: &str = r#"You are KittyPaw, an AI agent that helps users by writing JavaScript (ES2020) code.

## How you work
1. You receive an event (message, command, etc.)
2. You write JavaScript code to handle it
3. Your code is executed in a QuickJS sandbox
4. The result is returned to the user

## Rules
- Write ONLY valid JavaScript (ES2020) code. No markdown fences, no explanations.
- Use the available skill globals to interact with the outside world.
- Skill methods are synchronous — you can call them directly or use `await`.
- Your code runs inside an async function — `await` is available.
- Use `return` to send a value back as the response.
- Keep your code minimal and focused on the task.
- Handle errors with try/catch.
- Do NOT use: require(), import, fetch(), Node.js APIs, top-level await.

## Available Skills
- Telegram.sendMessage(chatId, text) — Send a message via Telegram
- Telegram.sendPhoto(chatId, url) — Send a photo
- Telegram.editMessage(chatId, messageId, text) — Edit a message
- Http.get(url) — HTTP GET request
- Http.post(url, body) — HTTP POST request
- Http.put(url, body) — HTTP PUT request
- Http.delete(url) — HTTP DELETE request
- Storage.get(key) — Read from persistent storage
- Storage.set(key, value) — Write to persistent storage
- Storage.delete(key) — Delete from storage
- Storage.list() — List all storage keys
- console.log(...args) — Log output (for debugging)
"#;

const MAX_RETRIES: usize = 3;

pub async fn run_agent_loop(
    event: Event,
    provider: &dyn LlmProvider,
    sandbox: &Sandbox,
    store: Arc<Mutex<Store>>,
    config: &kittypaw_core::config::Config,
    on_token: Option<std::sync::Arc<dyn Fn(String) + Send + Sync>>,
    // TODO: wire permission checks to sandbox file operations
    _on_permission_request: Option<
        Arc<
            dyn Fn(PermissionRequest) -> tokio::sync::oneshot::Receiver<PermissionDecision>
                + Send
                + Sync,
        >,
    >,
) -> Result<String> {
    let agent_id = agent_id_for_event(&event);

    // Load or create agent state — ensure agent exists in DB before adding turns.
    let mut state = {
        let s = store.lock().unwrap();
        match s.load_state(&agent_id)? {
            Some(existing) => existing,
            None => {
                let new_state = AgentState::new(&agent_id, SYSTEM_PROMPT);
                s.save_state(&new_state)?;
                new_state
            }
        }
    };

    // Build prompt messages
    let event_text = format_event(&event);
    let messages = build_prompt(&state, &event_text);

    // Add user turn
    let user_turn = ConversationTurn {
        role: Role::User,
        content: event_text.clone(),
        code: None,
        result: None,
        timestamp: now_timestamp(),
    };
    state.add_turn(user_turn.clone());
    {
        let s = store.lock().unwrap();
        s.add_turn(&agent_id, &user_turn)?;
    }

    // Generate code with retry loop
    let mut last_error: Option<String> = None;
    let mut retry_messages = messages.clone();

    for attempt in 0..MAX_RETRIES {
        if attempt > 0 {
            tracing::info!("Retry attempt {attempt}/{MAX_RETRIES}");
        }

        // If we had an error, append it as feedback
        if let Some(ref err) = last_error {
            retry_messages.push(LlmMessage {
                role: Role::User,
                content: format!(
                    "Your previous code had an error:\n{err}\n\nPlease fix the code and try again."
                ),
            });
        }

        // Call LLM
        let code = if let Some(ref cb) = on_token {
            provider
                .generate_stream(&retry_messages, cb.clone())
                .instrument(info_span!("llm_generate"))
                .await?
        } else {
            provider
                .generate(&retry_messages)
                .instrument(info_span!("llm_generate"))
                .await?
        };
        tracing::debug!("Generated JS ({} chars)", code.len());

        // Execute in sandbox
        let context = serde_json::json!({
            "event": event.payload,
            "event_type": format!("{:?}", event.event_type),
            "agent_id": agent_id,
        });

        // Build a SkillResolver so JS skill stubs return real data
        // (Http responses, Storage values, Llm outputs) instead of "null".
        //
        // Build a CapabilityChecker from the matching agent config.
        // If no agent config matches, the checker is None (permissive mode).
        let checker: Option<Arc<Mutex<CapabilityChecker>>> = {
            let agent_config = config.agents.iter().find(|a| {
                a.id == agent_id
                    || (agent_id.starts_with("telegram-")
                        && a.channels.iter().any(|c| c == "telegram"))
                    || (agent_id.starts_with("web-") && a.channels.iter().any(|c| c == "web"))
                    || (agent_id.starts_with("desktop-")
                        && a.channels.iter().any(|c| c == "desktop"))
            });
            agent_config.map(|ac| Arc::new(Mutex::new(CapabilityChecker::from_agent_config(ac))))
        };

        let store_for_resolver = Arc::clone(&store);
        let config_for_resolver = Arc::new(config.clone());
        let checker_for_resolver = checker.clone();
        let skill_resolver: Option<kittypaw_sandbox::SkillResolver> =
            Some(Arc::new(move |call: kittypaw_core::types::SkillCall| {
                let store = Arc::clone(&store_for_resolver);
                let config = Arc::clone(&config_for_resolver);
                let checker = checker_for_resolver.clone();
                Box::pin(async move {
                    crate::skill_executor::resolve_skill_call(
                        &call,
                        &config,
                        &store,
                        checker.as_ref(),
                    )
                    .await
                })
            }));

        let exec_result = sandbox
            .execute_with_resolver(&code, context, skill_resolver)
            .instrument(info_span!("sandbox_execute"))
            .await?;

        if exec_result.success {
            // Skill calls were already executed inline by the resolver during
            // sandbox execution. Log any that were captured for observability.
            if !exec_result.skill_calls.is_empty() {
                tracing::info!(
                    "{} skill calls resolved inline during execution",
                    exec_result.skill_calls.len()
                );
            }

            let output = if exec_result.output.is_empty() {
                "(no output)".to_string()
            } else {
                exec_result.output.clone()
            };

            let assistant_turn = ConversationTurn {
                role: Role::Assistant,
                content: output.clone(),
                code: Some(code),
                result: Some(format_exec_result(&exec_result)),
                timestamp: now_timestamp(),
            };
            state.add_turn(assistant_turn.clone());
            {
                let s = store.lock().unwrap();
                s.add_turn(&agent_id, &assistant_turn)?;
                s.save_state(&state)?;
            }

            return Ok(output);
        }

        // Error — retry with feedback
        let err_msg = exec_result.error.unwrap_or("unknown error".into());
        tracing::warn!("Execution error (attempt {attempt}): {err_msg}");
        last_error = Some(err_msg);

        // Add the failed attempt as assistant message for context
        retry_messages.push(LlmMessage {
            role: Role::Assistant,
            content: code,
        });
    }

    // All retries exhausted
    let err_msg = last_error.unwrap_or("unknown error".into());
    let assistant_turn = ConversationTurn {
        role: Role::Assistant,
        content: format!("Error after {MAX_RETRIES} retries: {err_msg}"),
        code: None,
        result: None,
        timestamp: now_timestamp(),
    };
    state.add_turn(assistant_turn.clone());
    {
        let s = store.lock().unwrap();
        s.add_turn(&agent_id, &assistant_turn)?;
        s.save_state(&state)?;
    }

    Err(KittypawError::Sandbox(format!(
        "Code execution failed after {MAX_RETRIES} retries: {err_msg}"
    )))
}

fn build_prompt(state: &AgentState, event_text: &str) -> Vec<LlmMessage> {
    let mut messages = vec![LlmMessage {
        role: Role::System,
        content: SYSTEM_PROMPT.to_string(),
    }];

    // Add conversation history (last 20 turns)
    for turn in state.recent_turns(20) {
        match turn.role {
            Role::User => {
                let mut content = turn.content.clone();
                if let Some(ref result) = turn.result {
                    content.push_str(&format!("\n[Previous result: {result}]"));
                }
                messages.push(LlmMessage {
                    role: Role::User,
                    content,
                });
            }
            Role::Assistant => {
                messages.push(LlmMessage {
                    role: Role::Assistant,
                    content: turn.code.clone().unwrap_or(turn.content.clone()),
                });
            }
            Role::System => {}
        }
    }

    // Current event
    messages.push(LlmMessage {
        role: Role::User,
        content: event_text.to_string(),
    });

    messages
}

fn format_event(event: &Event) -> String {
    let payload = &event.payload;
    match event.event_type {
        EventType::Telegram => {
            let user = payload
                .get("from_name")
                .and_then(|v| v.as_str())
                .unwrap_or("User");
            let text = payload.get("text").and_then(|v| v.as_str()).unwrap_or("");
            let chat_id = payload
                .get("chat_id")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            format!("[Telegram] {user} (chat_id={chat_id}): {text}")
        }
        EventType::WebChat => {
            let text = payload.get("text").and_then(|v| v.as_str()).unwrap_or("");
            let session = payload
                .get("session_id")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            format!("[WebChat] (session={session}): {text}")
        }
        EventType::Desktop => {
            let text = payload.get("text").and_then(|v| v.as_str()).unwrap_or("");
            let workspace = payload
                .get("workspace_id")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            format!("[Desktop] (workspace={workspace}): {text}")
        }
    }
}

fn format_exec_result(result: &ExecutionResult) -> String {
    let mut parts = vec![format!("output: {}", result.output)];
    if !result.skill_calls.is_empty() {
        let calls: Vec<String> = result
            .skill_calls
            .iter()
            .map(|c| format!("{}.{}({:?})", c.skill_name, c.method, c.args))
            .collect();
        parts.push(format!("skill_calls: [{}]", calls.join(", ")));
    }
    parts.join("; ")
}

fn agent_id_for_event(event: &Event) -> String {
    match event.event_type {
        EventType::Telegram => {
            let chat_id = event
                .payload
                .get("chat_id")
                .map(|v| {
                    v.as_str()
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| v.to_string())
                })
                .unwrap_or_else(|| "default".to_string());
            format!("telegram-{chat_id}")
        }
        EventType::WebChat => {
            let session = event
                .payload
                .get("session_id")
                .and_then(|v| v.as_str())
                .unwrap_or("default");
            format!("web-{session}")
        }
        EventType::Desktop => {
            let workspace = event
                .payload
                .get("workspace_id")
                .and_then(|v| v.as_str())
                .unwrap_or("default");
            format!("desktop-{workspace}")
        }
    }
}
