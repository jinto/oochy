use kittypaw_core::error::{KittypawError, Result};
use kittypaw_core::types::SkillCall;
use kittypaw_store::Store;

pub(super) fn execute_storage(
    call: &SkillCall,
    store: &Store,
    skill_context: Option<&str>,
) -> Result<serde_json::Value> {
    let namespace = skill_context.unwrap_or("default");

    match call.method.as_str() {
        "get" => {
            let key = call.args.first().and_then(|v| v.as_str()).unwrap_or("");
            match store.storage_get(namespace, key)? {
                Some(value) => Ok(serde_json::json!({ "value": value })),
                None => Ok(serde_json::json!({ "value": null })),
            }
        }
        "set" => {
            let key = call.args.first().and_then(|v| v.as_str()).unwrap_or("");
            let value = call.args.get(1).and_then(|v| v.as_str()).unwrap_or("");
            if key.is_empty() {
                return Err(KittypawError::Skill("Storage.set: key is required".into()));
            }
            store.storage_set(namespace, key, value)?;
            Ok(serde_json::json!({ "ok": true }))
        }
        "delete" => {
            let key = call.args.first().and_then(|v| v.as_str()).unwrap_or("");
            if key.is_empty() {
                return Err(KittypawError::Skill(
                    "Storage.delete: key is required".into(),
                ));
            }
            store.storage_delete(namespace, key)?;
            Ok(serde_json::json!({ "ok": true }))
        }
        "list" => {
            let keys = store.storage_list(namespace)?;
            Ok(serde_json::json!({ "keys": keys }))
        }
        _ => Err(KittypawError::Skill(format!(
            "Unknown Storage method: {}",
            call.method
        ))),
    }
}
