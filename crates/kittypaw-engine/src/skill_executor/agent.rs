use kittypaw_core::error::{KittypawError, Result};
use kittypaw_core::types::SkillCall;

const MAX_DELEGATE_DEPTH: u32 = 2;
const MAX_TASK_LENGTH: usize = 4096;

pub(super) async fn execute_agent(
    call: &SkillCall,
    config: &kittypaw_core::config::Config,
) -> Result<serde_json::Value> {
    match call.method.as_str() {
        "delegate" => {
            let task = call.args.first().and_then(|v| v.as_str()).unwrap_or("");
            if task.is_empty() {
                return Err(KittypawError::Sandbox(
                    "Agent.delegate: task description is required".into(),
                ));
            }
            if task.len() > MAX_TASK_LENGTH {
                return Err(KittypawError::Sandbox(format!(
                    "Agent.delegate: task exceeds {MAX_TASK_LENGTH} character limit"
                )));
            }

            // Check delegate depth from context (passed via args[1] if present)
            let depth = call.args.get(1).and_then(|v| v.as_u64()).unwrap_or(0) as u32;
            if depth >= MAX_DELEGATE_DEPTH {
                return Err(KittypawError::Sandbox(format!(
                    "Agent.delegate: max depth ({MAX_DELEGATE_DEPTH}) exceeded"
                )));
            }

            // Build LLM provider
            let registry = if !config.models.is_empty() {
                let mut models = config.models.clone();
                if !config.llm.api_key.is_empty() {
                    for model in &mut models {
                        if model.api_key.is_empty() {
                            model.api_key = config.llm.api_key.clone();
                        }
                    }
                }
                kittypaw_llm::registry::LlmRegistry::from_configs(&models)
            } else if !config.llm.api_key.is_empty() {
                let legacy = kittypaw_core::config::ModelConfig {
                    name: config.llm.provider.clone(),
                    provider: config.llm.provider.clone(),
                    model: config.llm.model.clone(),
                    api_key: config.llm.api_key.clone(),
                    max_tokens: config.llm.max_tokens,
                    default: true,
                    base_url: None,
                    context_window: None,
                    tier: None,
                };
                kittypaw_llm::registry::LlmRegistry::from_configs(&[legacy])
            } else {
                return Err(KittypawError::Sandbox(
                    "Agent.delegate: no LLM provider configured".into(),
                ));
            };

            let provider = match registry.default_provider() {
                Some(p) => p,
                None => {
                    return Err(KittypawError::Sandbox(
                        "Agent.delegate: no default LLM provider available".into(),
                    ));
                }
            };

            // Send task to LLM
            let messages = vec![
                kittypaw_core::types::LlmMessage {
                    role: kittypaw_core::types::Role::System,
                    content: format!(
                        "{}\n\nYou are a sub-agent. Complete the delegated task and return the result as plain text.",
                        crate::agent_loop::SYSTEM_PROMPT
                    ),
                },
                kittypaw_core::types::LlmMessage {
                    role: kittypaw_core::types::Role::User,
                    content: task.to_string(),
                },
            ];

            match provider.generate(&messages).await {
                Ok(resp) => Ok(serde_json::json!({
                    "result": resp.content,
                    "success": true,
                })),
                Err(e) => Err(KittypawError::Skill(format!(
                    "Agent.delegate: LLM call failed: {e}"
                ))),
            }
        }
        _ => Err(KittypawError::Sandbox(format!(
            "Unknown Agent method: {}",
            call.method
        ))),
    }
}
