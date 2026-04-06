use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

use kittypaw_core::config::Config;
use kittypaw_core::error::Result;
use kittypaw_core::registry::RegistryEntry;
use kittypaw_core::types::{
    now_timestamp, AgentState, ConversationTurn, Event, EventType, LlmMessage, LoopPhase, Role,
};
use kittypaw_llm::provider::LlmProvider;
use kittypaw_sandbox::sandbox::Sandbox;
use kittypaw_store::Store;
use serde::{Deserialize, Serialize};

use crate::teach_loop;

/// Actions the assistant can take in response to user input.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum AssistantAction {
    /// Reply with natural language text
    Reply { text: String },
    /// Search the skill registry for matching skills
    SearchRegistry { query: String },
    /// Recommend a specific registry skill to the user
    RecommendSkill { skill_id: String, reason: String },
    /// Create a new skill via the teach loop
    CreateSkill {
        description: String,
        schedule: Option<String>,
    },
    /// Save a user preference to user_context
    SavePreference { key: String, value: String },
    /// Ask the user a clarifying question
    AskQuestion {
        question: String,
        options: Vec<String>,
    },
}

/// Result of running one turn of the assistant loop.
pub struct AssistantTurn {
    pub response_text: String,
    pub actions_taken: Vec<AssistantAction>,
}

const SYSTEM_PROMPT: &str = r#"You are KittyPaw, a friendly personal AI assistant that helps users automate their daily tasks.

## Your Personality
- Warm, helpful, and concise
- You speak the user's language (Korean if they speak Korean, English if English, etc.)
- You proactively suggest automations based on what you learn about the user

## What You Can Do
You help users by understanding their needs and either:
1. **Recommending existing skills** from the registry if one matches
2. **Creating new skills** — immediately when the request is specific enough
3. **Remembering preferences** so future interactions are personalized

## How to Respond
Reply with a JSON array of actions. Each action is an object with an "action" field.

Available actions:
- `{"action": "reply", "text": "..."}` — Say something to the user
- `{"action": "search_registry", "query": "..."}` — Search for existing skills (use keywords)
- `{"action": "recommend_skill", "skill_id": "...", "reason": "..."}` — Recommend a found skill
- `{"action": "create_skill", "description": "...", "schedule": "cron expression or null"}` — Create a new automation
- `{"action": "save_preference", "key": "...", "value": "..."}` — Remember something about the user
- `{"action": "ask_question", "question": "...", "options": ["A", "B", ...]}` — Ask for clarification

## Rules
- ALWAYS respond with a valid JSON array, even for simple replies: `[{"action": "reply", "text": "안녕하세요!"}]`
- ALWAYS include at least one "reply" action with a natural response to the user. NEVER return only search_registry or other non-reply actions.
- When a user names specific skills or says "만들어줘" / "build" / "create", emit create_skill actions IMMEDIATELY for each one. Do NOT ask questions first. Act now.
- If a user asks for multiple skills at once, include ALL of them as separate create_skill actions in your JSON response array.
- Only ask clarifying questions if the request is truly vague (no task names, no clear purpose).
- You MUST use actions to take action — do not just describe or list what you would do. Execute it now.
- Do NOT use search_registry — the skill registry is not yet populated. Instead, create new skills directly.
- Save preferences when you learn something reusable (location, preferred channels, schedule patterns)
- Preference keys should be descriptive: "preferred_channel", "location", "wake_up_time", etc.
- For simple questions or greetings, just reply naturally without trying to create skills.
"#;

/// Context for running an assistant turn. Bundles all dependencies.
pub struct AssistantContext<'a> {
    pub event: &'a Event,
    pub provider: &'a dyn LlmProvider,
    pub store: Arc<Mutex<Store>>,
    pub registry_entries: &'a [RegistryEntry],
    pub sandbox: &'a Sandbox,
    pub config: &'a Config,
    pub on_token: Option<Arc<dyn Fn(String) + Send + Sync>>,
}

fn extract_chat_id(event: &Event) -> String {
    match event.event_type {
        EventType::Telegram => event
            .payload
            .get("chat_id")
            .map(|v| {
                v.as_str()
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| v.to_string())
            })
            .unwrap_or_else(|| "default".to_string()),
        _ => "local".to_string(),
    }
}

fn is_admin_event(event: &Event, config: &Config) -> bool {
    // Desktop/CLI are always admin — it's the local user
    if matches!(event.event_type, EventType::Desktop) {
        return true;
    }
    if config.admin_chat_ids.is_empty() {
        return false;
    }
    let chat_id = extract_chat_id(event);
    config.admin_chat_ids.iter().any(|id| id == &chat_id)
}

