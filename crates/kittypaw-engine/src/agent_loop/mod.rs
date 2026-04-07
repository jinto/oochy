mod commands;
mod prompt;

use std::sync::Arc;
use tokio::sync::Mutex;

use kittypaw_core::capability::CapabilityChecker;
use kittypaw_core::error::{KittypawError, Result};
use kittypaw_core::types::{
    now_timestamp, AgentState, ConversationTurn, Event, LlmMessage, LoopPhase, Role,
    TransitionReason,
};
use kittypaw_llm::provider::{LlmProvider, TokenUsage};
use kittypaw_sandbox::sandbox::Sandbox;
use kittypaw_store::Store;
use tracing::{info_span, Instrument};

pub const SYSTEM_PROMPT: &str = r#"You are KittyPaw, an AI agent that helps users by writing JavaScript (ES2020) code.

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

{{SKILLS_SECTION}}
- console.log(...args) — Log output (for debugging)

## When to create a skill
If the user asks for something recurring ("매일", "every day", "주기적으로"), create a skill with a schedule trigger.
For one-time requests, just execute the code directly without creating a skill.

Example — scheduled skill (MUST include schedule as 5th argument):
  await Skill.create("ai-news", "AI 뉴스 매시간 요약", `
    const r = await Web.search("AI news");
    const summary = r.results.map(x => x.title).join("\\n");
    await Telegram.sendMessage(summary);
    return summary;
  `, "schedule", "every 1h");

Schedule formats: "every 10m", "every 2h", "every 1d", or cron like "*/10 * * * *"

## Search language
When the user communicates in a specific language (e.g. Korean), generate Web.search queries in that SAME language to get locally relevant results.

## CRITICAL: Real data only — never fabricate
For ANY request involving external information (news, weather, prices, etc.):
1. ALWAYS call Web.search(query) or Http.get(url) FIRST to get real data
2. Use the ACTUAL search results in your response — do not summarize from memory
3. If search returns empty or fails, return "검색 결과를 가져오지 못했습니다" and STOP
4. Do NOT use Llm.generate() to create fake news/data — that is hallucination

ABSOLUTE PROHIBITIONS:
- Hardcoded news, weather, stock prices, or any factual content in your code
- Using Llm.generate() to write news articles (the LLM has no real-time knowledge)
- catch/fallback blocks containing fabricated content
- Returning "전송했습니다" without sending real fetched data

Example — CORRECT:
  const results = await Web.search("AI news today");
  const summary = results.results.map(r => r.title + ": " + r.snippet).join("\n");
  return summary;

Example — WRONG (hallucination):
  const news = await Llm.generate("write AI news");  // LLM invents fake news!
  return news;

## Voice output
When the user says "읽어줘", "읽어달라", "음성으로", or "read aloud":
1. Generate text content first
2. Call `const tts = await Tts.speak(text)` to create an audio file
3. Call `await Telegram.sendVoice(tts.path)` to send it as a voice message

## Clarification
When a request is ambiguous, ask a clarifying question in natural language BEFORE executing.
Example: User says "뉴스 보내줘" → return "어떤 분야의 뉴스를 원하시나요? (AI, 경제, 스타트업 등)"
The user's next message will contain the answer. Use it to proceed.

## Memory & Learning
When you learn something about the user (preferences, interests, corrections):
- Use Memory.user(key, value) to save it to their profile
- This reduces future clarification needs
- Most valuable memories: things that prevent the user from having to remind you again
"#;

const MAX_RETRIES: usize = 3;

/// Reusable session that holds provider/sandbox/store/config.
/// Create once, call `run()` for each event.
pub struct AgentSession<'a> {
    pub provider: &'a dyn LlmProvider,
    pub fallback_provider: Option<&'a dyn LlmProvider>,
    pub sandbox: &'a Sandbox,
    pub store: Arc<Mutex<Store>>,
    pub config: &'a kittypaw_core::config::Config,
    pub on_token: Option<Arc<dyn Fn(String) + Send + Sync>>,
    pub on_permission_request: Option<crate::skill_executor::PermissionCallback>,
}

