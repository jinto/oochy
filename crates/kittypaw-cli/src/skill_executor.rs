use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use kittypaw_core::error::{KittypawError, Result};
use kittypaw_core::types::SkillCall;
use kittypaw_store::Store;

const LLM_MAX_CALLS_PER_EXECUTION: u32 = 3;

/// Execute a single skill call inline (for use as a SkillResolver callback).
/// Returns a string result that flows back to JS during sandbox execution.
pub async fn resolve_skill_call(
    call: &SkillCall,
    config: &kittypaw_core::config::Config,
    store: &Arc<Mutex<Store>>,
) -> String {
    // Storage calls need synchronous Store access
    if call.skill_name == "Storage" {
        let s = store.lock().unwrap();
        return match execute_storage(call, &s, None) {
            Ok(val) => serde_json::to_string(&val).unwrap_or_else(|_| "null".to_string()),
            Err(e) => serde_json::to_string(&serde_json::json!({"error": e.to_string()}))
                .unwrap_or_else(|_| "null".to_string()),
        };
    }

    let llm_call_count = AtomicU32::new(0);
    let result = execute_single_call(
        call,
        &config.sandbox.allowed_hosts,
        config,
        None,
        &llm_call_count,
    )
    .await;

    if result.success {
        serde_json::to_string(&result.result).unwrap_or_else(|_| "null".to_string())
    } else {
        serde_json::to_string(&serde_json::json!({"error": result.error}))
            .unwrap_or_else(|_| "null".to_string())
    }
}

/// Resolve storage skill calls synchronously using the store.
/// Returns a parallel Vec of Option<SkillResult> — Some for Storage calls, None for others.
/// This is separated from the async path so &Store is never captured in a Send future.
pub fn resolve_storage_calls(
    skill_calls: &[SkillCall],
    store: &Store,
    skill_context: Option<&str>,
) -> Vec<Option<SkillResult>> {
    skill_calls
        .iter()
        .map(|call| {
            if call.skill_name == "Storage" {
                Some(make_skill_result(
                    call,
                    execute_storage(call, store, skill_context),
                ))
            } else {
                None
            }
        })
        .collect()
}

/// Execute captured skill calls on the host side (outside sandbox).
/// Each skill call was captured by JS stubs inside QuickJS and is now
/// executed with real API calls after capability checking.
/// Storage calls must be pre-resolved via `resolve_storage_calls` and passed as `preresolved`.
pub async fn execute_skill_calls(
    skill_calls: &[SkillCall],
    config: &kittypaw_core::config::Config,
    preresolved: Vec<Option<SkillResult>>,
    skill_context: Option<&str>,
) -> Result<Vec<SkillResult>> {
    let allowed_hosts = &config.sandbox.allowed_hosts;
    // Per-execution LLM call counter (not global, avoids race between concurrent executions)
    let llm_call_count = AtomicU32::new(0);
    // Sequential execution: skill calls are ordered side-effects from JS.
    // Parallel would break ordering guarantees (message order, read-after-write).
    let mut results = Vec::new();
    for (call, precomp) in skill_calls.iter().zip(preresolved.into_iter()) {
        let result = if let Some(r) = precomp {
            r
        } else {
            execute_single_call(call, allowed_hosts, config, skill_context, &llm_call_count).await
        };
        results.push(result);
    }

    Ok(results)
}

