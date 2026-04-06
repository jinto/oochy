use std::collections::HashMap;

use kittypaw_core::error::{KittypawError, Result};
use kittypaw_core::types::SkillCall;

pub(super) fn execute_env(
    call: &SkillCall,
    config_values: Option<&HashMap<String, String>>,
) -> Result<serde_json::Value> {
    match call.method.as_str() {
        "get" => {
            let key = call.args.first().and_then(|v| v.as_str()).unwrap_or("");
            if key.is_empty() {
                return Err(KittypawError::Sandbox("Env.get: key is required".into()));
            }
            // Read from package config, NOT from real environment variables
            let value = config_values.and_then(|m| m.get(key)).cloned();
            match value {
                Some(v) => Ok(serde_json::json!({ "value": v })),
                None => Ok(serde_json::json!({ "value": null })),
            }
        }
        _ => Err(KittypawError::Sandbox(format!(
            "Unknown Env method: {}",
            call.method
        ))),
    }
}
