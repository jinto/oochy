use std::sync::atomic::{AtomicU32, Ordering};

use oochy_core::error::{OochyError, Result};
use oochy_core::types::SkillCall;
use rusqlite::{params, Connection};

static LLM_CALL_COUNT: AtomicU32 = AtomicU32::new(0);
const LLM_MAX_CALLS_PER_EXECUTION: u32 = 3;

/// Execute captured skill calls on the host side (outside sandbox).
/// Each skill call was captured by JS stubs inside QuickJS and is now
/// executed with real API calls after capability checking.
pub async fn execute_skill_calls(
    skill_calls: &[SkillCall],
    config: &oochy_core::config::Config,
) -> Result<Vec<SkillResult>> {
    let allowed_hosts = &config.sandbox.allowed_hosts;
    let db_path = std::env::var("OOCHY_DB_PATH").unwrap_or_else(|_| "oochy.db".into());
    // Reset Llm recursion guard for this top-level execution
    LLM_CALL_COUNT.store(0, Ordering::Relaxed);
    // Sequential execution: skill calls are ordered side-effects from JS.
    // Parallel would break ordering guarantees (message order, read-after-write).
    let mut results = Vec::new();
    for call in skill_calls {
        let result = execute_single_call(call, allowed_hosts, &db_path, config).await;
        results.push(result);
    }

    Ok(results)
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
    db_path: &str,
    config: &oochy_core::config::Config,
) -> SkillResult {
    let result = match call.skill_name.as_str() {
        "Telegram" => execute_telegram(call).await,
        "Http" => execute_http(call, allowed_hosts).await,
        "Storage" => execute_storage(call, db_path),
        "Llm" => execute_llm(call, config).await,
        _ => Err(OochyError::CapabilityDenied(format!(
            "Unknown skill: {}",
            call.skill_name
        ))),
    };

    match result {
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

async fn execute_telegram(call: &SkillCall) -> Result<serde_json::Value> {
    let bot_token = std::env::var("OOCHY_TELEGRAM_TOKEN")
        .map_err(|_| OochyError::Config("OOCHY_TELEGRAM_TOKEN not set".into()))?;

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
                .map_err(|e| OochyError::Skill(format!("Telegram API error: {e}")))?;

            let body: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| OochyError::Skill(format!("Telegram response parse error: {e}")))?;

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
                .map_err(|e| OochyError::Skill(format!("Telegram API error: {e}")))?;

            let body: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| OochyError::Skill(format!("Telegram response parse error: {e}")))?;
            Ok(body)
        }
        _ => Err(OochyError::CapabilityDenied(format!(
            "Unknown Telegram method: {}",
            call.method
        ))),
    }
}

fn validate_url(url_str: &str, allowed_hosts: &[String]) -> Result<()> {
    use std::net::IpAddr;

    let parsed = url::Url::parse(url_str)
        .map_err(|_| OochyError::Sandbox("Http: invalid URL".into()))?;

    // Block non-HTTP(S) schemes
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(OochyError::Sandbox("Http: only http/https schemes allowed".into()));
    }

    let host = parsed.host_str()
        .ok_or_else(|| OochyError::Sandbox("Http: URL has no host".into()))?;

    // Block private/internal IPs (including IPv6-mapped IPv4)
    let addr_str = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = addr_str.parse::<IpAddr>() {
        let blocked = match ip {
            IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local() || v4.is_unspecified(),
            IpAddr::V6(v6) => {
                v6.is_loopback() || v6.is_unspecified() || v6.is_multicast()
                    // ULA (fc00::/7) and link-local (fe80::/10)
                    || (v6.segments()[0] & 0xfe00) == 0xfc00  // fc00::/7
                    || (v6.segments()[0] & 0xffc0) == 0xfe80  // fe80::/10
                    || matches!(v6.to_ipv4_mapped(), Some(v4) if v4.is_loopback() || v4.is_private() || v4.is_link_local())
            }
        };
        if blocked {
            return Err(OochyError::Sandbox(format!("Http: blocked private/internal IP: {host}")));
        }
    }

    // Block known private hostnames
    if matches!(host, "localhost" | "metadata.google.internal") {
        return Err(OochyError::Sandbox(format!("Http: blocked host: {host}")));
    }

    // Check allowlist if configured
    if !allowed_hosts.is_empty() && !allowed_hosts.iter().any(|h| host.ends_with(h.as_str())) {
        return Err(OochyError::Sandbox(format!("Http: host '{host}' not in allowed_hosts")));
    }

    Ok(())
}