/// Run one turn of the assistant conversation.
async fn execute_actions(
    actions: &[AssistantAction],
    ctx: &AssistantContext<'_>,
) -> (Vec<String>, Vec<AssistantAction>) {
    let mut response_parts: Vec<String> = Vec::new();
    let mut actions_taken: Vec<AssistantAction> = Vec::new();

    for action in actions {
        match action {
            AssistantAction::Reply { text } => {
                response_parts.push(text.clone());
            }
            AssistantAction::SearchRegistry { query } => {
                let results = search_entries(ctx.registry_entries, query);
                if results.is_empty() {
                    response_parts.push(
                        "아직 스킬 레지스트리에 맞는 스킬이 없어요. 새로 만들어드릴까요? 필요한 정보를 알려주세요!".to_string()
                    );
                } else {
                    let listing: Vec<String> = results
                        .iter()
                        .map(|e| format!("- **{}** ({}): {}", e.name, e.id, e.description))
                        .collect();
                    response_parts.push(format!("관련 스킬을 찾았어요:\n{}", listing.join("\n")));
                }
            }
            AssistantAction::RecommendSkill { skill_id, reason } => {
                if let Some(entry) = ctx.registry_entries.iter().find(|e| e.id == *skill_id) {
                    response_parts.push(format!(
                        "이 스킬을 추천해요: **{}** ({})\n{}\n\n설치할까요?",
                        entry.name, entry.id, reason
                    ));
                } else {
                    response_parts.push(format!(
                        "스킬 '{skill_id}'을 추천하고 싶었는데, 레지스트리에서 찾을 수 없어요."
                    ));
                }
            }
            AssistantAction::CreateSkill {
                description,
                schedule,
            } => {
                if !is_admin_event(ctx.event, ctx.config) {
                    response_parts.push(
                        "스킬 생성 권한이 없습니다. admin_chat_ids 설정을 확인해주세요.".into(),
                    );
                    actions_taken.push(action.clone());
                    continue;
                }

                let chat_id = extract_chat_id(ctx.event);
                let full_description = if let Some(ref sched) = schedule {
                    format!("{description} (schedule: {sched})")
                } else {
                    description.clone()
                };

                match teach_loop::handle_teach(
                    &full_description,
                    &chat_id,
                    ctx.provider,
                    ctx.sandbox,
                    ctx.config,
                )
                .await
                {
                    Ok(
                        ref result @ teach_loop::TeachResult::Generated {
                            ref skill_name,
                            ref dry_run_output,
                            ..
                        },
                    ) => match teach_loop::approve_skill(result) {
                        Ok(()) => {
                            response_parts.push(format!(
                                "스킬 **{skill_name}** 생성 완료!\n드라이런 결과: {dry_run_output}"
                            ));
                        }
                        Err(e) => {
                            response_parts.push(format!("스킬 저장 실패: {e}"));
                        }
                    },
                    Ok(teach_loop::TeachResult::Error(e)) => {
                        response_parts.push(format!("스킬 생성 실패: {e}"));
                    }
                    Err(e) => {
                        response_parts.push(format!("오류 발생: {e}"));
                    }
                }
            }
            AssistantAction::SavePreference { key, value } => {
                let s = ctx.store.lock().await;
                let _ = s.set_user_context(key, value, "assistant");
            }
            AssistantAction::AskQuestion { question, options } => {
                if options.is_empty() {
                    response_parts.push(question.clone());
                } else {
                    let opts: Vec<String> = options
                        .iter()
                        .enumerate()
                        .map(|(i, o)| format!("{}. {o}", i + 1))
                        .collect();
                    response_parts.push(format!("{question}\n{}", opts.join("\n")));
                }
            }
        }
        actions_taken.push(action.clone());
    }

    (response_parts, actions_taken)
}

