use kittypaw_core::types::{AgentState, Event, EventType, ExecutionResult, LlmMessage, Role};

pub(super) fn build_prompt(
    state: &AgentState,
    event_text: &str,
    config: &crate::compaction::CompactionConfig,
    app_config: &kittypaw_core::config::Config,
    channel_type: &str,
    active_profile_override: Option<&str>,
    memory_context: &[String],
) -> Vec<LlmMessage> {
    use crate::compaction::{compact_turns, CompactionMode};

    // Build system prompt with auto-generated skills section
    let skills_section = crate::skill_registry::build_skills_prompt();
    let system_prompt = super::SYSTEM_PROMPT.replace("{{SKILLS_SECTION}}", &skills_section);

    let mut messages = vec![LlmMessage {
        role: Role::System,
        content: system_prompt,
    }];

    // Inject profile (SOUL.md + USER.md)
    let profile_name = kittypaw_core::profile::resolve_profile_name(
        app_config,
        channel_type,
        active_profile_override,
    );
    let profile = kittypaw_core::profile::load_profile(&profile_name);
    {
        let nick = app_config
            .profiles
            .iter()
            .find(|p| p.id == profile_name)
            .map(|p| p.nick.as_str())
            .unwrap_or("");

        if !profile.soul.is_empty() {
            messages.push(LlmMessage {
                role: Role::System,
                content: format!("## Your Identity (SOUL.md)\n{}", profile.soul),
            });
        }
        if !nick.is_empty() {
            messages.push(LlmMessage {
                role: Role::System,
                content: format!("Your name/nickname is: {nick}"),
            });
        }
        if !profile.user_md.is_empty() {
            messages.push(LlmMessage {
                role: Role::System,
                content: format!("## User Profile (USER.md)\n{}", profile.user_md),
            });
        }
    }

    // Inject memory context (user facts, recent failures, today's stats)
    // Dedup: remove DB entries whose keys already appear in USER.md
    if !memory_context.is_empty() {
        let user_keys = kittypaw_core::profile::extract_user_md_keys(&profile.user_md);
        let deduped: Vec<String> = if user_keys.is_empty() {
            memory_context.to_vec()
        } else {
            memory_context
                .iter()
                .map(|section| {
                    if !section.starts_with("## Remembered Facts") {
                        return section.clone();
                    }
                    let lines: Vec<&str> = section
                        .lines()
                        .filter(|line| {
                            if let Some(rest) = line.strip_prefix("- ") {
                                if let Some(colon) = rest.find(": ") {
                                    return !user_keys.contains(&rest[..colon]);
                                }
                            }
                            true // keep header and non-kv lines
                        })
                        .collect();
                    // If only the header remains, skip the section
                    if lines.len() <= 1 {
                        String::new()
                    } else {
                        lines.join("\n")
                    }
                })
                .filter(|s| !s.is_empty())
                .collect()
        };
        if !deduped.is_empty() {
            messages.push(LlmMessage {
                role: Role::System,
                content: deduped.join("\n\n"),
            });
        }
    }

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

pub(super) fn format_event(event: &Event) -> String {
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
                .map(|v| match v.as_str() {
                    Some(s) => s.to_string(),
                    None => v.to_string(), // handles i64 chat_id
                })
                .unwrap_or_default();
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
        EventType::KakaoTalk => {
            let text = payload.get("text").and_then(|v| v.as_str()).unwrap_or("");
            let user_id = payload
                .get("user_id")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            format!("[KakaoTalk] (user_id={user_id}): {text}")
        }
    }
}

pub(super) fn format_exec_result(result: &ExecutionResult) -> String {
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