async fn execute_http(call: &SkillCall, allowed_hosts: &[String]) -> Result<serde_json::Value> {
    // Disable redirects to prevent redirect-based SSRF bypass
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| OochyError::Sandbox(format!("Http client build error: {e}")))?;
    let url = call.args.first().and_then(|v| v.as_str()).unwrap_or("");

    if url.is_empty() {
        return Err(OochyError::Sandbox("Http: URL is required".into()));
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
            return Err(OochyError::CapabilityDenied(format!(
                "Unknown Http method: {}",
                call.method
            )))
        }
    }
    .map_err(|e| OochyError::Skill(format!("HTTP error: {e}")))?;

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

fn open_storage_db(db_path: &str) -> Result<Connection> {
    let conn = Connection::open(db_path)
        .map_err(|e| OochyError::Store(e.to_string()))?;

    conn.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|e| OochyError::Store(e.to_string()))?;

    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS skill_storage (
            namespace TEXT NOT NULL,
            key TEXT NOT NULL,
            value TEXT NOT NULL,
            PRIMARY KEY (namespace, key)
        );",
    )
    .map_err(|e| OochyError::Store(e.to_string()))?;

    Ok(conn)
}

fn execute_storage(call: &SkillCall, db_path: &str) -> Result<serde_json::Value> {
    let conn = open_storage_db(db_path)?;
    let namespace = "default";

    match call.method.as_str() {
        "get" => {
            let key = call.args.first().and_then(|v| v.as_str()).unwrap_or("");
            let result: rusqlite::Result<String> = conn.query_row(
                "SELECT value FROM skill_storage WHERE namespace = ?1 AND key = ?2",
                params![namespace, key],
                |row| row.get(0),
            );
            match result {
                Ok(value) => Ok(serde_json::json!({ "value": value })),
                Err(rusqlite::Error::QueryReturnedNoRows) => {
                    Ok(serde_json::json!({ "value": null }))
                }
                Err(e) => Err(OochyError::Store(e.to_string())),
            }
        }
        "set" => {
            let key = call.args.first().and_then(|v| v.as_str()).unwrap_or("");
            let value = call.args.get(1).and_then(|v| v.as_str()).unwrap_or("");
            conn.execute(
                "INSERT OR REPLACE INTO skill_storage (namespace, key, value) VALUES (?1, ?2, ?3)",
                params![namespace, key, value],
            )
            .map_err(|e| OochyError::Store(e.to_string()))?;
            Ok(serde_json::json!({ "ok": true }))
        }
        "delete" => {
            let key = call.args.first().and_then(|v| v.as_str()).unwrap_or("");
            conn.execute(
                "DELETE FROM skill_storage WHERE namespace = ?1 AND key = ?2",
                params![namespace, key],
            )
            .map_err(|e| OochyError::Store(e.to_string()))?;
            Ok(serde_json::json!({ "ok": true }))
        }
        "list" => {
            let mut stmt = conn
                .prepare("SELECT key FROM skill_storage WHERE namespace = ?1")
                .map_err(|e| OochyError::Store(e.to_string()))?;
            let keys: Vec<String> = stmt
                .query_map(params![namespace], |row| row.get(0))
                .map_err(|e| OochyError::Store(e.to_string()))?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| OochyError::Store(e.to_string()))?;
            Ok(serde_json::json!({ "keys": keys }))
        }
        _ => Err(OochyError::Skill(format!(
            "Unknown Storage method: {}",
            call.method
        ))),
    }
}