pub async fn run_assistant_turn(ctx: &AssistantContext<'_>) -> Result<AssistantTurn> {
    let agent_id = format!("assistant-{}", ctx.event.session_id());

    // Load or create agent state — ensure agent exists in DB before adding turns
    let mut state = {
        let s = ctx.store.lock().await;
        match s.load_state(&agent_id)? {
            Some(existing) => existing,
            None => {
                let new_state = AgentState::new(&agent_id, SYSTEM_PROMPT);
                s.save_state(&new_state)?;
                new_state
            }
        }
    };
    tracing::info!(phase = ?LoopPhase::Init, agent_id = %agent_id, "assistant state ready");

    // Load user context for personalization
    let user_context = {
        let s = ctx.store.lock().await;
        s.list_shared_context().unwrap_or_default()
    };

    // Build messages
    let event_text = extract_text(ctx.event);
    let messages = build_messages(
        &state,
        &event_text,
        &user_context,
        ctx.registry_entries,
        ctx.config,
    );

    // Save user turn
    let user_turn = ConversationTurn {
        role: Role::User,
        content: event_text.clone(),
        code: None,
        result: None,
        timestamp: now_timestamp(),
    };
    state.add_turn(user_turn.clone());
    {
        let s = ctx.store.lock().await;
        s.add_turn(&agent_id, &user_turn)?;
    }

    // Call LLM
    let llm_resp = if let Some(ref cb) = ctx.on_token {
        ctx.provider.generate_stream(&messages, cb.clone()).await?
    } else {
        ctx.provider.generate(&messages).await?
    };
    let raw_response = llm_resp.content;

    tracing::info!(phase = ?LoopPhase::Generate, agent_id = %agent_id, response_len = raw_response.len(), "llm response received");

    let actions = parse_actions(&raw_response);
    tracing::info!(phase = ?LoopPhase::Execute, agent_id = %agent_id, action_count = actions.len(), "actions parsed");

    let (response_parts, actions_taken) = execute_actions(&actions, ctx).await;

    let response_text = if response_parts.is_empty() {
        raw_response.clone()
    } else {
        response_parts.join("\n\n")
    };

    // Save assistant turn
    let assistant_turn = ConversationTurn {
        role: Role::Assistant,
        content: response_text.clone(),
        code: None,
        result: Some(serde_json::to_string(&actions_taken).unwrap_or_default()),
        timestamp: now_timestamp(),
    };
    state.add_turn(assistant_turn.clone());
    {
        let s = ctx.store.lock().await;
        s.add_turn(&agent_id, &assistant_turn)?;
        s.save_state(&state)?;
    }
    tracing::info!(phase = ?LoopPhase::Finish, agent_id = %agent_id, response_len = response_text.len(), actions = actions_taken.len(), "assistant turn complete");

    Ok(AssistantTurn {
        response_text,
        actions_taken,
    })
}

fn build_messages(
    state: &AgentState,
    event_text: &str,
    user_context: &HashMap<String, String>,
    registry_entries: &[RegistryEntry],
    config: &Config,
) -> Vec<LlmMessage> {
    let mut messages = vec![LlmMessage {
        role: Role::System,
        content: SYSTEM_PROMPT.to_string(),
    }];

    // Inject user context
    if !user_context.is_empty() {
        let context_lines: Vec<String> = user_context
            .iter()
            .map(|(k, v)| format!("- {k}: {v}"))
            .collect();
        messages.push(LlmMessage {
            role: Role::System,
            content: format!(
                "## What you know about this user\n{}",
                context_lines.join("\n")
            ),
        });
    }

    // Inject configured channel info so LLM knows it can send messages
    {
        let mut channels = Vec::new();
        if let Ok(Some(tg_id)) = kittypaw_core::secrets::get_secret("telegram", "chat_id") {
            if !tg_id.is_empty() {
                channels.push(format!(
                    "- Telegram: 연결됨 (chat_id: \"{tg_id}\"). 사용자가 텔레그램 관련 요청을 하면 새 스킬을 만들지 말고, 코드에서 바로 await Telegram.sendMessage(\"{tg_id}\", \"메시지 내용\") 을 사용하세요."
                ));
            }
        }
        for ch in &config.channels {
            match ch.channel_type {
                kittypaw_core::config::ChannelType::Slack => {
                    channels.push(
                        "- Slack: 설정됨. Slack.sendMessage(channel, text) 사용 가능.".into(),
                    );
                }
                kittypaw_core::config::ChannelType::Discord => {
                    channels.push(
                        "- Discord: 설정됨. Discord.sendMessage(channel, text) 사용 가능.".into(),
                    );
                }
                _ => {}
            }
        }
        if !channels.is_empty() {
            messages.push(LlmMessage {
                role: Role::System,
                content: format!("## Connected channels\n{}", channels.join("\n")),
            });
        }
    }

    // Inject available skills summary
    if !registry_entries.is_empty() {
        let skill_list: Vec<String> = registry_entries
            .iter()
            .take(50)
            .map(|e| format!("- {} ({}): {}", e.name, e.id, e.description))
            .collect();
        messages.push(LlmMessage {
            role: Role::System,
            content: format!("## Available skills in registry\n{}", skill_list.join("\n")),
        });
    }

    // Add compacted conversation history (3-stage: summary / truncated / full).
    // When context_compaction is disabled, use a simple recent-only window.
    {
        use crate::compaction::{compact_turns, CompactionConfig, CompactionMode};
        let compaction_cfg = if config.features.context_compaction {
            CompactionConfig::default()
        } else {
            CompactionConfig {
                recent_window: 20,
                middle_window: 0,
                truncate_len: 100,
            }
        };
        let compacted = compact_turns(&state.turns, &compaction_cfg, &CompactionMode::Assistant);
        messages.extend(compacted);
    }

    // Current user message
    messages.push(LlmMessage {
        role: Role::User,
        content: event_text.to_string(),
    });

    messages
}

