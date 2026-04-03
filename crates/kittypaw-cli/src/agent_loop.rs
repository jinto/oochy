use std::sync::Arc;
use tokio::sync::Mutex;

use kittypaw_core::capability::CapabilityChecker;
use kittypaw_core::error::{KittypawError, Result};
use kittypaw_core::permission::{PermissionDecision, PermissionRequest};
use kittypaw_core::types::{
    now_timestamp, AgentState, ConversationTurn, Event, EventType, ExecutionResult, LlmMessage,
    LoopPhase, Role, TransitionReason,
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
    on_permission_request: Option<
        Arc<
            dyn Fn(PermissionRequest) -> tokio::sync::oneshot::Receiver<PermissionDecision>
                + Send
                + Sync,
        >,
    >,
) -> Result<String> {
    let agent_id = match event.event_type {
        EventType::Telegram => format!("telegram-{}", event.session_id()),
        EventType::WebChat => format!("web-{}", event.session_id()),
        EventType::Desktop => format!("desktop-{}", event.session_id()),
    };

    // Load or create agent state — ensure agent exists in DB before adding turns.
    let mut state = {
        let s = store.lock().await;
        match s.load_state(&agent_id)? {
            Some(existing) => existing,
            None => {
                let new_state = AgentState::new(&agent_id, SYSTEM_PROMPT);
                s.save_state(&new_state)?;
                new_state
            }
        }
    };

    let reason = TransitionReason::StateReady;
    tracing::info!(
        phase = ?LoopPhase::Init,
        agent_id = %agent_id,
        transition = ?reason,
        "agent state ready"
    );

    // Build event text and persist user turn
    let event_text = format_event(&event);

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
        let s = store.lock().await;
        s.add_turn(&agent_id, &user_turn)?;
    }

    // Generate code with retry loop — each attempt uses progressively tighter compaction
    let mut last_error: Option<String> = None;

    for attempt in 0..MAX_RETRIES {
        if attempt > 0 {
            tracing::info!("Retry attempt {attempt}/{MAX_RETRIES}");
        }

        // Build prompt fresh for this attempt with the appropriate compaction level.
        // Feature flags gate both progressive retry and full 3-stage compaction.
        let compaction = if !config.features.context_compaction {
            // context_compaction disabled: use simple recent-only window (no middle/old stages)
            crate::compaction::CompactionConfig {
                recent_window: 20,
                middle_window: 0,
                truncate_len: 100,
            }
        } else if !config.features.progressive_retry {
            // progressive_retry disabled: always use the default (attempt-0) compaction
            crate::compaction::CompactionConfig::default()
        } else {
            crate::compaction::compaction_for_attempt(attempt)
        };
        let mut messages = build_prompt(&state, &event_text, &compaction);
        let reason = TransitionReason::PromptBuilt {
            message_count: messages.len(),
        };
        tracing::info!(
            phase = ?LoopPhase::Prompt,
            attempt,
            recent_window = compaction.recent_window,
            transition = ?reason,
            "prompt built with compaction"
        );

        // Proactive token budget check - skip LLM call if prompt is too large
        let est_tokens: usize = messages
            .iter()
            .map(|m| crate::compaction::estimate_tokens(&m.content))
            .sum();
        const TOKEN_BUDGET: usize = 8_000;
        if est_tokens > TOKEN_BUDGET && attempt < MAX_RETRIES - 1 {
            tracing::warn!(
                est_tokens,
                budget = TOKEN_BUDGET,
                attempt,
                "Prompt exceeds token budget, applying tighter compaction"
            );
            last_error = Some(format!(
                "Estimated {est_tokens} tokens exceeds budget {TOKEN_BUDGET}"
            ));
            continue;
        }

        // If we had an error, append it as feedback
        if let Some(ref err) = last_error {
            messages.push(LlmMessage {
                role: Role::User,
                content: format!(
                    "Your previous code had an error:\n{err}\n\nPlease fix the code and try again."
                ),
            });
        }

        // Call LLM
        let llm_result = if let Some(ref cb) = on_token {
            provider
                .generate_stream(&messages, cb.clone())
                .instrument(info_span!("llm_generate"))
                .await
        } else {
            provider
                .generate(&messages)
                .instrument(info_span!("llm_generate"))
                .await
        };

        let code = match llm_result {
            Ok(c) => c,
            Err(kittypaw_core::error::KittypawError::Llm {
                kind: kittypaw_core::error::LlmErrorKind::TokenLimit,
                ref message,
            }) => {
                last_error = Some(message.clone());
                if attempt >= MAX_RETRIES - 1 {
                    tracing::warn!(
                        attempt,
                        "Token limit at maximum compaction, giving up: {message}"
                    );
                    break;
                }
                tracing::warn!(
                    attempt,
                    "Token limit hit, retrying with tighter compaction: {message}"
                );
                continue;
            }
            Err(kittypaw_core::error::KittypawError::Llm {
                kind: kittypaw_core::error::LlmErrorKind::RateLimit,
                ref message,
            }) => {
                tracing::warn!(
                    attempt,
                    "Rate limit hit, sleeping 2s then retrying same prompt: {message}"
                );
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                // Keep last_error so retry-exhausted path reports the real reason
                last_error = Some(message.clone());
                continue;
            }
            Err(kittypaw_core::error::KittypawError::Llm { ref message, .. }) => {
                tracing::error!(attempt, "LLM error (non-retryable): {message}");
                return Err(kittypaw_core::error::KittypawError::Llm {
                    kind: kittypaw_core::error::LlmErrorKind::Other,
                    message: message.clone(),
                });
            }
            Err(e) => return Err(e),
        };
        tracing::debug!("Generated JS ({} chars)", code.len());
        let reason = TransitionReason::CodeGenerated {
            code_len: code.len(),
        };
        tracing::info!(
            phase = ?LoopPhase::Generate,
            agent_id = %agent_id,
            attempt,
            transition = ?reason,
            "code generated"
        );

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
        let checker: Option<Arc<std::sync::Mutex<CapabilityChecker>>> = {
            let agent_config = config.agents.iter().find(|a| {
                a.id == agent_id
                    || (agent_id.starts_with("telegram-")
                        && a.channels.iter().any(|c| c == "telegram"))
                    || (agent_id.starts_with("web-") && a.channels.iter().any(|c| c == "web"))
                    || (agent_id.starts_with("desktop-")
                        && a.channels.iter().any(|c| c == "desktop"))
            });
            agent_config.map(|ac| {
                Arc::new(std::sync::Mutex::new(CapabilityChecker::from_agent_config(
                    ac,
                )))
            })
        };

        let store_for_resolver = Arc::clone(&store);
        let config_for_resolver = Arc::new(config.clone());
        let checker_for_resolver = checker.clone();
        let permission_for_resolver = on_permission_request.clone();
        let skill_resolver: Option<kittypaw_sandbox::SkillResolver> =
            Some(Arc::new(move |call: kittypaw_core::types::SkillCall| {
                let store = Arc::clone(&store_for_resolver);
                let config = Arc::clone(&config_for_resolver);
                let checker = checker_for_resolver.clone();
                let on_perm = permission_for_resolver.clone();
                Box::pin(async move {
                    let perm_ref = on_perm
                        .as_ref()
                        .map(|p| p as &crate::skill_executor::PermissionCallback);
                    crate::skill_executor::resolve_skill_call(
                        &call,
                        &config,
                        &store,
                        checker.as_ref(),
                        perm_ref,
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

            let reason = TransitionReason::ExecutionSuccess {
                output_len: output.len(),
                skill_calls: exec_result.skill_calls.len(),
            };
            tracing::info!(
                phase = ?LoopPhase::Finish,
                agent_id = %agent_id,
                transition = ?reason,
                "execution success"
            );

            let assistant_turn = ConversationTurn {
                role: Role::Assistant,
                content: output.clone(),
                code: Some(code),
                result: Some(format_exec_result(&exec_result)),
                timestamp: now_timestamp(),
            };
            state.add_turn(assistant_turn.clone());
            {
                let s = store.lock().await;
                s.add_turn(&agent_id, &assistant_turn)?;
                s.save_state(&state)?;
            }

            return Ok(output);
        }

        // Error — retry with feedback
        let err_msg = exec_result.error.unwrap_or("unknown error".into());
        tracing::warn!("Execution error (attempt {attempt}): {err_msg}");
        let reason = TransitionReason::ExecutionFailed {
            error: err_msg.clone(),
            attempt,
        };
        tracing::info!(
            phase = ?LoopPhase::Retry,
            agent_id = %agent_id,
            transition = ?reason,
            "execution failed, retrying"
        );
        last_error = Some(err_msg);
    }

    // All retries exhausted
    let err_msg = last_error.unwrap_or("unknown error".into());
    let reason = TransitionReason::RetriesExhausted {
        error: err_msg.clone(),
    };
    tracing::info!(
        phase = ?LoopPhase::Finish,
        agent_id = %agent_id,
        transition = ?reason,
        "retries exhausted"
    );
    let assistant_turn = ConversationTurn {
        role: Role::Assistant,
        content: format!("Error after {MAX_RETRIES} retries: {err_msg}"),
        code: None,
        result: None,
        timestamp: now_timestamp(),
    };
    state.add_turn(assistant_turn.clone());
    {
        let s = store.lock().await;
        s.add_turn(&agent_id, &assistant_turn)?;
        s.save_state(&state)?;
    }

    Err(KittypawError::Sandbox(format!(
        "Code execution failed after {MAX_RETRIES} retries: {err_msg}"
    )))
}

fn build_prompt(
    state: &AgentState,
    event_text: &str,
    config: &crate::compaction::CompactionConfig,
) -> Vec<LlmMessage> {
    use crate::compaction::{compact_turns, CompactionMode};

    let mut messages = vec![LlmMessage {
        role: Role::System,
        content: SYSTEM_PROMPT.to_string(),
    }];

    // Add compacted conversation history (3-stage: summary / truncated / full)
    let compacted = compact_turns(&state.turns, config, &CompactionMode::AgentLoop);
    messages.extend(compacted);

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
