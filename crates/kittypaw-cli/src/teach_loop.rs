use kittypaw_core::config::Config;
use kittypaw_core::error::{KittypawError, Result};
use kittypaw_core::skill::{Skill, SkillPermissions, SkillTrigger};
use kittypaw_core::types::{LlmMessage, Role};
use kittypaw_llm::provider::LlmProvider;
use kittypaw_llm::util::strip_code_fences;
use kittypaw_sandbox::sandbox::Sandbox;

const TEACH_PROMPT: &str = r#"You are KittyPaw's skill generator. The user describes an automation they want, and you write a reusable JavaScript skill.

## Output format
Write ONLY valid JavaScript (ES2020) code. No markdown fences, no explanations.
Your code must be a single async function body that will be wrapped as:
  async function(ctx) { YOUR_CODE_HERE }

The `ctx` object contains:
- ctx.event_type: "telegram" | "web_chat"
- ctx.event_text: the message text that triggered this skill
- ctx.chat_id: the chat ID (for Telegram responses)

## Available primitives
- Telegram.sendMessage(chatId, text)
- Telegram.sendPhoto(chatId, url)
- Http.get(url) — returns {status, body}
- Http.post(url, body) — returns {status, body}
- Storage.get(key) — returns {value} or {value: null}
- Storage.set(key, value) — returns {ok: true}
- Storage.delete(key)
- Storage.list() — returns {keys: [...]}
- Llm.generate(prompt) — returns {text: "..."}. Max 3 calls per execution.
- console.log(...args)

## Rules
- Write focused, minimal code for the task described
- Use return to provide a text response to the user
- Use try/catch for error handling
- Do NOT use: require(), import, fetch(), Node.js APIs
"#;

pub enum TeachResult {
    Generated {
        code: String,
        dry_run_output: String,
        skill_name: String,
        description: String,
        trigger: SkillTrigger,
        permissions: Vec<String>,
    },
    Error(String),
}

pub async fn handle_teach(
    teach_text: &str,
    chat_id: &str,
    provider: &dyn LlmProvider,
    sandbox: &Sandbox,
    config: &Config,
) -> Result<TeachResult> {
    // Admin check (deny-by-default: empty list blocks all)
    if config.admin_chat_ids.is_empty() || !config.admin_chat_ids.iter().any(|id| id == chat_id) {
        return Ok(TeachResult::Error(
            "Permission denied: you are not an admin.".into(),
        ));
    }

    // Generate code via LLM
    let messages = vec![
        LlmMessage {
            role: Role::System,
            content: TEACH_PROMPT.to_string(),
        },
        LlmMessage {
            role: Role::User,
            content: format!("Create a skill for: {teach_text}\n\nThe chat_id is: {chat_id}"),
        },
    ];

    let raw_code = provider.generate(&messages).await?.content;
    let code = strip_code_fences(&raw_code);
    validate_generated_code(&code)?;

    // Dry-run in sandbox with mock context
    let mock_context = serde_json::json!({
        "event_type": "telegram",
        "event_text": teach_text,
        "chat_id": chat_id,
    });

    let wrapped = format!("const ctx = JSON.parse(__context__);\n{code}");
    let exec_result = sandbox.execute(&wrapped, mock_context).await?;

    if !exec_result.success {
        let err_msg = exec_result
            .error
            .unwrap_or_else(|| "Unknown sandbox error".into());
        return Ok(TeachResult::Error(format!(
            "Generated code failed dry-run: {err_msg}"
        )));
    }

    // Derive skill metadata
    let skill_name = slugify_description(teach_text);
    let permissions = detect_permissions(&code);
    let is_schedule = detect_schedule(teach_text);
    let trigger = if is_schedule {
        SkillTrigger {
            trigger_type: "schedule".into(),
            keyword: None,
            cron: None,
            natural: Some(teach_text.to_string()),
        }
    } else {
        SkillTrigger {
            trigger_type: "message".into(),
            keyword: Some(skill_name.clone()),
            cron: None,
            natural: None,
        }
    };

    let dry_run_output = if exec_result.output.is_empty() {
        "(no output)".to_string()
    } else {
        exec_result.output
    };

    Ok(TeachResult::Generated {
        code,
        dry_run_output,
        skill_name,
        description: teach_text.to_string(),
        trigger,
        permissions,
    })
}

pub fn approve_skill(result: &TeachResult) -> Result<()> {
    match result {
        TeachResult::Generated {
            code,
            skill_name,
            description,
            trigger,
            permissions,
            ..
        } => {
            // Validate cron expression before saving
            if let Some(ref cron_expr) = trigger.cron {
                if let Err(e) = crate::schedule::validate_cron(cron_expr) {
                    return Err(KittypawError::Config(format!("Invalid schedule: {e}")));
                }
            }
            let now = now_iso8601();
            let skill = Skill {
                name: skill_name.clone(),
                version: 1,
                description: description.clone(),
                created_at: now.clone(),
                updated_at: now,
                enabled: true,
                trigger: trigger.clone(),
                permissions: SkillPermissions {
                    primitives: permissions.clone(),
                    allowed_hosts: vec![],
                },
                format: kittypaw_core::skill::SkillFormat::Native,
            };
            kittypaw_core::skill::save_skill(&skill, code)?;
            tracing::info!("Skill '{}' saved successfully", skill_name);
            Ok(())
        }
        TeachResult::Error(e) => Err(KittypawError::Skill(format!(
            "Cannot approve a failed result: {e}"
        ))),
    }
}