/// Parse the LLM response into a list of actions.
/// Handles both valid JSON arrays and plain text fallback.
fn parse_actions(response: &str) -> Vec<AssistantAction> {
    let trimmed = response.trim();

    // Try to extract JSON from markdown code fences
    let json_str = if let Some(start) = trimmed.find("```json") {
        let after_fence = &trimmed[start + 7..];
        if let Some(end) = after_fence.find("```") {
            after_fence[..end].trim()
        } else {
            trimmed
        }
    } else if let Some(start) = trimmed.find("```") {
        let after_fence = &trimmed[start + 3..];
        if let Some(end) = after_fence.find("```") {
            after_fence[..end].trim()
        } else {
            trimmed
        }
    } else {
        trimmed
    };

    // Try parsing as JSON array
    if let Ok(actions) = serde_json::from_str::<Vec<AssistantAction>>(json_str) {
        return actions;
    }

    // Try parsing as single JSON object
    if let Ok(action) = serde_json::from_str::<AssistantAction>(json_str) {
        return vec![action];
    }

    // Fallback: treat entire response as a plain text reply
    vec![AssistantAction::Reply {
        text: response.to_string(),
    }]
}

/// Simple keyword search over registry entries.
fn search_entries<'a>(entries: &'a [RegistryEntry], query: &str) -> Vec<&'a RegistryEntry> {
    let keywords: Vec<String> = query
        .to_lowercase()
        .split_whitespace()
        .map(String::from)
        .collect();
    if keywords.is_empty() {
        return vec![];
    }

    let mut scored: Vec<(&RegistryEntry, usize)> = entries
        .iter()
        .filter_map(|entry| {
            let haystack = format!(
                "{} {} {} {}",
                entry.name,
                entry.description,
                entry.category,
                entry.tags.join(" ")
            )
            .to_lowercase();

            let score = keywords
                .iter()
                .filter(|kw| haystack.contains(kw.as_str()))
                .count();
            if score > 0 {
                Some((entry, score))
            } else {
                None
            }
        })
        .collect();

    scored.sort_by(|a, b| b.1.cmp(&a.1));
    scored.into_iter().take(5).map(|(e, _)| e).collect()
}

