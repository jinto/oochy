use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use kittypaw_core::config::Config;
use kittypaw_core::error::Result;
use kittypaw_core::registry::RegistryEntry;
use kittypaw_core::types::{
    now_timestamp, AgentState, ConversationTurn, Event, EventType, LlmMessage, Role,
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
2. **Creating new skills** when nothing exists — by asking clarifying questions first
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
- You can chain multiple actions: search first, then reply based on results
- When a user describes an automation need, ALWAYS search the registry first before creating
- Ask clarifying questions when the request is ambiguous
- Save preferences when you learn something reusable (location, preferred channels, schedule patterns)
- Preference keys should be descriptive: "preferred_channel", "location", "wake_up_time", etc.
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
    if config.admin_chat_ids.is_empty() {
        // Desktop/CLI are always admin, remote channels need explicit config
        return matches!(event.event_type, EventType::Desktop);
    }
    let chat_id = extract_chat_id(event);
    config.admin_chat_ids.iter().any(|id| id == &chat_id)
}

/// Run one turn of the assistant conversation.
pub async fn run_assistant_turn(ctx: &AssistantContext<'_>) -> Result<AssistantTurn> {
    let agent_id = assistant_id_for_event(ctx.event);

    // Load or create agent state — ensure agent exists in DB before adding turns
    let mut state = {
        let s = ctx.store.lock().unwrap();
        match s.load_state(&agent_id)? {
            Some(existing) => existing,
            None => {
                let new_state = AgentState::new(&agent_id, SYSTEM_PROMPT);
                s.save_state(&new_state)?;
                new_state
            }
        }
    };

    // Load user context for personalization
    let user_context = {
        let s = ctx.store.lock().unwrap();
        s.list_shared_context().unwrap_or_default()
    };

    // Build messages
    let event_text = extract_text(ctx.event);
    let messages = build_messages(&state, &event_text, &user_context, ctx.registry_entries);

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
        let s = ctx.store.lock().unwrap();
        s.add_turn(&agent_id, &user_turn)?;
    }

    // Call LLM
    let raw_response = if let Some(ref cb) = ctx.on_token {
        ctx.provider.generate_stream(&messages, cb.clone()).await?
    } else {
        ctx.provider.generate(&messages).await?
    };

    // Parse actions from response
    let actions = parse_actions(&raw_response);
    let mut response_parts: Vec<String> = Vec::new();
    let mut actions_taken: Vec<AssistantAction> = Vec::new();

    for action in &actions {
        match action {
            AssistantAction::Reply { text } => {
                response_parts.push(text.clone());
                actions_taken.push(action.clone());
            }
            AssistantAction::SearchRegistry { query } => {
                let results = search_entries(ctx.registry_entries, query);
                if results.is_empty() {
                    // No matches — feed back to LLM for follow-up
                    response_parts.push(format!("(레지스트리 검색 '{query}': 결과 없음)"));
                } else {
                    let listing: Vec<String> = results
                        .iter()
                        .map(|e| format!("- **{}** ({}): {}", e.name, e.id, e.description))
                        .collect();
                    response_parts.push(format!("관련 스킬을 찾았어요:\n{}", listing.join("\n")));
                }
                actions_taken.push(action.clone());
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
                actions_taken.push(action.clone());
            }
            AssistantAction::CreateSkill {
                description,
                schedule,
            } => {
                // Admin check for skill creation
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
                actions_taken.push(action.clone());
            }
            AssistantAction::SavePreference { key, value } => {
                let s = ctx.store.lock().unwrap();
                let _ = s.set_user_context(key, value, "assistant");
                actions_taken.push(action.clone());
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
                actions_taken.push(action.clone());
            }
        }
    }

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
        let s = ctx.store.lock().unwrap();
        s.add_turn(&agent_id, &assistant_turn)?;
        s.save_state(&state)?;
    }

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

    // Add conversation history (last 20 turns)
    for turn in state.recent_turns(20) {
        match turn.role {
            Role::User => {
                messages.push(LlmMessage {
                    role: Role::User,
                    content: turn.content.clone(),
                });
            }
            Role::Assistant => {
                // Feed back the raw actions JSON if available, otherwise the text
                let content = turn.result.clone().unwrap_or_else(|| turn.content.clone());
                messages.push(LlmMessage {
                    role: Role::Assistant,
                    content,
                });
            }
            Role::System => {}
        }
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

fn assistant_id_for_event(event: &Event) -> String {
    let suffix = match event.event_type {
        EventType::Telegram => event
            .payload
            .get("chat_id")
            .map(|v| {
                v.as_str()
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| v.to_string())
            })
            .unwrap_or_else(|| "default".to_string()),
        EventType::WebChat => event
            .payload
            .get("session_id")
            .and_then(|v| v.as_str())
            .unwrap_or("default")
            .to_string(),
        EventType::Desktop => event
            .payload
            .get("workspace_id")
            .and_then(|v| v.as_str())
            .unwrap_or("default")
            .to_string(),
    };
    format!("assistant-{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(assistant_id_for_event(&event), "assistant-12345");

        let event = Event {
            event_type: EventType::Desktop,
            payload: serde_json::json!({"text": "hello"}),
        };
        assert_eq!(assistant_id_for_event(&event), "assistant-default");
    }
}