impl<'a> AgentSession<'a> {
    pub async fn run(&self, event: Event) -> Result<String> {
        // Fast path: handle slash commands without LLM invocation
        if let Some(response) = commands::try_handle_command(
            &event,
            self.store.clone(),
            self.config,
            self.provider,
            &self.sandbox,
        )
        .await
        {
            return response;
        }

        run_agent_loop_inner(
            event,
            self.provider,
            self.fallback_provider,
            &self.sandbox,
            self.store.clone(),
            self.config,
            self.on_token.clone(),
            self.on_permission_request.clone(),
        )
        .await
    }
}

async fn run_agent_loop_inner(
    event: Event,
    provider: &dyn LlmProvider,
    fallback_provider: Option<&dyn LlmProvider>,
    sandbox: &Sandbox,
    store: Arc<Mutex<Store>>,
    config: &kittypaw_core::config::Config,
    on_token: Option<Arc<dyn Fn(String) + Send + Sync>>,
    on_permission_request: Option<crate::skill_executor::PermissionCallback>,
) -> Result<String> {
    let channel_name = event.event_type.channel_name();
    let channel_user_id = event.session_id();

    // Check if this channel user is linked to a global user identity.
    // If so, use a shared agent_id for cross-channel context.
    let agent_id = {
        let s = store.lock().await;
        match s.resolve_user(channel_name, &channel_user_id) {
            Ok(Some(global_id)) => {
                tracing::info!(
                    channel = channel_name,
                    channel_user_id = %channel_user_id,
                    global_user_id = %global_id,
                    "Resolved cross-channel identity"
                );
                format!("user-{global_id}")
            }
            _ => format!("{channel_name}-{channel_user_id}"),
        }
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

    let event_text = prompt::format_event(&event);

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

    // Check daily token budget before starting
    if config.features.daily_token_limit > 0 {
        let stats = store.lock().await.today_stats()?;
        if stats.total_tokens >= config.features.daily_token_limit {
            return Err(KittypawError::Llm {
                kind: kittypaw_core::error::LlmErrorKind::Other,
                message: format!(
                    "Daily token limit reached ({}/{})",
                    stats.total_tokens, config.features.daily_token_limit
                ),
            });
        }
    }

    let mut last_error: Option<String> = None;
    let mut active_provider: &dyn LlmProvider = provider;
    let mut fallback_used = false;
    let mut usage_ledger: Vec<TokenUsage> = Vec::new();

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
        // Resolve active profile for this agent
        let (active_profile_override, memory_context) = {
            let s = store.lock().await;
            let key = format!("active_profile:{}", agent_id);
            let profile = s.get_user_context(&key).ok().flatten();
            let mem_ctx = {
                use kittypaw_core::memory::MemoryProvider;
                s.memory_context_lines().unwrap_or_default()
            };
            (profile, mem_ctx)
        };
        let mut messages = prompt::build_prompt(
            &state,
            &event_text,
            &compaction,
            config,
            channel_name,
            active_profile_override.as_deref(),
            &memory_context,
        );
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
        let token_budget = active_provider
            .context_window()
            .saturating_sub(active_provider.max_tokens());
        if est_tokens > token_budget && attempt < MAX_RETRIES - 1 {
            tracing::warn!(
                est_tokens,
                budget = token_budget,
                context_window = active_provider.context_window(),
                attempt,
                "Prompt exceeds token budget, applying tighter compaction"
            );
            last_error = Some(format!(
                "Estimated {est_tokens} tokens exceeds budget {token_budget}"
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

        // Log full prompt chain at trace level for debugging
        // Usage: RUST_LOG=kittypaw_engine::agent_loop=trace kittypaw test-event "msg"
        if tracing::enabled!(tracing::Level::TRACE) {
            for (i, msg) in messages.iter().enumerate() {
                tracing::trace!(
                    "[prompt {i}] role={:?} len={}\n{}",
                    msg.role,
                    msg.content.len(),
                    msg.content
                );
            }
        }

        // Call LLM
        let llm_result = if let Some(ref cb) = on_token {
            active_provider
                .generate_stream(&messages, cb.clone())
                .instrument(info_span!("llm_generate"))
                .await
        } else {
            active_provider
                .generate(&messages)
                .instrument(info_span!("llm_generate"))
                .await
        };

        let code = match llm_result {
            Ok(resp) => {
                if let Some(u) = resp.usage {
                    usage_ledger.push(u);
                }
                resp.content
            }
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
                kind:
                    kittypaw_core::error::LlmErrorKind::RateLimit
                    | kittypaw_core::error::LlmErrorKind::Network,
                ref message,
            }) => {
                // On last attempt, try fallback before giving up
                if attempt >= MAX_RETRIES - 1 && !fallback_used {
                    if let Some(fb) = fallback_provider {
                        tracing::warn!(
                            attempt,
                            "Transient error exhausted retries, switching to fallback: {message}"
                        );
                        active_provider = fb;
                        fallback_used = true;
                        last_error = Some(message.clone());
                        continue;
                    }
                }
                tracing::warn!(
                    attempt,
                    "Transient error, sleeping 2s then retrying: {message}"
                );
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                last_error = Some(message.clone());
                continue;
            }
            Err(kittypaw_core::error::KittypawError::Llm { ref message, .. }) => {
                if !fallback_used {
                    if let Some(fb) = fallback_provider {
                        tracing::warn!(
                            attempt,
                            "LLM error, switching to fallback provider: {message}"
                        );
                        active_provider = fb;
                        fallback_used = true;
                        last_error = Some(message.clone());
                        continue;
                    }
                }
                tracing::error!(attempt, "LLM error (non-retryable, no fallback): {message}");
                return Err(kittypaw_core::error::KittypawError::Llm {
                    kind: kittypaw_core::error::LlmErrorKind::Other,
                    message: message.clone(),
                });
            }
            Err(e) => return Err(e),
        };
        tracing::debug!("Generated JS ({} chars)", code.len());

        // Security: scan for dangerous code patterns
        let warnings = crate::security::scan_code(&code);
        if !warnings.is_empty() {
            tracing::warn!("Dangerous code patterns detected: {:?}", warnings);
            crate::security::audit(crate::security::AuditEvent::warn(
                "dangerous_code",
                format!("agent={agent_id}: {}", warnings.join("; ")),
            ));
        }

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
        tracing::debug!(agent_id = %agent_id, "generated JS:\n{code}");

        let context = serde_json::json!({
            "event": event.payload,
            "event_type": format!("{:?}", event.event_type),
            "agent_id": agent_id,
        });

        // None = permissive mode (no agent config matched)
        // Match by agent_id, or by the originating channel name (works for both
        // channel-prefixed IDs like "telegram-123" and cross-channel "user-*" IDs).
        let checker: Option<Arc<std::sync::Mutex<CapabilityChecker>>> = {
            let agent_config = config
                .agents
                .iter()
                .find(|a| a.id == agent_id || a.channels.iter().any(|c| c == channel_name));
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
            if !exec_result.skill_calls.is_empty() {
                tracing::info!(
                    "{} skill calls resolved inline during execution",
                    exec_result.skill_calls.len()
                );
            }

            let raw_output = if exec_result.output.is_empty() {
                "(no output)".to_string()
            } else {
                exec_result.output.clone()
            };

            // Security: mask any leaked secrets in output
            let known_secrets = crate::security::load_known_secrets();
            let output = crate::security::mask_secrets(&raw_output, &known_secrets);

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
                result: Some(prompt::format_exec_result(&exec_result)),
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

        let err_msg = exec_result.error.unwrap_or("unknown error".into());
        tracing::warn!("Execution error (attempt {attempt}): {err_msg}\n--- failed code ---\n{code}\n--- end ---");
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{
        desktop_event, make_in_memory_store, telegram_event, test_config, MockJsProvider,
        PanicProvider, SequentialMockProvider,
    };
    use kittypaw_sandbox::Sandbox;

    #[test]
    fn system_prompt_contains_search_language_guide() {
        assert!(SYSTEM_PROMPT.contains("Search language"));
        assert!(SYSTEM_PROMPT.contains("SAME language"));
    }

    /// Agent loop processes a simple return and produces the expected output.
    #[tokio::test]
    async fn test_agent_loop_simple_return() {
        let provider = MockJsProvider::new(r#"return "hello from agent";"#);
        let config = test_config();
        let sandbox = Sandbox::new_threaded(config.sandbox.clone());
        let store = make_in_memory_store();

        let session = AgentSession {
            provider: &provider,
            fallback_provider: None,
            sandbox: &sandbox,
            store,
            config: &config,
            on_token: None,
            on_permission_request: None,
        };

        let result = session.run(desktop_event("hello")).await;
        assert!(result.is_ok(), "agent_loop should succeed: {:?}", result);
        assert_eq!(result.unwrap(), "hello from agent");
    }

    /// /help slash command is handled without LLM invocation.
    #[tokio::test]
    async fn test_slash_help_no_llm() {
        let config = test_config();
        let sandbox = Sandbox::new_threaded(config.sandbox.clone());
        let store = make_in_memory_store();
        let session = AgentSession {
            provider: &PanicProvider,
            fallback_provider: None,
            sandbox: &sandbox,
            store,
            config: &config,
            on_token: None,
            on_permission_request: None,
        };

        let result = session.run(desktop_event("/help")).await;
        assert!(result.is_ok(), "/help should succeed: {:?}", result);
        let text = result.unwrap();
        assert!(
            text.contains("/run"),
            "help text should mention /run: {text}"
        );
        assert!(
            text.contains("/teach"),
            "help text should mention /teach: {text}"
        );
    }

    /// /status returns execution stats from the in-memory store.
    #[tokio::test]
    async fn test_slash_status_returns_stats() {
        let config = test_config();
        let sandbox = Sandbox::new_threaded(config.sandbox.clone());
        let store = make_in_memory_store();
        let session = AgentSession {
            provider: &PanicProvider,
            fallback_provider: None,
            sandbox: &sandbox,
            store,
            config: &config,
            on_token: None,
            on_permission_request: None,
        };

        let result = session.run(desktop_event("/status")).await;
        assert!(result.is_ok(), "/status should succeed: {:?}", result);
        let text = result.unwrap();
        assert!(
            text.contains("오늘") || text.contains("실행") || text.contains("토큰"),
            "/status should contain run stats: {text}"
        );
    }

    /// Agent retries on sandbox error and succeeds on second attempt.
    #[tokio::test]
    async fn test_agent_loop_retries_on_sandbox_error() {
        // First attempt: invalid JS → sandbox error
        // Second attempt: valid JS → succeeds
        let provider = SequentialMockProvider::new([
            "this is not valid javascript !!!@@@###",
            r#"return "recovered";"#,
        ]);
        let config = test_config();
        let sandbox = Sandbox::new_threaded(config.sandbox.clone());
        let store = make_in_memory_store();

        let session = AgentSession {
            provider: &provider,
            fallback_provider: None,
            sandbox: &sandbox,
            store,
            config: &config,
            on_token: None,
            on_permission_request: None,
        };

        let result = session.run(desktop_event("try something")).await;
        assert!(result.is_ok(), "should succeed after retry: {:?}", result);
        assert_eq!(result.unwrap(), "recovered");
    }

    /// Telegram events are processed correctly (separate agent_id from desktop).
    #[tokio::test]
    async fn test_agent_loop_telegram_event() {
        let provider = MockJsProvider::new(r#"return "tg:ok";"#);
        let config = test_config();
        let sandbox = Sandbox::new_threaded(config.sandbox.clone());
        let store = make_in_memory_store();

        let session = AgentSession {
            provider: &provider,
            fallback_provider: None,
            sandbox: &sandbox,
            store,
            config: &config,
            on_token: None,
            on_permission_request: None,
        };

        let result = session.run(telegram_event("hello", "99999")).await;
        assert!(
            result.is_ok(),
            "telegram event should succeed: {:?}",
            result
        );
        assert_eq!(result.unwrap(), "tg:ok");
    }
}
