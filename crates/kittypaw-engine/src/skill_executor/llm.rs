use std::sync::atomic::{AtomicU32, Ordering};

use kittypaw_core::error::{KittypawError, Result};
use kittypaw_core::types::SkillCall;

use super::LLM_MAX_CALLS_PER_EXECUTION;

pub(super) async fn execute_llm(
    call: &SkillCall,
    config: &kittypaw_core::config::Config,
    llm_call_count: &AtomicU32,
    model_override: Option<&str>,
) -> Result<serde_json::Value> {
    let count = llm_call_count.fetch_add(1, Ordering::Relaxed);
    if count >= LLM_MAX_CALLS_PER_EXECUTION {
        return Err(KittypawError::Skill(
            "Llm recursion limit exceeded (max 3 calls per execution)".into(),
        ));
    }

    let prompt = call.args.first().and_then(|v| v.as_str()).unwrap_or("");

    if prompt.is_empty() {
        return Err(KittypawError::Skill(
            "Llm.generate: prompt is required".into(),
        ));
    }

    let max_tokens = call.args.get(1).and_then(|v| v.as_u64()).unwrap_or(1024) as u32;

    // Resolve model config: if a model override name is provided and matches a registered
    // model in config.models, use that model's credentials; otherwise fall back to default.
    let (api_key, model, base_url, provider) =
        if let Some(name) = model_override.filter(|s| !s.is_empty()) {
            if let Some(mc) = config.models.iter().find(|m| m.name == name) {
                let key = if mc.api_key.is_empty() {
                    kittypaw_core::secrets::get_secret("models", &mc.name)
                        .ok()
                        .flatten()
                        .unwrap_or_default()
                } else {
                    mc.api_key.clone()
                };
                (
                    key,
                    mc.model.clone(),
                    mc.base_url.clone(),
                    mc.provider.clone(),
                )
            } else {
                tracing::warn!(
                    "model_override '{}' not found in config.models, using default",
                    name
                );
                (
                    config.llm.api_key.clone(),
                    config.llm.model.clone(),
                    None,
                    config.llm.provider.clone(),
                )
            }
        } else {
            (
                config.llm.api_key.clone(),
                config.llm.model.clone(),
                None,
                config.llm.provider.clone(),
            )
        };

    let provider_lower = provider.to_lowercase();

    if api_key.is_empty() && !matches!(provider_lower.as_str(), "ollama" | "local") {
        return Err(KittypawError::Skill(format!(
            "No API key configured for model '{}'",
            model
        )));
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| KittypawError::Skill(format!("Llm client build error: {e}")))?;

    // Route to provider-specific endpoint
    let is_openai_compat = matches!(provider_lower.as_str(), "openai" | "ollama" | "local");
    let endpoint = if is_openai_compat {
        let url = base_url
            .as_deref()
            .unwrap_or("https://api.openai.com/v1")
            .trim_end_matches('/');
        format!("{url}/chat/completions")
    } else {
        "https://api.anthropic.com/v1/messages".to_string()
    };

    let resp = if is_openai_compat {
        let mut req = client
            .post(&endpoint)
            .header("content-type", "application/json")
            .json(&serde_json::json!({
                "model": model,
                "max_tokens": max_tokens,
                "messages": [{"role": "user", "content": prompt}]
            }));
        if !api_key.is_empty() {
            req = req.header("Authorization", format!("Bearer {api_key}"));
        }
        req.send()
            .await
            .map_err(|e| KittypawError::Skill(format!("Llm API error: {e}")))?
    } else {
        client
            .post(&endpoint)
            .header("x-api-key", &api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&serde_json::json!({
                "model": model,
                "max_tokens": max_tokens,
                "messages": [{"role": "user", "content": prompt}]
            }))
            .send()
            .await
            .map_err(|e| KittypawError::Skill(format!("Llm API error: {e}")))?
    };

    let status = resp.status();
    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| KittypawError::Skill(format!("Llm response parse error: {e}")))?;

    if !status.is_success() {
        let err = body
            .get("error")
            .and_then(|v| v.get("message"))
            .and_then(|v| v.as_str())
            .or_else(|| body.get("error").and_then(|v| v.as_str()))
            .unwrap_or("unknown error");
        return Err(KittypawError::Skill(format!(
            "Llm API error {status}: {err}"
        )));
    }

    let text = if is_openai_compat {
        body["choices"]
            .as_array()
            .and_then(|arr| arr.first())
            .and_then(|c| c["message"]["content"].as_str())
            .unwrap_or("")
            .to_string()
    } else {
        body["content"]
            .as_array()
            .and_then(|arr| arr.first())
            .and_then(|block| block["text"].as_str())
            .unwrap_or("")
            .to_string()
    };

    // Extract usage from response (works for both OpenAI and Claude formats)
    let input_tokens = body["usage"]["input_tokens"]
        .as_u64()
        .or_else(|| body["usage"]["prompt_tokens"].as_u64())
        .unwrap_or(0);
    let output_tokens = body["usage"]["output_tokens"]
        .as_u64()
        .or_else(|| body["usage"]["completion_tokens"].as_u64())
        .unwrap_or(0);

    Ok(serde_json::json!({
        "text": text,
        "usage": {
            "input_tokens": input_tokens,
            "output_tokens": output_tokens,
            "model": model
        }
    }))
}