fn extract_text(event: &Event) -> String {
    event
        .payload
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use kittypaw_llm::openai::OpenAiProvider;

    #[test]
    fn test_parse_actions_json_array() {
        let input = r#"[{"action": "reply", "text": "안녕하세요!"}]"#;
        let actions = parse_actions(input);
        assert_eq!(actions.len(), 1);
        assert!(matches!(&actions[0], AssistantAction::Reply { text } if text == "안녕하세요!"));
    }

    #[test]
    fn test_parse_actions_single_object() {
        let input = r#"{"action": "reply", "text": "hello"}"#;
        let actions = parse_actions(input);
        assert_eq!(actions.len(), 1);
    }

    #[test]
    fn test_parse_actions_code_fence() {
        let input =
            "Here's my response:\n```json\n[{\"action\": \"reply\", \"text\": \"hi\"}]\n```";
        let actions = parse_actions(input);
        assert_eq!(actions.len(), 1);
        assert!(matches!(&actions[0], AssistantAction::Reply { text } if text == "hi"));
    }

    #[test]
    fn test_parse_actions_plain_text_fallback() {
        let input = "I don't understand structured output yet";
        let actions = parse_actions(input);
        assert_eq!(actions.len(), 1);
        assert!(matches!(&actions[0], AssistantAction::Reply { .. }));
    }

    #[test]
    fn test_parse_actions_multi_action() {
        let input = r#"[
            {"action": "save_preference", "key": "location", "value": "Seoul"},
            {"action": "reply", "text": "서울로 기억할게요!"}
        ]"#;
        let actions = parse_actions(input);
        assert_eq!(actions.len(), 2);
        assert!(
            matches!(&actions[0], AssistantAction::SavePreference { key, .. } if key == "location")
        );
        assert!(matches!(&actions[1], AssistantAction::Reply { .. }));
    }

    #[test]
    fn test_parse_actions_search_registry() {
        let input = r#"[{"action": "search_registry", "query": "weather"}]"#;
        let actions = parse_actions(input);
        assert_eq!(actions.len(), 1);
        assert!(
            matches!(&actions[0], AssistantAction::SearchRegistry { query } if query == "weather")
        );
    }

    #[test]
    fn test_search_entries() {
        let entries = vec![
            RegistryEntry {
                id: "weather-briefing".into(),
                name: "날씨 브리핑".into(),
                version: "1.0.0".into(),
                description: "매일 아침 날씨를 알려줍니다".into(),
                author: "kittypaw".into(),
                category: "weather".into(),
                tags: vec!["weather".into(), "daily".into()],
                download_url: "https://example.com".into(),
            },
            RegistryEntry {
                id: "news-summary".into(),
                name: "뉴스 요약".into(),
                version: "1.0.0".into(),
                description: "주요 뉴스를 요약합니다".into(),
                author: "kittypaw".into(),
                category: "news".into(),
                tags: vec!["news".into()],
                download_url: "https://example.com".into(),
            },
        ];

        let results = search_entries(&entries, "날씨");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "weather-briefing");

        let results = search_entries(&entries, "weather daily");
        assert_eq!(results.len(), 1);

        let results = search_entries(&entries, "gaming");
        assert!(results.is_empty());
    }

    #[test]
    fn test_assistant_id_for_event() {
        let event = Event {
            event_type: EventType::Telegram,
            payload: serde_json::json!({"chat_id": "12345", "text": "hello"}),
        };
        assert_eq!(
            format!("assistant-{}", event.session_id()),
            "assistant-12345"
        );

        let event = Event {
            event_type: EventType::Desktop,
            payload: serde_json::json!({"text": "hello"}),
        };
        assert_eq!(
            format!("assistant-{}", event.session_id()),
            "assistant-default"
        );
    }

    /// Integration test: verify the LLM returns multiple create_skill actions
    /// when asked to build a batch of skills.
    /// Run with: OPENROUTER_API_KEY=sk-or-... cargo test -p kittypaw-cli test_batch_skill_creation -- --ignored --nocapture
    #[tokio::test]
    #[ignore]
    async fn test_batch_skill_creation() {
        let api_key = match std::env::var("OPENROUTER_API_KEY") {
            Ok(k) if !k.is_empty() => k,
            _ => {
                eprintln!("OPENROUTER_API_KEY not set, skipping integration test");
                return;
            }
        };
        let provider = OpenAiProvider::with_base_url(
            "https://openrouter.ai/api/v1".into(),
            api_key,
            "nvidia/nemotron-3-super-120b-a12b:free".into(),
            4096,
        );
        let messages = vec![
            LlmMessage {
                role: Role::System,
                content: SYSTEM_PROMPT.to_string(),
            },
            LlmMessage {
                role: Role::User,
                content: "콘텐츠 자동화 시스템을 만들어줘: 브랜드 보이스, 인용 트윗 생성기, \
                    X 아티클 작성기, YouTube→트윗 변환, CTA 생성기, 트렌드 리서치. \
                    총 6개 스킬을 지금 바로 만들어줘."
                    .to_string(),
            },
        ];
        let raw = provider
            .generate(&messages)
            .await
            .expect("LLM call failed")
            .content;
        let actions = parse_actions(&raw);
        let create_count = actions
            .iter()
            .filter(|a| matches!(a, AssistantAction::CreateSkill { .. }))
            .count();
        eprintln!("--- LLM raw response ---\n{raw}\n");
        eprintln!(
            "--- parsed actions: {} total, {} create_skill ---",
            actions.len(),
            create_count
        );
        for (i, action) in actions.iter().enumerate() {
            match action {
                AssistantAction::CreateSkill { description, .. } => {
                    eprintln!("  [{i}] create_skill: {description}");
                }
                AssistantAction::Reply { text } => {
                    eprintln!("  [{i}] reply: {}...", &text[..text.len().min(80)]);
                }
                AssistantAction::AskQuestion { question, .. } => {
                    eprintln!("  [{i}] ask_question: {question}");
                }
                other => {
                    eprintln!("  [{i}] {:?}", other);
                }
            }
        }
        assert!(
            create_count >= 3,
            "expected >=3 create_skill actions, got {create_count}"
        );
    }
}
