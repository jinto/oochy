use kittypaw_core::error::{KittypawError, Result};
use kittypaw_core::types::SkillCall;

pub(super) fn execute_memory(
    call: &SkillCall,
    store: &kittypaw_store::Store,
    profile_name: Option<&str>,
) -> Result<serde_json::Value> {
    match call.method.as_str() {
        "save" => {
            let key = call
                .args
                .first()
                .and_then(|v| v.as_str())
                .ok_or_else(|| KittypawError::Skill("Memory.save: key required".into()))?;
            let value = call
                .args
                .get(1)
                .and_then(|v| v.as_str())
                .ok_or_else(|| KittypawError::Skill("Memory.save: value required".into()))?;
            store.set_user_context(key, value, "memory")?;
            Ok(serde_json::json!({"saved": true, "key": key}))
        }
        "recall" => {
            let query = call.args.first().and_then(|v| v.as_str()).unwrap_or("");
            if query.is_empty() {
                // Return all memory entries
                let entries = store.list_shared_context()?;
                Ok(serde_json::json!(entries))
            } else {
                // Prefix search
                let entries = store.list_user_context_prefix(query)?;
                let map: serde_json::Map<String, serde_json::Value> = entries
                    .into_iter()
                    .map(|(k, v)| (k, serde_json::Value::String(v)))
                    .collect();
                Ok(serde_json::Value::Object(map))
            }
        }
        "user" => {
            // Update USER.md with a key-value pair
            let key = call
                .args
                .first()
                .and_then(|v| v.as_str())
                .ok_or_else(|| KittypawError::Skill("Memory.user: key required".into()))?;
            let value = call
                .args
                .get(1)
                .and_then(|v| v.as_str())
                .ok_or_else(|| KittypawError::Skill("Memory.user: value required".into()))?;

            let profile = profile_name.unwrap_or("default");
            let existing = kittypaw_core::profile::load_profile(profile);
            let mut user_md = existing.user_md;

            // Update or append key-value line
            let line = format!("- {key}: {value}");
            let key_prefix = format!("- {key}:");
            if let Some(pos) = user_md.find(&key_prefix) {
                // Replace existing line
                let end = user_md[pos..]
                    .find('\n')
                    .map(|i| pos + i + 1)
                    .unwrap_or(user_md.len());
                user_md.replace_range(pos..end, &format!("{line}\n"));
            } else {
                if !user_md.is_empty() && !user_md.ends_with('\n') {
                    user_md.push('\n');
                }
                user_md.push_str(&format!("{line}\n"));
            }
            kittypaw_core::profile::save_user_md(profile, &user_md)
                .map_err(|e| KittypawError::Skill(format!("Failed to save USER.md: {e}")))?;

            Ok(serde_json::json!({"saved": true, "key": key, "profile": profile}))
        }
        _ => Err(KittypawError::CapabilityDenied(format!(
            "Unknown Memory method: {}",
            call.method
        ))),
    }
}