async fn execute_llm(
    call: &SkillCall,
    config: &oochy_core::config::Config,
) -> Result<serde_json::Value> {
    let count = LLM_CALL_COUNT.fetch_add(1, Ordering::Relaxed);
    if count >= LLM_MAX_CALLS_PER_EXECUTION {
        return Err(OochyError::Skill(
            "Llm recursion limit exceeded (max 3 calls per execution)".into(),
        ));
    }

    let prompt = call
        .args
        .first()
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if prompt.is_empty() {
        return Err(OochyError::Skill("Llm.generate: prompt is required".into()));
    }

    let max_tokens = call
        .args
        .get(1)
        .and_then(|v| v.as_u64())
        .unwrap_or(1024) as u32;

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
        .map_err(|e| OochyError::Skill(format!("Llm API error: {e}")))?;

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| OochyError::Skill(format!("Llm response parse error: {e}")))?;

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
            "oochy_skill_test_{}_{}.db",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        p
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
        let db = path.to_str().unwrap();

        let call = make_call("set", vec![json_str("mykey"), json_str("myvalue")]);
        let result = execute_storage(&call, db).unwrap();
        assert_eq!(result, serde_json::json!({ "ok": true }));

        let call = make_call("get", vec![json_str("mykey")]);
        let result = execute_storage(&call, db).unwrap();
        assert_eq!(result, serde_json::json!({ "value": "myvalue" }));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_storage_get_nonexistent_key() {
        let path = temp_db_path();
        let db = path.to_str().unwrap();

        let call = make_call("get", vec![json_str("nokey")]);
        let result = execute_storage(&call, db).unwrap();
        assert_eq!(result, serde_json::json!({ "value": null }));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_storage_set_overwrites() {
        let path = temp_db_path();
        let db = path.to_str().unwrap();

        let call = make_call("set", vec![json_str("k"), json_str("v1")]);
        execute_storage(&call, db).unwrap();

        let call = make_call("set", vec![json_str("k"), json_str("v2")]);
        execute_storage(&call, db).unwrap();

        let call = make_call("get", vec![json_str("k")]);
        let result = execute_storage(&call, db).unwrap();
        assert_eq!(result, serde_json::json!({ "value": "v2" }));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_storage_delete() {
        let path = temp_db_path();
        let db = path.to_str().unwrap();

        let call = make_call("set", vec![json_str("k"), json_str("v")]);
        execute_storage(&call, db).unwrap();

        let call = make_call("delete", vec![json_str("k")]);
        let result = execute_storage(&call, db).unwrap();
        assert_eq!(result, serde_json::json!({ "ok": true }));

        let call = make_call("get", vec![json_str("k")]);
        let result = execute_storage(&call, db).unwrap();
        assert_eq!(result, serde_json::json!({ "value": null }));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_storage_delete_nonexistent_key() {
        let path = temp_db_path();
        let db = path.to_str().unwrap();

        let call = make_call("delete", vec![json_str("nokey")]);
        let result = execute_storage(&call, db).unwrap();
        assert_eq!(result, serde_json::json!({ "ok": true }));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_storage_list_empty() {
        let path = temp_db_path();
        let db = path.to_str().unwrap();

        let call = make_call("list", vec![]);
        let result = execute_storage(&call, db).unwrap();
        let empty: Vec<String> = vec![];
        assert_eq!(result, serde_json::json!({ "keys": empty }));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_storage_list_with_keys() {
        let path = temp_db_path();
        let db = path.to_str().unwrap();

        for key in &["alpha", "beta", "gamma"] {
            let call = make_call("set", vec![json_str(key), json_str("val")]);
            execute_storage(&call, db).unwrap();
        }

        let call = make_call("list", vec![]);
        let result = execute_storage(&call, db).unwrap();
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
        let db = path.to_str().unwrap();

        // Set a key via the default namespace (through execute_storage)
        let call = make_call("set", vec![json_str("shared_key"), json_str("default_val")]);
        execute_storage(&call, db).unwrap();

        // Directly insert into a different namespace to verify isolation
        let conn = open_storage_db(db).unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO skill_storage (namespace, key, value) VALUES (?1, ?2, ?3)",
            params!["other_ns", "shared_key", "other_val"],
        )
        .unwrap();

        // Reading via execute_storage should only see the "default" namespace
        let call = make_call("get", vec![json_str("shared_key")]);
        let result = execute_storage(&call, db).unwrap();
        assert_eq!(result, serde_json::json!({ "value": "default_val" }));

        // List should only show default namespace keys
        let call = make_call("list", vec![]);
        let result = execute_storage(&call, db).unwrap();
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
