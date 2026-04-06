use kittypaw_core::error::{KittypawError, Result};
use kittypaw_core::types::{LlmMessage, Role, SkillCall};
use kittypaw_llm::registry::LlmRegistry;

/// Mixture of Agents: query multiple models in parallel, then aggregate.
pub(super) async fn execute_moa(
    call: &SkillCall,
    config: &kittypaw_core::config::Config,
) -> Result<serde_json::Value> {
    match call.method.as_str() {
        "query" => {
            let prompt = call
                .args
                .first()
                .and_then(|v| v.as_str())
                .ok_or_else(|| KittypawError::Skill("Moa.query: prompt required".into()))?;

            // Build registry from config
            let registry = build_registry(config);
            let model_names = registry.list();
            if model_names.is_empty() {
                return Err(KittypawError::Skill(
                    "Moa.query: no models configured in [[models]]".into(),
                ));
            }

            // Layer 1: Query all models in parallel
            let mut handles = Vec::new();
            for name in &model_names {
                if let Some(provider) = registry.get(name) {
                    let prompt = prompt.to_string();
                    let model_name = name.clone();
                    handles.push(tokio::spawn(async move {
                        let messages = vec![LlmMessage {
                            role: Role::User,
                            content: prompt,
                        }];
                        let result = provider.generate(&messages).await;
                        (model_name, result)
                    }));
                }
            }

            let mut responses = Vec::new();
            for handle in handles {
                if let Ok((name, Ok(resp))) = handle.await {
                    responses.push(format!("[{}]: {}", name, resp.content));
                }
            }

            if responses.is_empty() {
                return Err(KittypawError::Skill("Moa.query: all models failed".into()));
            }

            // If only 1 model responded, return directly
            if responses.len() == 1 {
                return Ok(serde_json::json!({
                    "result": responses[0],
                    "models_used": 1,
                }));
            }

            // Layer 2: Aggregate with the default model
            let aggregator = registry
                .default_provider()
                .ok_or_else(|| KittypawError::Skill("Moa: no default model".into()))?;

            let agg_prompt = format!(
                "다음은 여러 AI 모델의 응답입니다. 이들을 종합하여 가장 정확하고 완전한 답변을 작성하세요.\n\n{}\n\n종합 답변:",
                responses.join("\n\n")
            );

            let agg_messages = vec![LlmMessage {
                role: Role::User,
                content: agg_prompt,
            }];
            let agg_result = aggregator
                .generate(&agg_messages)
                .await
                .map_err(|e| KittypawError::Skill(format!("Moa aggregation failed: {e}")))?;

            Ok(serde_json::json!({
                "result": agg_result.content,
                "models_used": responses.len(),
            }))
        }
        _ => Err(KittypawError::CapabilityDenied(format!(
            "Unknown Moa method: {}",
            call.method
        ))),
    }
}

fn build_registry(config: &kittypaw_core::config::Config) -> LlmRegistry {
    if !config.models.is_empty() {
        let mut models = config.models.clone();
        if !config.llm.api_key.is_empty() {
            for model in &mut models {
                if model.api_key.is_empty()
                    && matches!(model.provider.as_str(), "claude" | "anthropic" | "openai")
                {
                    model.api_key = config.llm.api_key.clone();
                }
            }
        }
        LlmRegistry::from_configs(&models)
    } else {
        LlmRegistry::new()
    }
}