fn make_skill_result(call: &SkillCall, res: Result<serde_json::Value>) -> SkillResult {
    match res {
        Ok(value) => SkillResult {
            skill_name: call.skill_name.clone(),
            method: call.method.clone(),
            success: true,
            result: value,
            error: None,
        },
        Err(e) => SkillResult {
            skill_name: call.skill_name.clone(),
            method: call.method.clone(),
            success: false,
            result: serde_json::Value::Null,
            error: Some(e.to_string()),
        },
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SkillResult {
    pub skill_name: String,
    pub method: String,
    pub success: bool,
    pub result: serde_json::Value,
    pub error: Option<String>,
}

async fn execute_single_call(
    call: &SkillCall,
    allowed_hosts: &[String],
    config: &kittypaw_core::config::Config,
    _skill_context: Option<&str>,
    llm_call_count: &AtomicU32,
) -> SkillResult {
    let result = match call.skill_name.as_str() {
        "Telegram" => execute_telegram(call).await,
        "Http" => execute_http(call, allowed_hosts).await,
        "Llm" => execute_llm(call, config, llm_call_count).await,
        _ => Err(KittypawError::CapabilityDenied(format!(
            "Unknown skill: {}",
            call.skill_name
        ))),
    };

    make_skill_result(call, result)
}

async fn execute_telegram(call: &SkillCall) -> Result<serde_json::Value> {
    let bot_token = std::env::var("KITTYPAW_TELEGRAM_TOKEN")
        .map_err(|_| KittypawError::Config("KITTYPAW_TELEGRAM_TOKEN not set".into()))?;

    let client = reqwest::Client::new();

    match call.method.as_str() {
        "sendMessage" => {
            let chat_id = call.args.first().and_then(|v| v.as_str()).unwrap_or("");
            let text = call.args.get(1).and_then(|v| v.as_str()).unwrap_or("");

            let url = format!("https://api.telegram.org/bot{bot_token}/sendMessage");
            let resp = client
                .post(&url)
                .json(&serde_json::json!({
                    "chat_id": chat_id,
                    "text": text,
                }))
                .send()
                .await
                .map_err(|e| KittypawError::Skill(format!("Telegram API error: {e}")))?;

            let body: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| KittypawError::Skill(format!("Telegram response parse error: {e}")))?;

            Ok(body)
        }
        "sendPhoto" => {
            let chat_id = call.args.first().and_then(|v| v.as_str()).unwrap_or("");
            let photo_url = call.args.get(1).and_then(|v| v.as_str()).unwrap_or("");

            let url = format!("https://api.telegram.org/bot{bot_token}/sendPhoto");
            let resp = client
                .post(&url)
                .json(&serde_json::json!({
                    "chat_id": chat_id,
                    "photo": photo_url,
                }))
                .send()
                .await
                .map_err(|e| KittypawError::Skill(format!("Telegram API error: {e}")))?;

            let body: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| KittypawError::Skill(format!("Telegram response parse error: {e}")))?;
            Ok(body)
        }
        _ => Err(KittypawError::CapabilityDenied(format!(
            "Unknown Telegram method: {}",
            call.method
        ))),
    }
}

fn validate_url(url_str: &str, allowed_hosts: &[String]) -> Result<()> {
    use std::net::IpAddr;

    let parsed =
        url::Url::parse(url_str).map_err(|_| KittypawError::Sandbox("Http: invalid URL".into()))?;

    // Block non-HTTP(S) schemes
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(KittypawError::Sandbox(
            "Http: only http/https schemes allowed".into(),
        ));
    }

    let host = parsed
        .host_str()
        .ok_or_else(|| KittypawError::Sandbox("Http: URL has no host".into()))?;

    // Block private/internal IPs (including IPv6-mapped IPv4)
    let addr_str = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = addr_str.parse::<IpAddr>() {
        let blocked = match ip {
            IpAddr::V4(v4) => {
                v4.is_loopback() || v4.is_private() || v4.is_link_local() || v4.is_unspecified()
            }
            IpAddr::V6(v6) => {
                v6.is_loopback() || v6.is_unspecified() || v6.is_multicast()
                    // ULA (fc00::/7) and link-local (fe80::/10)
                    || (v6.segments()[0] & 0xfe00) == 0xfc00  // fc00::/7
                    || (v6.segments()[0] & 0xffc0) == 0xfe80  // fe80::/10
                    || matches!(v6.to_ipv4_mapped(), Some(v4) if v4.is_loopback() || v4.is_private() || v4.is_link_local())
            }
        };
        if blocked {
            return Err(KittypawError::Sandbox(format!(
                "Http: blocked private/internal IP: {host}"
            )));
        }
    }

    // Block known private hostnames
    if matches!(host, "localhost" | "metadata.google.internal") {
        return Err(KittypawError::Sandbox(format!(
            "Http: blocked host: {host}"
        )));
    }

    // Check allowlist if configured
    if !allowed_hosts.is_empty() && !allowed_hosts.iter().any(|h| host.ends_with(h.as_str())) {
        return Err(KittypawError::Sandbox(format!(
            "Http: host '{host}' not in allowed_hosts"
        )));
    }

    Ok(())
}

