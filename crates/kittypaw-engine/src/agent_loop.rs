use std::sync::Arc;
use tokio::sync::Mutex;

use kittypaw_core::capability::CapabilityChecker;
use kittypaw_core::error::{KittypawError, Result};
use kittypaw_core::types::{
    now_timestamp, AgentState, ConversationTurn, Event, EventType, ExecutionResult, LlmMessage,
    LoopPhase, Role, TransitionReason,
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
- Web.search(query) — Web search, returns results as JSON
- Web.fetch(url) — Fetch a web page, returns text content
- Llm.generate(prompt) — Generate text using LLM
- Tts.speak(text, options?) — Text-to-speech, returns { path, size }. Options: { voice, rate, pitch }
- Telegram.sendVoice(chatId, filePath, caption?) — Send audio file as voice message
- Telegram.sendDocument(chatId, fileUrl, caption?) — Send a file
- File.read(path) — Read a file
- File.write(path, content) — Write a file
- Env.get(key) — Get environment variable
- Shell.exec(command) — Execute a shell command
- Skill.create(name, description, code, triggerType, triggerValue) — Create a reusable skill
  triggerType: "schedule" (with cron like "0 0 7 * * *") or "message" (with keyword)
  Example: Skill.create("daily-news", "뉴스 요약", "const n = await Web.search('news'); return n;", "schedule", "0 0 7 * * *")
- Skill.list() — List all saved skills
- Skill.delete(name) — Delete a skill
- console.log(...args) — Log output (for debugging)

## When to create a skill
If the user asks for something recurring ("매일", "every day", "주기적으로"), create a skill with a schedule trigger.
For one-time requests, just execute the code directly without creating a skill.

## Important: Execute the actual task
When a user asks you to do something (e.g., "뉴스 브리핑 보내줘"), actually DO it:
1. Fetch real data using Http.get or Web.search
2. Process/summarize using Llm.generate if needed
3. Send the actual content via Telegram.sendMessage
Do NOT just return "전송했습니다" without actually fetching and sending real data.
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
        if let Some(response) = try_handle_command(
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

/// Legacy params struct — kept for backward compatibility.
pub struct AgentLoopParams<'a> {
    pub event: Event,
    pub provider: &'a dyn LlmProvider,
    pub fallback_provider: Option<&'a dyn LlmProvider>,
    pub sandbox: &'a Sandbox,
    pub store: Arc<Mutex<Store>>,
    pub config: &'a kittypaw_core::config::Config,
    pub on_token: Option<Arc<dyn Fn(String) + Send + Sync>>,
    pub on_permission_request: Option<crate::skill_executor::PermissionCallback>,
}

pub async fn run_agent_loop(params: AgentLoopParams<'_>) -> Result<String> {
    let AgentLoopParams {
        event,
        provider,
        fallback_provider,
        sandbox,
        store,
        config,
        on_token,
        on_permission_request,
    } = params;
    run_agent_loop_inner(
        event,
        provider,
        fallback_provider,
        sandbox,
        store,
        config,
        on_token,
        on_permission_request,
    )
    .await
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
    let channel_name = match event.event_type {
        EventType::Telegram => "telegram",
        EventType::WebChat => "web",
        EventType::Desktop => "desktop",
    };
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

    let event_text = format_event(&event);

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

    // Inject connected channel info so LLM can use Telegram/Slack/Discord directly
    {
        let mut channel_info = Vec::new();
        if let Ok(Some(tg_id)) = kittypaw_core::secrets::get_secret("telegram", "chat_id") {
            if !tg_id.is_empty() {
                channel_info.push(
                    "Telegram is connected. Send messages with: await Telegram.sendMessage(\"message text\")".to_string()
                );
            }
        }
        if !channel_info.is_empty() {
            messages.push(LlmMessage {
                role: Role::System,
                content: channel_info.join("\n"),
            });
        }
    }

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

// ── Slash command pre-processing ────────────────────────────────────────
//
// These handlers intercept slash commands before LLM invocation for
// fast, deterministic responses. Returns None to fall through to agent_loop.

async fn try_handle_command(
    event: &Event,
    store: Arc<Mutex<Store>>,
    config: &kittypaw_core::config::Config,
    provider: &dyn LlmProvider,
    sandbox: &Sandbox,
) -> Option<Result<String>> {
    let text = event
        .payload
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let trimmed = text.trim();

    // Slash commands: fast-path without LLM
    if trimmed.starts_with('/') {
        return match trimmed {
            "/help" | "/start" => Some(Ok("KittyPaw 명령어:\n\n\
             /run <스킬이름> — 스킬 즉시 실행\n\
             /status — 오늘 실행 통계\n\
             /teach <설명> — 새 스킬 가르치기\n\
             /link <유저ID> — 이 채널을 유저 ID에 연결 (크로스채널 대화 공유)\n\
             /help — 도움말\n\n\
             자연어 메시지를 보내면 AI가 직접 처리합니다."
                .to_string())),

            "/status" => {
                let s = store.lock().await;
                match s.today_stats() {
                    Ok(stats) => Some(Ok(format!(
                        "📊 오늘 실행: {} (성공 {}, 실패 {})\n토큰: {}",
                        stats.total_runs, stats.successful, stats.failed, stats.total_tokens
                    ))),
                    Err(e) => Some(Err(e)),
                }
            }

            _ if trimmed.starts_with("/run ") => {
                let skill_name = trimmed.strip_prefix("/run ").unwrap().trim();
                if skill_name.is_empty() {
                    return Some(Ok("Usage: /run <스킬이름>".to_string()));
                }
                let session_id = event.session_id();
                Some(
                    run_skill_by_name(
                        skill_name,
                        &session_id,
                        event,
                        config,
                        provider,
                        sandbox,
                        &store,
                    )
                    .await,
                )
            }

            _ if trimmed.starts_with("/link ") => {
                let global_user_id = trimmed.strip_prefix("/link ").unwrap().trim();
                if global_user_id.is_empty() {
                    return Some(Ok("Usage: /link <유저ID>".to_string()));
                }
                let channel = match event.event_type {
                    kittypaw_core::types::EventType::Telegram => "telegram",
                    kittypaw_core::types::EventType::WebChat => "web",
                    kittypaw_core::types::EventType::Desktop => "desktop",
                };
                let channel_user_id = event.session_id();

                // Only admin users (or anyone if admin list is empty) can link identities.
                if !config.admin_chat_ids.is_empty()
                    && !config
                        .admin_chat_ids
                        .iter()
                        .any(|id| id == &channel_user_id)
                {
                    return Some(Ok("❌ identity 연결은 관리자만 가능합니다.".to_string()));
                }

                let s = store.lock().await;
                match s.link_identity(global_user_id, channel, &channel_user_id) {
                    Ok(()) => Some(Ok(format!(
                        "✅ {channel}:{channel_user_id} → {global_user_id} 연결 완료.\n\
                         이제 연결된 채널에서 동일한 대화 기록을 공유합니다."
                    ))),
                    Err(e) => Some(Err(e)),
                }
            }

            _ if trimmed.starts_with("/teach") => {
                let teach_text = trimmed.strip_prefix("/teach").unwrap().trim();
                let session_id = event.session_id();
                if teach_text.is_empty() {
                    return Some(Ok(
                        "Usage: /teach <설명>\n\nExample: /teach 매일 아침 날씨 알려줘".to_string(),
                    ));
                }
                Some(handle_teach_command(teach_text, &session_id, provider, sandbox, config).await)
            }

            // Unknown slash command — fall through to agent_loop
            _ => None,
        };
    }

    // Non-slash messages: check taught skill triggers before agent_loop
    if let Ok(skill_list) = kittypaw_core::skill::load_all_skills() {
        if let Some((skill, js_code)) = skill_list
            .into_iter()
            .find(|(skill, _)| skill.enabled && kittypaw_core::skill::match_trigger(skill, trimmed))
        {
            let session_id = event.session_id();
            return Some(
                execute_skill_code(
                    &js_code,
                    &skill.name,
                    &session_id,
                    event,
                    config,
                    sandbox,
                    &store,
                    Some(&skill.permissions),
                )
                .await,
            );
        }
    }

    // No skill matched — respect freeform_fallback setting
    if !config.freeform_fallback {
        return Some(Ok(
            "매칭되는 스킬이 없습니다. /teach로 새 스킬을 만들어보세요.".to_string(),
        ));
    }

    // Fall through to agent_loop (LLM-powered response)
    None
}

/// Execute a saved skill or installed package by name.
async fn run_skill_by_name(
    skill_name: &str,
    session_id: &str,
    event: &Event,
    config: &kittypaw_core::config::Config,
    provider: &dyn LlmProvider,
    sandbox: &Sandbox,
    store: &Arc<Mutex<Store>>,
) -> Result<String> {
    // Try user-taught skill first
    if let Ok(Some((skill, code_or_prompt))) = kittypaw_core::skill::load_skill(skill_name) {
        let js_code = if skill.format == kittypaw_core::skill::SkillFormat::SkillMd {
            // SKILL.md: use LLM to generate JS from the prompt
            let messages = vec![
                LlmMessage {
                    role: Role::System,
                    content: format!("{}\n\n{}", SYSTEM_PROMPT, code_or_prompt),
                },
                LlmMessage {
                    role: Role::User,
                    content: format!("Execute this skill for chat_id={}", session_id),
                },
            ];
            provider.generate(&messages).await?.content
        } else {
            code_or_prompt
        };

        return execute_skill_code(
            &js_code,
            skill_name,
            session_id,
            event,
            config,
            sandbox,
            store,
            Some(&skill.permissions),
        )
        .await;
    }

    // Try installed package
    let data_dir = kittypaw_core::secrets::data_dir()
        .unwrap_or_else(|_| std::path::PathBuf::from(".kittypaw"));
    let packages_dir = data_dir.join("packages");
    let pkg_mgr = kittypaw_core::package_manager::PackageManager::new(packages_dir.clone());

    if let Ok(pkg) = pkg_mgr.load_package(skill_name) {
        let js_path = packages_dir.join(skill_name).join("main.js");
        let js_code = std::fs::read_to_string(&js_path).map_err(|_| {
            KittypawError::Sandbox(format!(
                "패키지 '{skill_name}'의 main.js를 찾을 수 없습니다."
            ))
        })?;

        let config_values = pkg_mgr
            .get_config_with_defaults(skill_name)
            .unwrap_or_default();
        let shared_ctx = {
            let s = store.lock().await;
            s.list_shared_context().unwrap_or_default()
        };
        let event_payload = serde_json::json!({
            "event_type": format!("{:?}", event.event_type).to_lowercase(),
            "chat_id": session_id,
        });
        let context = pkg.build_context(&config_values, event_payload, None, &shared_ctx);
        let wrapped_code = format!("const ctx = JSON.parse(__context__);\n{js_code}");

        let exec_result = sandbox.execute(&wrapped_code, context).await?;
        if !exec_result.skill_calls.is_empty() {
            let s = store.lock().await;
            let preresolved = crate::skill_executor::resolve_storage_calls(
                &exec_result.skill_calls,
                &*s,
                Some(&pkg.meta.id),
            );
            drop(s);
            let mut checker =
                kittypaw_core::capability::CapabilityChecker::from_package_permissions(
                    &pkg.permissions,
                );
            let _ = crate::skill_executor::execute_skill_calls(
                &exec_result.skill_calls,
                config,
                preresolved,
                Some(&pkg.meta.id),
                Some(&mut checker),
                None,
            )
            .await;
        }

        return Ok(if exec_result.output.is_empty() {
            "(no output)".to_string()
        } else {
            exec_result.output
        });
    }

    Err(KittypawError::Sandbox(format!(
        "스킬 또는 패키지 '{skill_name}'을 찾을 수 없습니다."
    )))
}

/// Execute JS code in sandbox and handle resulting skill calls.
async fn execute_skill_code(
    js_code: &str,
    skill_name: &str,
    session_id: &str,
    event: &Event,
    config: &kittypaw_core::config::Config,
    sandbox: &Sandbox,
    store: &Arc<Mutex<Store>>,
    permissions: Option<&kittypaw_core::skill::SkillPermissions>,
) -> Result<String> {
    let wrapped_code = format!("const ctx = JSON.parse(__context__);\n{js_code}");
    let context = serde_json::json!({
        "event_type": format!("{:?}", event.event_type).to_lowercase(),
        "event_text": event.payload.get("text").and_then(|v| v.as_str()).unwrap_or(""),
        "chat_id": session_id,
        "skill_name": skill_name,
    });

    let exec_result = sandbox.execute(&wrapped_code, context).await?;
    if !exec_result.skill_calls.is_empty() {
        let s = store.lock().await;
        let preresolved = crate::skill_executor::resolve_storage_calls(
            &exec_result.skill_calls,
            &*s,
            Some(skill_name),
        );
        drop(s);
        let mut checker = permissions.map(|perms| {
            kittypaw_core::capability::CapabilityChecker::from_skill_permissions(perms)
        });
        let _ = crate::skill_executor::execute_skill_calls(
            &exec_result.skill_calls,
            config,
            preresolved,
            Some(skill_name),
            checker.as_mut(),
            None,
        )
        .await;
    }

    Ok(if exec_result.output.is_empty() {
        "(no output)".to_string()
    } else {
        exec_result.output
    })
}

/// Handle /teach command: generate a skill via LLM, save it.
async fn handle_teach_command(
    teach_text: &str,
    session_id: &str,
    provider: &dyn LlmProvider,
    sandbox: &Sandbox,
    config: &kittypaw_core::config::Config,
) -> Result<String> {
    match crate::teach_loop::handle_teach(teach_text, session_id, provider, sandbox, config).await {
        Ok(
            ref result @ crate::teach_loop::TeachResult::Generated {
                ref code,
                ref dry_run_output,
                ref skill_name,
                ..
            },
        ) => match crate::teach_loop::approve_skill(result) {
            Ok(()) => Ok(format!(
                "✅ 스킬 '{skill_name}' 생성 완료!\n\nCode:\n{code}\n\nDry-run: {dry_run_output}"
            )),
            Err(e) => Err(e),
        },
        Ok(crate::teach_loop::TeachResult::Error(e)) => {
            Err(KittypawError::Sandbox(format!("Teach failed: {e}")))
        }
        Err(e) => Err(e),
    }
}