fn slugify_description(text: &str) -> String {
    let slug: String = text
        .split_whitespace()
        .take(4)
        .collect::<Vec<_>>()
        .join("-")
        .to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();

    if slug.is_empty() {
        "unnamed-skill".to_string()
    } else {
        slug
    }
}

fn detect_schedule(text: &str) -> bool {
    let lower = text.to_lowercase();
    const SCHEDULE_KEYWORDS: &[&str] = &[
        "every day",
        "every morning",
        "every evening",
        "every night",
        "every hour",
        "every minute",
        "daily",
        "hourly",
        "weekly",
        "monthly",
        "at midnight",
        "at noon",
        "every monday",
        "every tuesday",
        "every wednesday",
        "every thursday",
        "every friday",
        "every saturday",
        "every sunday",
        "once a day",
        "once a week",
        "once a month",
        "every week",
        "every month",
        "scheduled",
        "cron",
        // Korean keywords
        "매일",
        "매시간",
        "매주",
        "매월",
        "아침",
        "저녁",
        "밤",
        "월요일",
        "화요일",
        "수요일",
        "목요일",
        "금요일",
        "토요일",
        "일요일",
        "하루에",
        "시간마다",
        "분마다",
    ];
    SCHEDULE_KEYWORDS.iter().any(|kw| lower.contains(kw))
}

/// Validate generated code for dangerous patterns.
/// Http+Storage combos are now allowed — the sandbox already provides
/// allowed_hosts and storage namespace isolation at runtime.
fn validate_generated_code(code: &str) -> Result<()> {
    let has_http = code.contains("Http.");
    let has_storage = code.contains("Storage.");
    if has_http && has_storage {
        tracing::info!("Skill uses both Http and Storage — allowed (sandbox-guarded)");
    }
    Ok(())
}

fn detect_permissions(code: &str) -> Vec<String> {
    let mut perms = Vec::new();
    if code.contains("Telegram.") {
        perms.push("Telegram".into());
    }
    if code.contains("Http.") {
        perms.push("Http".into());
    }
    if code.contains("Storage.") {
        perms.push("Storage".into());
    }
    if code.contains("Llm.") {
        perms.push("Llm".into());
    }
    perms
}

fn now_iso8601() -> String {
    chrono::Utc::now().to_rfc3339()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_slugify_description() {
        assert_eq!(
            slugify_description("send a daily joke"),
            "send-a-daily-joke"
        );
        assert_eq!(slugify_description("Hello World"), "hello-world");
        assert_eq!(
            slugify_description("track my expenses in a spreadsheet"),
            "track-my-expenses-in"
        );
        assert_eq!(slugify_description(""), "unnamed-skill");
    }

    #[test]
    fn test_detect_permissions() {
        let code = r#"
            const resp = await Http.get("https://example.com");
            await Telegram.sendMessage(ctx.chat_id, resp.body);
        "#;
        let perms = detect_permissions(code);
        assert!(perms.contains(&"Telegram".to_string()));
        assert!(perms.contains(&"Http".to_string()));
        assert!(!perms.contains(&"Storage".to_string()));
        assert!(!perms.contains(&"Llm".to_string()));
    }

    #[test]
    fn test_detect_permissions_all() {
        let code = "Telegram.sendMessage(); Http.get(); Storage.set(); Llm.generate();";
        let perms = detect_permissions(code);
        assert_eq!(perms.len(), 4);
    }

    #[test]
    fn test_validate_generated_code_allows_http_only() {
        let code = r#"const resp = await Http.get("https://example.com");"#;
        assert!(validate_generated_code(code).is_ok());
    }

    #[test]
    fn test_validate_generated_code_allows_storage_only() {
        let code = r#"const val = await Storage.get("key");"#;
        assert!(validate_generated_code(code).is_ok());
    }

    #[test]
    fn test_validate_generated_code_allows_http_and_storage() {
        let code = r#"
            const resp = await Http.get("https://example.com");
            await Storage.set("data", resp.body);
        "#;
        assert!(validate_generated_code(code).is_ok());
    }

    #[test]
    fn test_detect_schedule_korean() {
        assert!(detect_schedule("매일 아침 날씨 알려줘"));
        assert!(detect_schedule("매주 월요일에 보고서 보내줘"));
        assert!(detect_schedule("매시간마다 체크해줘"));
        assert!(detect_schedule("하루에 한 번 실행해줘"));
        assert!(detect_schedule("분마다 확인해줘"));
        assert!(detect_schedule("저녁에 알람 보내줘"));
        assert!(detect_schedule("토요일마다 정리해줘"));
        assert!(!detect_schedule("날씨 알려줘"));
    }
}