async fn execute_http(call: &SkillCall, allowed_hosts: &[String]) -> Result<serde_json::Value> {
    // Disable redirects to prevent redirect-based SSRF bypass
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| KittypawError::Sandbox(format!("Http client build error: {e}")))?;
    let url = call.args.first().and_then(|v| v.as_str()).unwrap_or("");

    if url.is_empty() {
        return Err(KittypawError::Sandbox("Http: URL is required".into()));
    }

    validate_url(url, allowed_hosts)?;

    let resp = match call.method.as_str() {
        "get" => client.get(url).send().await,
        "post" => {
            let body = call.args.get(1).cloned().unwrap_or(serde_json::Value::Null);
            client.post(url).json(&body).send().await
        }
        "put" => {
            let body = call.args.get(1).cloned().unwrap_or(serde_json::Value::Null);
            client.put(url).json(&body).send().await
        }
        "delete" => client.delete(url).send().await,
        _ => {
            return Err(KittypawError::CapabilityDenied(format!(
                "Unknown Http method: {}",
                call.method
            )))
        }
    }
    .map_err(|e| KittypawError::Skill(format!("HTTP error: {e}")))?;

    let status = resp.status().as_u16();
    let body: serde_json::Value = resp
        .json()
        .await
        .unwrap_or(serde_json::Value::String("(non-JSON response)".into()));

    Ok(serde_json::json!({
        "status": status,
        "body": body,
    }))
}

