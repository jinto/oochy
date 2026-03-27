use oochy_core::capability::CapabilityChecker;
use oochy_core::config::AgentConfig;
use oochy_core::error::{OochyError, Result};
use oochy_core::types::{
    AgentState, ConversationTurn, Event, EventType, ExecutionResult, LlmMessage, Role, SkillCall,
};
use oochy_llm::provider::LlmProvider;
use oochy_sandbox::sandbox::Sandbox;
use tracing::{info_span, Instrument};

use crate::store::Store;

const SYSTEM_PROMPT: &str = r#"You are Oochy, an AI agent that helps users by writing JavaScript (ES2020) code.

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
    store: &Store,
    config: &oochy_core::config::Config,
) -> Result<String> {
    let agent_id = agent_id_for_event(&event);

    // Load or create agent state
    let mut state = store
        .load_state(&agent_id)?
        .unwrap_or_else(|| AgentState::new(&agent_id, SYSTEM_PROMPT));

    // Build prompt messages
    let event_text = format_event(&event);
    let messages = build_prompt(&state, &event_text);

    // Add user turn
    let user_turn = ConversationTurn {
        role: Role::User,
        content: event_text.clone(),
        code: None,
        result: None,
        timestamp: chrono_now(),
    };
    state.add_turn(user_turn.clone());
    store.add_turn(&agent_id, &user_turn)?;

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
        let code = provider
            .generate(&retry_messages)
            .instrument(info_span!("llm_generate"))
            .await?;
        tracing::debug!("Generated JS ({} chars)", code.len());

        // Execute in sandbox
        let context = serde_json::json!({
            "event": event.payload,
            "event_type": format!("{:?}", event.event_type),
            "agent_id": agent_id,
        });

        let exec_result = sandbox
            .execute(&code, context)
            .instrument(info_span!("sandbox_execute"))
            .await?;

        if exec_result.success {
            // Execute captured skill calls on the host (real API calls)
            if !exec_result.skill_calls.is_empty() {
                tracing::info!("Executing {} skill calls", exec_result.skill_calls.len());
                let allowed_calls = filter_skill_calls(&exec_result.skill_calls, &config.agents, &agent_id);
                let skill_results = crate::skill_executor::execute_skill_calls(
                    &allowed_calls,
                    config,
                )
                .instrument(info_span!("skill_execute"))
                .await;
                match &skill_results {
                    Ok(results) => {
                        for r in results {
                            if !r.success {
                                tracing::warn!("Skill {}.{} failed: {:?}", r.skill_name, r.method, r.error);
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!("Skill executor failed: {e}");
                    }
                }
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
                timestamp: chrono_now(),
            };
            state.add_turn(assistant_turn.clone());
            store.add_turn(&agent_id, &assistant_turn)?;
            store.save_state(&state)?;

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
        timestamp: chrono_now(),
    };
    state.add_turn(assistant_turn.clone());
    store.add_turn(&agent_id, &assistant_turn)?;
    store.save_state(&state)?;

    Err(OochyError::Sandbox(format!(
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
            let user = payload.get("from_name").and_then(|v| v.as_str()).unwrap_or("User");
            let text = payload.get("text").and_then(|v| v.as_str()).unwrap_or("");
            let chat_id = payload.get("chat_id").and_then(|v| v.as_str()).unwrap_or("");
            format!("[Telegram] {user} (chat_id={chat_id}): {text}")
        }
        EventType::WebChat => {
            let text = payload.get("text").and_then(|v| v.as_str()).unwrap_or("");
            let session = payload.get("session_id").and_then(|v| v.as_str()).unwrap_or("");
            format!("[WebChat] (session={session}): {text}")
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
            let chat_id = event.payload.get("chat_id")
                .map(|v| v.as_str().map(|s| s.to_string()).unwrap_or_else(|| v.to_string()))
                .unwrap_or_else(|| "default".to_string());
            format!("telegram-{chat_id}")
        }
        EventType::WebChat => {
            let session = event.payload.get("session_id").and_then(|v| v.as_str()).unwrap_or("default");
            format!("web-{session}")
        }
    }
}

/// Filter skill calls through CapabilityChecker for the matching agent config.
/// If no agent config is found (e.g. stdin mode), all calls pass through.
fn filter_skill_calls(
    calls: &[SkillCall],
    agents: &[AgentConfig],
    agent_id: &str,
) -> Vec<SkillCall> {
    // Find the agent config whose id matches or whose channels match the agent_id prefix
    let agent_config = agents.iter().find(|a| {
        a.id == agent_id
            || (agent_id.starts_with("telegram-") && a.channels.iter().any(|c| c == "telegram"))
            || (agent_id.starts_with("web-") && a.channels.iter().any(|c| c == "web"))
    });

    let Some(config) = agent_config else {
        // Default-deny: no agent config means no skill calls allowed
        if !calls.is_empty() {
            tracing::warn!("No agent config for '{}' — denying {} skill calls (default-deny)", agent_id, calls.len());
        }
        return vec![];
    };

    let mut checker = CapabilityChecker::from_agent_config(config);
    let mut allowed = Vec::new();
    for call in calls {
        match checker.check(call) {
            Ok(()) => allowed.push(call.clone()),
            Err(e) => {
                tracing::warn!("Capability check denied {}.{}: {}", call.skill_name, call.method, e);
            }
        }
    }
    allowed
}

fn chrono_now() -> String {
    // Simple ISO-8601 timestamp without chrono dependency
    let duration = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}", duration.as_secs())
}
