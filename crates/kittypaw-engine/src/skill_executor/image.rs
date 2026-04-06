use kittypaw_core::error::{KittypawError, Result};
use kittypaw_core::types::SkillCall;

/// Image generation and vision analysis primitives.
pub(super) async fn execute_image(
    call: &SkillCall,
    config: &kittypaw_core::config::Config,
) -> Result<serde_json::Value> {
    match call.method.as_str() {
        "generate" => {
            let prompt =
                call.args.first().and_then(|v| v.as_str()).ok_or_else(|| {
                    KittypawError::Skill("Image.generate: prompt required".into())
                })?;

            // Use OpenAI DALL-E API via Http
            let api_key = resolve_api_key(config)?;
            let client = reqwest::Client::new();
            let resp = client
                .post("https://api.openai.com/v1/images/generations")
                .header("Authorization", format!("Bearer {api_key}"))
                .json(&serde_json::json!({
                    "model": "dall-e-3",
                    "prompt": prompt,
                    "n": 1,
                    "size": "1024x1024",
                }))
                .send()
                .await
                .map_err(|e| KittypawError::Skill(format!("Image API error: {e}")))?;

            let status = resp.status();
            let body: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| KittypawError::Skill(format!("Image response error: {e}")))?;
            if !status.is_success() {
                let err = body["error"]["message"].as_str().unwrap_or("unknown");
                return Err(KittypawError::Skill(format!("Image API {status}: {err}")));
            }

            let url = body["data"][0]["url"].as_str().unwrap_or("").to_string();
            if url.is_empty() {
                let err = body["error"]["message"].as_str().unwrap_or("unknown error");
                return Err(KittypawError::Skill(format!(
                    "Image generation failed: {err}"
                )));
            }

            Ok(serde_json::json!({ "url": url }))
        }
        _ => Err(KittypawError::CapabilityDenied(format!(
            "Unknown Image method: {}",
            call.method
        ))),
    }
}

/// Vision analysis using multimodal LLM.
pub(super) async fn execute_vision(
    call: &SkillCall,
    config: &kittypaw_core::config::Config,
) -> Result<serde_json::Value> {
    match call.method.as_str() {
        "analyze" => {
            let image_url =
                call.args.first().and_then(|v| v.as_str()).ok_or_else(|| {
                    KittypawError::Skill("Vision.analyze: imageUrl required".into())
                })?;
            let prompt = call
                .args
                .get(1)
                .and_then(|v| v.as_str())
                .unwrap_or("Describe this image in detail.");

            // Use OpenAI Vision API
            let api_key = resolve_api_key(config)?;
            let client = reqwest::Client::new();
            let resp = client
                .post("https://api.openai.com/v1/chat/completions")
                .header("Authorization", format!("Bearer {api_key}"))
                .json(&serde_json::json!({
                    "model": "gpt-4o",
                    "messages": [{
                        "role": "user",
                        "content": [
                            { "type": "text", "text": prompt },
                            { "type": "image_url", "image_url": { "url": image_url } }
                        ]
                    }],
                    "max_tokens": 1024,
                }))
                .send()
                .await
                .map_err(|e| KittypawError::Skill(format!("Vision API error: {e}")))?;

            let status = resp.status();
            let body: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| KittypawError::Skill(format!("Vision response error: {e}")))?;
            if !status.is_success() {
                let err = body["error"]["message"].as_str().unwrap_or("unknown");
                return Err(KittypawError::Skill(format!("Vision API {status}: {err}")));
            }

            let analysis = body["choices"][0]["message"]["content"]
                .as_str()
                .unwrap_or("")
                .to_string();
            if analysis.is_empty() {
                let err = body["error"]["message"].as_str().unwrap_or("unknown error");
                return Err(KittypawError::Skill(format!(
                    "Vision analysis failed: {err}"
                )));
            }

            Ok(serde_json::json!({ "analysis": analysis }))
        }
        _ => Err(KittypawError::CapabilityDenied(format!(
            "Unknown Vision method: {}",
            call.method
        ))),
    }
}

fn resolve_api_key(config: &kittypaw_core::config::Config) -> Result<String> {
    // Try OpenAI model in config.models first
    if let Some(openai) = config
        .models
        .iter()
        .find(|m| m.provider == "openai" && !m.api_key.is_empty())
    {
        return Ok(openai.api_key.clone());
    }
    // Try env var
    if let Ok(key) = std::env::var("OPENAI_API_KEY") {
        return Ok(key);
    }
    // Try secrets
    if let Ok(Some(key)) = kittypaw_core::secrets::get_secret("models", "openai") {
        if !key.is_empty() {
            return Ok(key);
        }
    }
    Err(KittypawError::Config(
        "Image/Vision requires OpenAI API key (OPENAI_API_KEY or [[models]] provider='openai')"
            .into(),
    ))
}
