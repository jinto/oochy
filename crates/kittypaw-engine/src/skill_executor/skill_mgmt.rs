use kittypaw_core::error::{KittypawError, Result};
use kittypaw_core::skill::{Skill, SkillFormat, SkillPermissions, SkillTrigger};
use kittypaw_core::types::SkillCall;

pub(super) async fn execute_skill_mgmt(call: &SkillCall) -> Result<serde_json::Value> {
    match call.method.as_str() {
        "create" => {
            let name = call.args.first().and_then(|v| v.as_str()).unwrap_or("");
            let description = call.args.get(1).and_then(|v| v.as_str()).unwrap_or("");
            let code = call.args.get(2).and_then(|v| v.as_str()).unwrap_or("");
            let trigger_type = call
                .args
                .get(3)
                .and_then(|v| v.as_str())
                .unwrap_or("message");
            let trigger_value = call.args.get(4).and_then(|v| v.as_str()).unwrap_or("");

            if name.is_empty() {
                return Err(KittypawError::Sandbox(
                    "Skill.create: name is required".into(),
                ));
            }
            if code.is_empty() {
                return Err(KittypawError::Sandbox(
                    "Skill.create: code is required".into(),
                ));
            }

            let trigger = if trigger_type == "schedule" {
                if trigger_value.is_empty() {
                    return Err(KittypawError::Sandbox(
                        "Skill.create: schedule trigger requires a schedule expression as the 5th argument (e.g. \"every 10m\" or \"*/10 * * * *\")".into(),
                    ));
                }
                // parse_schedule handles "every 10m", 5-field cron → 6-field conversion
                let cron_expr = crate::teach_loop::parse_schedule(trigger_value)?;
                SkillTrigger {
                    trigger_type: "schedule".into(),
                    cron: Some(cron_expr),
                    natural: Some(description.to_string()),
                    keyword: None,
                }
            } else {
                SkillTrigger {
                    trigger_type: "message".into(),
                    cron: None,
                    natural: None,
                    keyword: if trigger_value.is_empty() {
                        Some(name.to_string())
                    } else {
                        Some(trigger_value.to_string())
                    },
                }
            };

            // Detect permissions from code
            let mut perms = Vec::new();
            for prim in [
                "Http", "Web", "Telegram", "Slack", "Discord", "Storage", "Llm", "Shell", "Git",
                "File",
            ] {
                if code.contains(prim) {
                    perms.push(prim.to_string());
                }
            }

            let now = chrono::Utc::now().to_rfc3339();
            let skill = Skill {
                name: name.to_string(),
                version: 1,
                description: description.to_string(),
                created_at: now.clone(),
                updated_at: now,
                enabled: true,
                trigger,
                permissions: SkillPermissions {
                    primitives: perms,
                    allowed_hosts: vec![],
                },
                format: SkillFormat::Native,
            };

            kittypaw_core::skill::save_skill(&skill, code)?;
            tracing::info!(name = name, "Skill created by LLM");
            Ok(serde_json::json!({"ok": true, "name": name, "description": description}))
        }
        "list" => {
            let skills = kittypaw_core::skill::load_all_skills()?;
            let list: Vec<_> = skills
                .iter()
                .map(|(s, _)| {
                    serde_json::json!({
                        "name": s.name,
                        "description": s.description,
                        "enabled": s.enabled,
                        "trigger": s.trigger.trigger_type,
                    })
                })
                .collect();
            Ok(serde_json::json!({"skills": list}))
        }
        "delete" => {
            let name = call.args.first().and_then(|v| v.as_str()).unwrap_or("");
            if name.is_empty() {
                return Err(KittypawError::Sandbox(
                    "Skill.delete: name is required".into(),
                ));
            }
            // Archive before delete (version increment)
            let _ = kittypaw_core::skill::version_increment(name);
            tracing::info!(name = name, "Skill deleted by LLM");
            Ok(serde_json::json!({"ok": true}))
        }
        _ => Err(KittypawError::Sandbox(format!(
            "Unknown Skill method: {}",
            call.method
        ))),
    }
}