fn execute_storage(
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
            store.storage_set(namespace, key, value)?;
            Ok(serde_json::json!({ "ok": true }))
        }
        "delete" => {
            let key = call.args.first().and_then(|v| v.as_str()).unwrap_or("");
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

async fn execute_llm(
    call: &SkillCall,
    config: &kittypaw_core::config::Config,
    llm_call_count: &AtomicU32,
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

    let api_key = &config.llm.api_key;
    let model = &config.llm.model;

    let client = reqwest::Client::new();
    let resp = client
        .post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(&serde_json::json!({
            "model": model,
            "max_tokens": max_tokens,
            "messages": [{"role": "user", "content": prompt}]
        }))
        .send()
        .await
        .map_err(|e| KittypawError::Skill(format!("Llm API error: {e}")))?;

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| KittypawError::Skill(format!("Llm response parse error: {e}")))?;

    let text = body["content"]
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|block| block["text"].as_str())
        .unwrap_or("")
        .to_string();

    Ok(serde_json::json!({ "text": text }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_db_path() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let mut p = std::env::temp_dir();
        p.push(format!(
            "kittypaw_skill_test_{}_{}.db",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        p
    }

    fn open_store(path: &std::path::Path) -> Store {
        Store::open(path.to_str().unwrap()).unwrap()
    }

    fn make_call(method: &str, args: Vec<serde_json::Value>) -> SkillCall {
        SkillCall {
            skill_name: "Storage".to_string(),
            method: method.to_string(),
            args,
        }
    }

    #[test]
    fn test_storage_set_and_get() {
        let path = temp_db_path();
        let store = open_store(&path);

        let call = make_call("set", vec![json_str("mykey"), json_str("myvalue")]);
        let result = execute_storage(&call, &store, None).unwrap();
        assert_eq!(result, serde_json::json!({ "ok": true }));

        let call = make_call("get", vec![json_str("mykey")]);
        let result = execute_storage(&call, &store, None).unwrap();
        assert_eq!(result, serde_json::json!({ "value": "myvalue" }));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_storage_get_nonexistent_key() {
        let path = temp_db_path();
        let store = open_store(&path);

        let call = make_call("get", vec![json_str("nokey")]);
        let result = execute_storage(&call, &store, None).unwrap();
        assert_eq!(result, serde_json::json!({ "value": null }));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_storage_set_overwrites() {
        let path = temp_db_path();
        let store = open_store(&path);

        let call = make_call("set", vec![json_str("k"), json_str("v1")]);
        execute_storage(&call, &store, None).unwrap();

        let call = make_call("set", vec![json_str("k"), json_str("v2")]);
        execute_storage(&call, &store, None).unwrap();

        let call = make_call("get", vec![json_str("k")]);
        let result = execute_storage(&call, &store, None).unwrap();
        assert_eq!(result, serde_json::json!({ "value": "v2" }));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_storage_delete() {
        let path = temp_db_path();
        let store = open_store(&path);

        let call = make_call("set", vec![json_str("k"), json_str("v")]);
        execute_storage(&call, &store, None).unwrap();

        let call = make_call("delete", vec![json_str("k")]);
        let result = execute_storage(&call, &store, None).unwrap();
        assert_eq!(result, serde_json::json!({ "ok": true }));

        let call = make_call("get", vec![json_str("k")]);
        let result = execute_storage(&call, &store, None).unwrap();
        assert_eq!(result, serde_json::json!({ "value": null }));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_storage_delete_nonexistent_key() {
        let path = temp_db_path();
        let store = open_store(&path);

        let call = make_call("delete", vec![json_str("nokey")]);
        let result = execute_storage(&call, &store, None).unwrap();
        assert_eq!(result, serde_json::json!({ "ok": true }));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_storage_list_empty() {
        let path = temp_db_path();
        let store = open_store(&path);

        let call = make_call("list", vec![]);
        let result = execute_storage(&call, &store, None).unwrap();
        let empty: Vec<String> = vec![];
        assert_eq!(result, serde_json::json!({ "keys": empty }));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_storage_list_with_keys() {
        let path = temp_db_path();
        let store = open_store(&path);

        for key in &["alpha", "beta", "gamma"] {
            let call = make_call("set", vec![json_str(key), json_str("val")]);
            execute_storage(&call, &store, None).unwrap();
        }

        let call = make_call("list", vec![]);
        let result = execute_storage(&call, &store, None).unwrap();
        let keys = result["keys"].as_array().unwrap();
        assert_eq!(keys.len(), 3);
        assert!(keys.contains(&serde_json::json!("alpha")));
        assert!(keys.contains(&serde_json::json!("beta")));
        assert!(keys.contains(&serde_json::json!("gamma")));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_storage_namespace_isolation() {
        let path = temp_db_path();
        let store = open_store(&path);

        // Set a key in the default namespace
        let call = make_call("set", vec![json_str("shared_key"), json_str("default_val")]);
        execute_storage(&call, &store, None).unwrap();

        // Insert directly into a different namespace to verify isolation
        store
            .storage_set("other_ns", "shared_key", "other_val")
            .unwrap();

        // Reading via execute_storage should only see the "default" namespace
        let call = make_call("get", vec![json_str("shared_key")]);
        let result = execute_storage(&call, &store, None).unwrap();
        assert_eq!(result, serde_json::json!({ "value": "default_val" }));

        // List should only show default namespace keys
        let call = make_call("list", vec![]);
        let result = execute_storage(&call, &store, None).unwrap();
        let keys = result["keys"].as_array().unwrap();
        assert_eq!(keys.len(), 1);

        let _ = std::fs::remove_file(&path);
    }

    fn json_str(s: &str) -> serde_json::Value {
        serde_json::Value::String(s.to_string())
    }

    // SSRF validation tests
    #[test]
    fn test_validate_url_blocks_localhost() {
        assert!(validate_url("http://localhost/foo", &[]).is_err());
        assert!(validate_url("http://127.0.0.1/foo", &[]).is_err());
    }

    #[test]
    fn test_validate_url_blocks_private_ipv4() {
        assert!(validate_url("http://10.0.0.1/api", &[]).is_err());
        assert!(validate_url("http://192.168.1.1/api", &[]).is_err());
        assert!(validate_url("http://172.16.0.1/api", &[]).is_err());
        assert!(validate_url("http://172.31.255.255/api", &[]).is_err());
    }

    #[test]
    fn test_validate_url_blocks_ipv6_loopback() {
        assert!(validate_url("http://[::1]/api", &[]).is_err());
    }

    #[test]
    fn test_validate_url_blocks_ipv6_ula() {
        assert!(validate_url("http://[fc00::1]/api", &[]).is_err());
        assert!(validate_url("http://[fd12:3456::1]/api", &[]).is_err());
    }

    #[test]
    fn test_validate_url_blocks_ipv6_link_local() {
        assert!(validate_url("http://[fe80::1]/api", &[]).is_err());
    }

    #[test]
    fn test_validate_url_blocks_ipv6_mapped_private() {
        assert!(validate_url("http://[::ffff:127.0.0.1]/api", &[]).is_err());
        assert!(validate_url("http://[::ffff:10.0.0.1]/api", &[]).is_err());
        assert!(validate_url("http://[::ffff:192.168.1.1]/api", &[]).is_err());
    }

    #[test]
    fn test_validate_url_allows_public() {
        assert!(validate_url("https://api.example.com/data", &[]).is_ok());
        assert!(validate_url("https://8.8.8.8/dns", &[]).is_ok());
    }

    #[test]
    fn test_validate_url_blocks_non_http_schemes() {
        assert!(validate_url("ftp://example.com/file", &[]).is_err());
        assert!(validate_url("file:///etc/passwd", &[]).is_err());
    }

    #[test]
    fn test_validate_url_blocks_metadata() {
        assert!(validate_url("http://metadata.google.internal/v1/", &[]).is_err());
    }

    #[test]
    fn test_validate_url_respects_allowlist() {
        let allowed = vec!["api.example.com".to_string()];
        assert!(validate_url("https://api.example.com/data", &allowed).is_ok());
        assert!(validate_url("https://evil.com/data", &allowed).is_err());
    }
}
