use oochy_core::config::Config;
use oochy_core::error::{OochyError, Result};
use oochy_core::skill::{Skill, SkillPermissions, SkillTrigger};
use oochy_core::types::{LlmMessage, Role};
use oochy_llm::provider::LlmProvider;
use oochy_llm::util::strip_code_fences;
use oochy_sandbox::sandbox::Sandbox;

const TEACH_PROMPT: &str = r#"You are Oochy's skill generator. The user describes an automation they want, and you write a reusable JavaScript skill.

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
    // Admin check
    if !config.admin_chat_ids.is_empty()
        && !config.admin_chat_ids.iter().any(|id| id == chat_id)
    {
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
            content: format!(
                "Create a skill for: {teach_text}\n\nThe chat_id is: {chat_id}"
            ),
        },
    ];

    let raw_code = provider.generate(&messages).await?;
    let code = strip_code_fences(&raw_code);

    // Dry-run in sandbox with mock context
    let mock_context = serde_json::json!({
        "event_type": "telegram",
        "event_text": teach_text,
        "chat_id": chat_id,
    });

    let exec_result = sandbox.execute(&code, mock_context).await?;

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
    let trigger = SkillTrigger {
        trigger_type: "message".into(),
        keyword: Some(skill_name.clone()),
        cron: None,
        natural: None,
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
            };
            oochy_core::skill::save_skill(&skill, code)?;
            tracing::info!("Skill '{}' saved successfully", skill_name);
            Ok(())
        }
        TeachResult::Error(e) => Err(OochyError::Skill(format!(
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
    let duration = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}", duration.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_slugify_description() {
        assert_eq!(slugify_description("send a daily joke"), "send-a-daily-joke");
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
}
