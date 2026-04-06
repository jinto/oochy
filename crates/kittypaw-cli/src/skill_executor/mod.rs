mod agent;
mod discord;
mod env;
mod file;
mod git;
mod http;
mod llm;
mod process;
mod shell;
mod slack;
mod storage;
mod telegram;

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;

use kittypaw_core::capability::CapabilityChecker;
use kittypaw_core::error::{KittypawError, Result};
use kittypaw_core::permission::{PermissionDecision, PermissionRequest, ResourceKind};
use kittypaw_core::types::SkillCall;
use kittypaw_store::Store;

/// Callback that sends a [`PermissionRequest`] to the UI and returns a one-shot
/// channel carrying the user's decision. When absent (`None`), file operations
/// are auto-allowed (backward-compatible with callers that have no UI).
pub type PermissionCallback = Arc<
    dyn Fn(PermissionRequest) -> tokio::sync::oneshot::Receiver<PermissionDecision> + Send + Sync,
>;

const LLM_MAX_CALLS_PER_EXECUTION: u32 = 3;
const MAX_SKILL_RESULT_BYTES: usize = 50 * 1024;

/// Check capability and log denial. Returns Err(message) if denied.
fn check_capability(
    checker: &mut CapabilityChecker,
    call: &SkillCall,
) -> std::result::Result<(), String> {
    if let Err(e) = checker.check(call) {
        let msg = e.to_string();
        tracing::warn!(
            "Capability denied for {}.{}: {}",
            call.skill_name,
            call.method,
            msg
        );
        Err(msg)
    } else {
        Ok(())
    }
}

/// Check file-level permission before a `File.read` or `File.write` operation.
///
/// When `on_permission` is `None`, the call is auto-allowed (legacy/headless mode).
/// Otherwise, a [`PermissionRequest`] is sent through the callback and the caller
/// blocks until the UI responds with a [`PermissionDecision`].
///
/// Returns `Ok(())` when the operation may proceed, or an `Err` with a
/// user-facing message when the request was denied or the channel dropped.
async fn check_file_permission(
    call: &SkillCall,
    on_permission: Option<&PermissionCallback>,
) -> std::result::Result<(), String> {
    let cb = match on_permission {
        Some(cb) => cb,
        None => return Ok(()), // auto-allow
    };

    let action = match call.method.as_str() {
        "read" => "read",
        "write" => "write",
        _ => return Ok(()), // unknown methods are rejected later by execute_file
    };

    let path = call
        .args
        .first()
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let request = PermissionRequest {
        request_id: uuid_v4(),
        resource_kind: ResourceKind::File,
        resource_path: path,
        action: action.to_string(),
        workspace_id: String::new(), // filled by caller if workspace-scoped
    };

    let rx = cb(request);
    match rx.await {
        Ok(PermissionDecision::AllowOnce | PermissionDecision::AllowPermanent) => Ok(()),
        Ok(PermissionDecision::Deny) => Err(format!("Permission denied: File.{action}")),
        Err(_) => Err("Permission check failed: response channel dropped".to_string()),
    }
}

/// Generate a simple v4-style UUID without pulling in the `uuid` crate.
fn uuid_v4() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    // Mix with thread-local counter for uniqueness within the same nanosecond.
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{nanos:032x}-{seq:08x}")
}

/// Execute a single skill call inline (for use as a SkillResolver callback).
/// Returns a string result that flows back to JS during sandbox execution.
/// When `checker` is provided, the call is verified against the capability allowlist
/// before execution. If `None`, all calls are permitted (permissive/legacy mode).
pub async fn resolve_skill_call(
    call: &SkillCall,
    config: &kittypaw_core::config::Config,
    store: &Arc<Mutex<Store>>,
    checker: Option<&Arc<std::sync::Mutex<CapabilityChecker>>>,
    on_permission: Option<&PermissionCallback>,
) -> String {
    resolve_skill_call_with_mcp(call, config, store, checker, on_permission, None).await
}

/// Extended version that also supports MCP tool calls.
pub async fn resolve_skill_call_with_mcp(
    call: &SkillCall,
    config: &kittypaw_core::config::Config,
    store: &Arc<Mutex<Store>>,
    checker: Option<&Arc<std::sync::Mutex<CapabilityChecker>>>,
    on_permission: Option<&PermissionCallback>,
    mcp_registry: Option<&Arc<tokio::sync::Mutex<crate::mcp_registry::McpRegistry>>>,
) -> String {
    let result =
        resolve_skill_call_inner(call, config, store, checker, on_permission, mcp_registry).await;
    if result.len() > MAX_SKILL_RESULT_BYTES {
        return serde_json::to_string(&serde_json::json!({
            "error": format!("Result too large ({} bytes, limit {})", result.len(), MAX_SKILL_RESULT_BYTES),
            "truncated": true
        }))
        .unwrap_or_else(|_| "null".to_string());
    }
    result
}

async fn resolve_skill_call_inner(
    call: &SkillCall,
    config: &kittypaw_core::config::Config,
    store: &Arc<Mutex<Store>>,
    checker: Option<&Arc<std::sync::Mutex<CapabilityChecker>>>,
    on_permission: Option<&PermissionCallback>,
    mcp_registry: Option<&Arc<tokio::sync::Mutex<crate::mcp_registry::McpRegistry>>>,
) -> String {
    if let Some(cap) = checker {
        match cap.lock() {
            Ok(mut guard) => {
                if let Err(msg) = check_capability(&mut guard, call) {
                    return serde_json::to_string(&serde_json::json!({"error": msg}))
                        .unwrap_or_else(|_| "null".to_string());
                }
            }
            Err(_) => {
                return serde_json::to_string(
                    &serde_json::json!({"error": "capability checker lock poisoned"}),
                )
                .unwrap_or_else(|_| "null".to_string());
            }
        }
    }

    // File calls are synchronous — but require an async permission check first
    if call.skill_name == "File" {
        if let Err(msg) = check_file_permission(call, on_permission).await {
            return serde_json::to_string(&serde_json::json!({"error": msg}))
                .unwrap_or_else(|_| "null".to_string());
        }
        return match file::execute_file(call, None) {
            Ok(val) => serde_json::to_string(&val).unwrap_or_else(|_| "null".to_string()),
            Err(e) => serde_json::to_string(&serde_json::json!({"error": e.to_string()}))
                .unwrap_or_else(|_| "null".to_string()),
        };
    }

    // Env calls are synchronous
    if call.skill_name == "Env" {
        return match env::execute_env(call, None) {
            Ok(val) => serde_json::to_string(&val).unwrap_or_else(|_| "null".to_string()),
            Err(e) => serde_json::to_string(&serde_json::json!({"error": e.to_string()}))
                .unwrap_or_else(|_| "null".to_string()),
        };
    }

    // Web calls are async (HTTP), go through the async path
    if call.skill_name == "Web" {
        let result = http::execute_web(call, &config.sandbox.allowed_hosts).await;
        return match result {
            Ok(val) => serde_json::to_string(&val).unwrap_or_else(|_| "null".to_string()),
            Err(e) => serde_json::to_string(&serde_json::json!({"error": e.to_string()}))
                .unwrap_or_else(|_| "null".to_string()),
        };
    }

    // MCP calls delegate to the MCP registry
    if call.skill_name == "Mcp" {
        if let Some(registry) = mcp_registry {
            let mut reg = registry.lock().await;
            let result = match call.method.as_str() {
                "call" => {
                    let server = call.args.first().and_then(|v| v.as_str()).unwrap_or("");
                    let tool = call.args.get(1).and_then(|v| v.as_str()).unwrap_or("");
                    let args = call.args.get(2).cloned().unwrap_or(serde_json::Value::Null);
                    reg.call_tool(server, tool, args).await
                }
                "listTools" => {
                    let server = call.args.first().and_then(|v| v.as_str()).unwrap_or("");
                    reg.list_tools(server).await
                }
                _ => Err(KittypawError::Skill(format!(
                    "Unknown Mcp method: {}",
                    call.method
                ))),
            };
            return match result {
                Ok(val) => serde_json::to_string(&val).unwrap_or_else(|_| "null".to_string()),
                Err(e) => serde_json::to_string(&serde_json::json!({"error": e.to_string()}))
                    .unwrap_or_else(|_| "null".to_string()),
            };
        } else {
            return serde_json::to_string(&serde_json::json!({"error": "MCP not configured"}))
                .unwrap_or_else(|_| "null".to_string());
        }
    }

    // Storage calls need Store access
    if call.skill_name == "Storage" {
        let s = store.lock().await;
        return match storage::execute_storage(call, &s, None) {
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
        None,
        on_permission,
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
/// Returns a parallel Vec of Option<SkillResult> -- Some for Storage calls, None for others.
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
                    storage::execute_storage(call, store, skill_context),
                ))
            } else {
                None
            }
        })
        .collect()
}

fn is_read_only_skill_call(call: &SkillCall) -> bool {
    matches!(
        (call.skill_name.as_str(), call.method.as_str()),
        ("Http", "get")
            | ("Web", "search")
            | ("Web", "fetch")
            | ("Env", "get")
            | ("File", "read")
            | ("Storage", "get")
            | ("Storage", "list")
            | ("Git", "status")
            | ("Git", "diff")
            | ("Git", "log")
    )
}

/// Execute captured skill calls on the host side (outside sandbox).
/// Each skill call was captured by JS stubs inside QuickJS and is now
/// executed with real API calls after capability checking.
/// Storage calls must be pre-resolved via `resolve_storage_calls` and passed as `preresolved`.
/// When `checker` is provided, each call is verified against the capability allowlist.
/// If `None`, all calls are permitted (permissive/legacy mode).
/// `model_override` selects a named model from `config.models` for LLM calls instead of the default.
pub async fn execute_skill_calls(
    skill_calls: &[SkillCall],
    config: &kittypaw_core::config::Config,
    preresolved: Vec<Option<SkillResult>>,
    skill_context: Option<&str>,
    mut checker: Option<&mut CapabilityChecker>,
    model_override: Option<&str>,
) -> Result<Vec<SkillResult>> {
    let allowed_hosts = &config.sandbox.allowed_hosts;
    // Per-execution LLM call counter (not global, avoids race between concurrent executions)
    let llm_call_count = AtomicU32::new(0);
    let (safe_calls, unsafe_calls): (Vec<_>, Vec<_>) = skill_calls
        .iter()
        .partition(|call| is_read_only_skill_call(call));
    tracing::debug!(
        total_calls = skill_calls.len(),
        safe_calls = safe_calls.len(),
        unsafe_calls = unsafe_calls.len(),
        "classified skill calls for safe/unsafe execution"
    );
    // Sequential execution: skill calls are ordered side-effects from JS.
    // Parallel would break ordering guarantees (message order, read-after-write).
    let mut results = Vec::new();
    for (call, precomp) in skill_calls.iter().zip(preresolved.into_iter()) {
        let result = if let Some(r) = precomp {
            r
        } else {
            if let Some(ref mut cap) = checker {
                if let Err(msg) = check_capability(cap, call) {
                    results.push(SkillResult {
                        skill_name: call.skill_name.clone(),
                        method: call.method.clone(),
                        success: false,
                        result: serde_json::Value::Null,
                        error: Some(msg),
                    });
                    continue;
                }
            }
            execute_single_call(
                call,
                allowed_hosts,
                config,
                skill_context,
                &llm_call_count,
                model_override,
                None, // on_permission: auto-allow (batch path)
            )
            .await
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
    skill_context: Option<&str>,
    llm_call_count: &AtomicU32,
    model_override: Option<&str>,
    on_permission: Option<&PermissionCallback>,
) -> SkillResult {
    tracing::debug!(
        skill = %call.skill_name,
        method = %call.method,
        context = ?skill_context,
        "executing skill call"
    );

    // Autonomy level gate: check once, branch by level
    if !is_read_only_skill_call(call) {
        match config.autonomy_level {
            kittypaw_core::config::AutonomyLevel::Readonly => {
                return SkillResult {
                    skill_name: call.skill_name.clone(),
                    method: call.method.clone(),
                    success: false,
                    result: serde_json::Value::Null,
                    error: Some(format!(
                        "Blocked by ReadOnly mode: {}.{}",
                        call.skill_name, call.method
                    )),
                };
            }
            kittypaw_core::config::AutonomyLevel::Supervised => {
                if let Some(cb) = on_permission {
                    let request = kittypaw_core::permission::PermissionRequest {
                        request_id: uuid_v4(),
                        resource_kind: kittypaw_core::permission::ResourceKind::File,
                        resource_path: format!("{}.{}", call.skill_name, call.method),
                        action: "execute".to_string(),
                        workspace_id: String::new(),
                    };
                    let rx = cb(request);
                    match rx.await {
                        Ok(
                            kittypaw_core::permission::PermissionDecision::AllowOnce
                            | kittypaw_core::permission::PermissionDecision::AllowPermanent,
                        ) => {}
                        _ => {
                            return SkillResult {
                                skill_name: call.skill_name.clone(),
                                method: call.method.clone(),
                                success: false,
                                result: serde_json::Value::Null,
                                error: Some(format!(
                                    "Denied by Supervised mode: {}.{}",
                                    call.skill_name, call.method
                                )),
                            };
                        }
                    }
                }
            }
            kittypaw_core::config::AutonomyLevel::Full => {}
        }
    }

    let result = match call.skill_name.as_str() {
        "Telegram" => telegram::execute_telegram(call, config).await,
        "Slack" => slack::execute_slack(call, config).await,
        "Discord" => discord::execute_discord(call, config).await,
        "Http" => http::execute_http(call, allowed_hosts).await,
        "Web" => http::execute_web(call, allowed_hosts).await,
        "Llm" => llm::execute_llm(call, config, llm_call_count, model_override).await,
        "File" => match check_file_permission(call, on_permission).await {
            Ok(()) => file::execute_file(call, None),
            Err(msg) => Err(KittypawError::CapabilityDenied(msg)),
        },
        "Env" => env::execute_env(call, None),
        "Shell" => shell::execute_shell(call).await,
        "Git" => git::execute_git(call).await,
        "Agent" => agent::execute_agent(call, config).await,
        _ => Err(KittypawError::CapabilityDenied(format!(
            "Unknown skill: {}",
            call.skill_name
        ))),
    };

    // Retry once for Http/Web network errors (KittypawError::Skill = reqwest send failure,
    // NOT validation errors like Sandbox/CapabilityDenied which are permanent).
    let result = match result {
        Err(ref e)
            if matches!(call.skill_name.as_str(), "Http" | "Web")
                && matches!(e, KittypawError::Skill(_)) =>
        {
            tracing::info!(
                skill = %call.skill_name,
                method = %call.method,
                error = %e,
                "network error on Http/Web skill, retrying once after 1s"
            );
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            match call.skill_name.as_str() {
                "Http" => http::execute_http(call, allowed_hosts).await,
                "Web" => http::execute_web(call, allowed_hosts).await,
                _ => unreachable!(),
            }
        }
        other => other,
    };

    make_skill_result(call, result)
}

/// Resolve a channel token using the priority chain:
/// 1. secrets store
/// 2. environment variable
/// 3. config.channels[*] where channel_type matches
fn resolve_channel_token(
    config: &kittypaw_core::config::Config,
    channel_type: &str,
    secret_key: &str,
    env_var: &str,
) -> Option<String> {
    let result = // 1. secrets: channels/{secret_key} (Settings UI)
    kittypaw_core::secrets::get_secret("channels", secret_key)
        .ok()
        .flatten()
        .filter(|s| !s.is_empty())
        // 2. secrets: {channel_type}/bot_token (onboarding)
        .or_else(|| {
            kittypaw_core::secrets::get_secret(channel_type, "bot_token")
                .ok()
                .flatten()
                .filter(|s| !s.is_empty())
        })
        // 3. environment variable
        .or_else(|| std::env::var(env_var).ok().filter(|s| !s.is_empty()))
        // 4. config.channels[*]
        .or_else(|| {
            config
                .channels
                .iter()
                .find(|c| c.channel_type == channel_type)
                .map(|c| c.token.clone())
                .filter(|s| !s.is_empty())
        });
    if result.is_none() {
        tracing::warn!(
            channel = channel_type,
            "Channel token not found in secrets, env, or config"
        );
    } else {
        tracing::debug!(channel = channel_type, "Channel token resolved");
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
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
    fn test_is_read_only_http_get() {
        let call = SkillCall {
            skill_name: "Http".to_string(),
            method: "get".to_string(),
            args: vec![],
        };

        assert!(is_read_only_skill_call(&call));
    }

    #[test]
    fn test_is_not_read_only_http_post() {
        let call = SkillCall {
            skill_name: "Http".to_string(),
            method: "post".to_string(),
            args: vec![],
        };

        assert!(!is_read_only_skill_call(&call));
    }

    #[test]
    fn test_is_not_read_only_telegram() {
        let call = SkillCall {
            skill_name: "Telegram".to_string(),
            method: "sendMessage".to_string(),
            args: vec![],
        };

        assert!(!is_read_only_skill_call(&call));
    }

    #[test]
    fn test_is_read_only_storage_get() {
        let call = SkillCall {
            skill_name: "Storage".to_string(),
            method: "get".to_string(),
            args: vec![],
        };

        assert!(is_read_only_skill_call(&call));
    }

    #[test]
    fn test_is_not_read_only_storage_set() {
        let call = SkillCall {
            skill_name: "Storage".to_string(),
            method: "set".to_string(),
            args: vec![],
        };

        assert!(!is_read_only_skill_call(&call));
    }

    fn make_telegram_call(method: &str, args: Vec<serde_json::Value>) -> SkillCall {
        SkillCall {
            skill_name: "Telegram".to_string(),
            method: method.to_string(),
            args,
        }
    }

    fn telegram_config() -> kittypaw_core::config::Config {
        let mut config = kittypaw_core::config::Config::default();
        config.channels.push(kittypaw_core::config::ChannelConfig {
            channel_type: kittypaw_core::config::ChannelType::Telegram,
            token: "dummy-token".to_string(),
            bind_addr: None,
        });
        config
    }

    #[tokio::test]
    async fn test_telegram_send_message_empty_chat_id_uses_default() {
        // Empty chat_id → tries default from secrets (may fail if no secrets set)
        let call = make_telegram_call("sendMessage", vec![json_str(""), json_str("hello")]);
        let config = telegram_config();
        let result = telegram::execute_telegram(&call, &config).await;
        // Either succeeds with default chat_id or fails with "chat_id가 설정되지 않았습니다"
        if let Err(e) = &result {
            assert!(
                e.to_string().contains("chat_id") || e.to_string().contains("not configured"),
                "unexpected error: {e}"
            );
        }
    }

    #[tokio::test]
    async fn test_telegram_send_message_empty_text() {
        let call = make_telegram_call("sendMessage", vec![json_str("12345"), json_str("")]);
        let config = telegram_config();

        let result = telegram::execute_telegram(&call, &config).await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("missing text"), "error was: {err}");
    }

    #[test]
    fn test_storage_set_and_get() {
        let path = temp_db_path();
        let store = open_store(&path);

        let call = make_call("set", vec![json_str("mykey"), json_str("myvalue")]);
        let result = storage::execute_storage(&call, &store, None).unwrap();
        assert_eq!(result, serde_json::json!({ "ok": true }));

        let call = make_call("get", vec![json_str("mykey")]);
        let result = storage::execute_storage(&call, &store, None).unwrap();
        assert_eq!(result, serde_json::json!({ "value": "myvalue" }));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_storage_get_nonexistent_key() {
        let path = temp_db_path();
        let store = open_store(&path);

        let call = make_call("get", vec![json_str("nokey")]);
        let result = storage::execute_storage(&call, &store, None).unwrap();
        assert_eq!(result, serde_json::json!({ "value": null }));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_storage_set_overwrites() {
        let path = temp_db_path();
        let store = open_store(&path);

        let call = make_call("set", vec![json_str("k"), json_str("v1")]);
        storage::execute_storage(&call, &store, None).unwrap();

        let call = make_call("set", vec![json_str("k"), json_str("v2")]);
        storage::execute_storage(&call, &store, None).unwrap();

        let call = make_call("get", vec![json_str("k")]);
        let result = storage::execute_storage(&call, &store, None).unwrap();
        assert_eq!(result, serde_json::json!({ "value": "v2" }));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_storage_delete() {
        let path = temp_db_path();
        let store = open_store(&path);

        let call = make_call("set", vec![json_str("k"), json_str("v")]);
        storage::execute_storage(&call, &store, None).unwrap();

        let call = make_call("delete", vec![json_str("k")]);
        let result = storage::execute_storage(&call, &store, None).unwrap();
        assert_eq!(result, serde_json::json!({ "ok": true }));

        let call = make_call("get", vec![json_str("k")]);
        let result = storage::execute_storage(&call, &store, None).unwrap();
        assert_eq!(result, serde_json::json!({ "value": null }));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_storage_delete_nonexistent_key() {
        let path = temp_db_path();
        let store = open_store(&path);

        let call = make_call("delete", vec![json_str("nokey")]);
        let result = storage::execute_storage(&call, &store, None).unwrap();
        assert_eq!(result, serde_json::json!({ "ok": true }));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_storage_set_empty_key() {
        let path = temp_db_path();
        let store = open_store(&path);

        let call = make_call("set", vec![json_str(""), json_str("value")]);
        let result = storage::execute_storage(&call, &store, None);

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("key is required"), "error was: {err}");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_storage_delete_empty_key() {
        let path = temp_db_path();
        let store = open_store(&path);

        let call = make_call("delete", vec![json_str("")]);
        let result = storage::execute_storage(&call, &store, None);

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("key is required"), "error was: {err}");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_storage_list_empty() {
        let path = temp_db_path();
        let store = open_store(&path);

        let call = make_call("list", vec![]);
        let result = storage::execute_storage(&call, &store, None).unwrap();
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
            storage::execute_storage(&call, &store, None).unwrap();
        }

        let call = make_call("list", vec![]);
        let result = storage::execute_storage(&call, &store, None).unwrap();
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
        storage::execute_storage(&call, &store, None).unwrap();

        // Insert directly into a different namespace to verify isolation
        store
            .storage_set("other_ns", "shared_key", "other_val")
            .unwrap();

        // Reading via execute_storage should only see the "default" namespace
        let call = make_call("get", vec![json_str("shared_key")]);
        let result = storage::execute_storage(&call, &store, None).unwrap();
        assert_eq!(result, serde_json::json!({ "value": "default_val" }));

        // List should only show default namespace keys
        let call = make_call("list", vec![]);
        let result = storage::execute_storage(&call, &store, None).unwrap();
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
        assert!(http::validate_url("http://localhost/foo", &[]).is_err());
        assert!(http::validate_url("http://127.0.0.1/foo", &[]).is_err());
    }

    #[test]
    fn test_validate_url_blocks_private_ipv4() {
        assert!(http::validate_url("http://10.0.0.1/api", &[]).is_err());
        assert!(http::validate_url("http://192.168.1.1/api", &[]).is_err());
        assert!(http::validate_url("http://172.16.0.1/api", &[]).is_err());
        assert!(http::validate_url("http://172.31.255.255/api", &[]).is_err());
    }

    #[test]
    fn test_validate_url_blocks_ipv6_loopback() {
        assert!(http::validate_url("http://[::1]/api", &[]).is_err());
    }

    #[test]
    fn test_validate_url_blocks_ipv6_ula() {
        assert!(http::validate_url("http://[fc00::1]/api", &[]).is_err());
        assert!(http::validate_url("http://[fd12:3456::1]/api", &[]).is_err());
    }

    #[test]
    fn test_validate_url_blocks_ipv6_link_local() {
        assert!(http::validate_url("http://[fe80::1]/api", &[]).is_err());
    }

    #[test]
    fn test_validate_url_blocks_ipv6_mapped_private() {
        assert!(http::validate_url("http://[::ffff:127.0.0.1]/api", &[]).is_err());
        assert!(http::validate_url("http://[::ffff:10.0.0.1]/api", &[]).is_err());
        assert!(http::validate_url("http://[::ffff:192.168.1.1]/api", &[]).is_err());
    }

    #[test]
    fn test_validate_url_allows_public() {
        assert!(http::validate_url("https://api.example.com/data", &[]).is_ok());
        assert!(http::validate_url("https://8.8.8.8/dns", &[]).is_ok());
    }

    #[test]
    fn test_validate_url_blocks_non_http_schemes() {
        assert!(http::validate_url("ftp://example.com/file", &[]).is_err());
        assert!(http::validate_url("file:///etc/passwd", &[]).is_err());
    }

    #[test]
    fn test_validate_url_blocks_metadata() {
        assert!(http::validate_url("http://metadata.google.internal/v1/", &[]).is_err());
    }

    #[test]
    fn test_validate_url_respects_allowlist() {
        let allowed = vec!["api.example.com".to_string()];
        assert!(http::validate_url("https://api.example.com/data", &allowed).is_ok());
        assert!(http::validate_url("https://evil.com/data", &allowed).is_err());
    }

    // File skill tests

    fn make_file_call(method: &str, args: Vec<serde_json::Value>) -> SkillCall {
        SkillCall {
            skill_name: "File".to_string(),
            method: method.to_string(),
            args,
        }
    }

    #[test]
    fn test_file_write_and_read() {
        let dir = tempfile::tempdir().unwrap();
        let call = make_file_call("write", vec![json_str("test.txt"), json_str("hello world")]);
        let result = file::execute_file(&call, Some(dir.path())).unwrap();
        assert_eq!(result, serde_json::json!({ "ok": true }));

        let call = make_file_call("read", vec![json_str("test.txt")]);
        let result = file::execute_file(&call, Some(dir.path())).unwrap();
        assert_eq!(result, serde_json::json!({ "content": "hello world" }));
    }

    #[test]
    fn test_file_path_traversal_blocked() {
        let dir = tempfile::tempdir().unwrap();
        let call = make_file_call("read", vec![json_str("../../../etc/passwd")]);
        let result = file::execute_file(&call, Some(dir.path()));
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("path traversal"), "error was: {err}");
    }

    #[test]
    fn test_file_size_limit() {
        let dir = tempfile::tempdir().unwrap();
        let big_content = "x".repeat(11 * 1024 * 1024); // 11MB
        let call = make_file_call("write", vec![json_str("big.txt"), json_str(&big_content)]);
        let result = file::execute_file(&call, Some(dir.path()));
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("10MB"), "error was: {err}");
    }

    // Env skill tests

    fn make_env_call(method: &str, args: Vec<serde_json::Value>) -> SkillCall {
        SkillCall {
            skill_name: "Env".to_string(),
            method: method.to_string(),
            args,
        }
    }

    #[test]
    fn test_env_get_from_config() {
        let mut config = HashMap::new();
        config.insert("MY_KEY".to_string(), "my_value".to_string());
        let call = make_env_call("get", vec![json_str("MY_KEY")]);
        let result = env::execute_env(&call, Some(&config)).unwrap();
        assert_eq!(result, serde_json::json!({ "value": "my_value" }));
    }

    #[test]
    fn test_env_get_missing_key() {
        let config = HashMap::new();
        let call = make_env_call("get", vec![json_str("MISSING")]);
        let result = env::execute_env(&call, Some(&config)).unwrap();
        assert_eq!(result, serde_json::json!({ "value": null }));
    }

    // Web skill tests

    fn make_web_call(method: &str, args: Vec<serde_json::Value>) -> SkillCall {
        SkillCall {
            skill_name: "Web".to_string(),
            method: method.to_string(),
            args,
        }
    }

    #[tokio::test]
    async fn test_web_search_requires_query() {
        let call = make_web_call("search", vec![]);
        let result = http::execute_web(&call, &[]).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("query is required"), "error was: {err}");
    }

    #[tokio::test]
    async fn test_web_fetch_requires_url() {
        let call = make_web_call("fetch", vec![]);
        let result = http::execute_web(&call, &[]).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("url is required"), "error was: {err}");
    }

    #[test]
    fn test_strip_html_tags() {
        let html = "<html><body><h1>Hello</h1><p>World</p></body></html>";
        let text = http::strip_html_tags(html);
        assert_eq!(text, "Hello World");
    }

    #[test]
    fn test_strip_html_removes_scripts() {
        let html =
            "<p>Before</p><script>alert('xss')</script><style>.x{color:red}</style><p>After</p>";
        let text = http::strip_html_tags(html);
        assert_eq!(text, "Before After");
    }

    #[tokio::test]
    async fn test_resolve_skill_call_file_denied_by_permission() {
        let path = temp_db_path();
        let store = Arc::new(tokio::sync::Mutex::new(open_store(&path)));
        let config = kittypaw_core::config::Config::default();

        let call = SkillCall {
            skill_name: "File".to_string(),
            method: "write".to_string(),
            args: vec![
                serde_json::Value::String("test.txt".into()),
                serde_json::Value::String("content".into()),
            ],
        };

        let deny_cb: PermissionCallback = Arc::new(|_req| {
            let (tx, rx) = tokio::sync::oneshot::channel();
            let _ = tx.send(PermissionDecision::Deny);
            rx
        });

        let result = resolve_skill_call(&call, &config, &store, None, Some(&deny_cb)).await;
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert!(
            parsed.get("error").is_some(),
            "File.write should be denied when permission callback denies"
        );

        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn test_result_size_limit_over_threshold_returns_error_json_from_resolve_skill_call() {
        let path = temp_db_path();
        let store = Arc::new(tokio::sync::Mutex::new(open_store(&path)));
        let config = kittypaw_core::config::Config::default();

        let large = "x".repeat(MAX_SKILL_RESULT_BYTES + 1);
        {
            let s = store.lock().await;
            let set_call = make_call("set", vec![json_str("big"), json_str(&large)]);
            storage::execute_storage(&set_call, &s, None).unwrap();
        }

        let get_call = make_call("get", vec![json_str("big")]);
        let result = resolve_skill_call(&get_call, &config, &store, None, None).await;
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert!(parsed.get("error").is_some());
        assert_eq!(parsed.get("truncated").unwrap(), true);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_result_size_limit_under_threshold() {
        let small = "hello".to_string();
        assert!(small.len() <= MAX_SKILL_RESULT_BYTES);
    }

    #[test]
    fn test_result_size_limit_over_threshold_returns_valid_json() {
        let large = "x".repeat(MAX_SKILL_RESULT_BYTES + 1);
        let result = if large.len() > MAX_SKILL_RESULT_BYTES {
            serde_json::to_string(&serde_json::json!({
                "error": format!("Result too large ({} bytes, limit {})", large.len(), MAX_SKILL_RESULT_BYTES),
                "truncated": true
            })).unwrap()
        } else {
            large
        };
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert!(parsed.get("error").is_some());
        assert_eq!(parsed.get("truncated").unwrap(), true);
    }

    #[tokio::test]
    async fn test_http_invalid_url_not_retried() {
        // Sandbox errors (URL validation) should NOT be retried
        let call = SkillCall {
            skill_name: "Http".to_string(),
            method: "get".to_string(),
            args: vec![serde_json::Value::String("not-a-url".into())],
        };
        let config = kittypaw_core::config::Config::default();
        let counter = AtomicU32::new(0);
        let result = execute_single_call(&call, &[], &config, None, &counter, None, None).await;
        assert!(!result.success);
        assert!(
            result.error.as_ref().unwrap().contains("invalid URL"),
            "Expected URL validation error, got: {:?}",
            result.error
        );
    }

    // ── Shell tests ──

    #[tokio::test]
    async fn test_shell_exec_echo() {
        let call = SkillCall {
            skill_name: "Shell".to_string(),
            method: "exec".to_string(),
            args: vec![json_str("echo hello")],
        };
        let result = shell::execute_shell(&call).await.unwrap();
        assert_eq!(result["stdout"].as_str().unwrap().trim(), "hello");
        assert_eq!(result["exit_code"], 0);
    }

    #[tokio::test]
    async fn test_shell_exec_empty_command() {
        let call = SkillCall {
            skill_name: "Shell".to_string(),
            method: "exec".to_string(),
            args: vec![json_str("")],
        };
        let result = shell::execute_shell(&call).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("command is required"));
    }

    #[tokio::test]
    async fn test_shell_unknown_method() {
        let call = SkillCall {
            skill_name: "Shell".to_string(),
            method: "run".to_string(),
            args: vec![],
        };
        let result = shell::execute_shell(&call).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Unknown Shell method"));
    }

    // ── Git tests ──

    #[tokio::test]
    async fn test_git_status() {
        let call = SkillCall {
            skill_name: "Git".to_string(),
            method: "status".to_string(),
            args: vec![],
        };
        // Should succeed in any git repo (we're in one)
        let result = git::execute_git(&call).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap()["exit_code"], 0);
    }

    #[tokio::test]
    async fn test_git_log() {
        let call = SkillCall {
            skill_name: "Git".to_string(),
            method: "log".to_string(),
            args: vec![serde_json::json!(3)],
        };
        let result = git::execute_git(&call).await.unwrap();
        assert_eq!(result["exit_code"], 0);
        let stdout = result["stdout"].as_str().unwrap();
        assert!(!stdout.is_empty());
    }

    #[tokio::test]
    async fn test_git_commit_empty_message() {
        let call = SkillCall {
            skill_name: "Git".to_string(),
            method: "commit".to_string(),
            args: vec![json_str("")],
        };
        let result = git::execute_git(&call).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("message is required"));
    }

    #[tokio::test]
    async fn test_git_unknown_method() {
        let call = SkillCall {
            skill_name: "Git".to_string(),
            method: "push".to_string(),
            args: vec![],
        };
        let result = git::execute_git(&call).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Unknown Git method"));
    }

    // ── is_read_only: Shell & Git ──

    #[test]
    fn test_is_read_only_git_status() {
        let call = SkillCall {
            skill_name: "Git".to_string(),
            method: "status".to_string(),
            args: vec![],
        };
        assert!(is_read_only_skill_call(&call));
    }

    #[test]
    fn test_is_read_only_git_diff() {
        let call = SkillCall {
            skill_name: "Git".to_string(),
            method: "diff".to_string(),
            args: vec![],
        };
        assert!(is_read_only_skill_call(&call));
    }

    #[test]
    fn test_is_read_only_git_log() {
        let call = SkillCall {
            skill_name: "Git".to_string(),
            method: "log".to_string(),
            args: vec![],
        };
        assert!(is_read_only_skill_call(&call));
    }

    #[test]
    fn test_is_not_read_only_git_commit() {
        let call = SkillCall {
            skill_name: "Git".to_string(),
            method: "commit".to_string(),
            args: vec![],
        };
        assert!(!is_read_only_skill_call(&call));
    }

    #[test]
    fn test_is_not_read_only_shell_exec() {
        let call = SkillCall {
            skill_name: "Shell".to_string(),
            method: "exec".to_string(),
            args: vec![],
        };
        assert!(!is_read_only_skill_call(&call));
    }

    // ── File.edit tests ──

    #[test]
    fn test_file_edit_replaces_content() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, "hello world").unwrap();

        let call = SkillCall {
            skill_name: "File".to_string(),
            method: "edit".to_string(),
            args: vec![json_str("test.txt"), json_str("hello"), json_str("goodbye")],
        };
        let result = file::execute_file(&call, Some(dir.path())).unwrap();
        assert_eq!(result, serde_json::json!({ "ok": true }));

        let content = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(content, "goodbye world");
    }

    #[test]
    fn test_file_edit_old_content_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, "hello world").unwrap();

        let call = SkillCall {
            skill_name: "File".to_string(),
            method: "edit".to_string(),
            args: vec![json_str("test.txt"), json_str("xyz"), json_str("abc")],
        };
        let result = file::execute_file(&call, Some(dir.path()));
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("not found in file"));
    }

    #[test]
    fn test_file_edit_empty_path() {
        let dir = tempfile::tempdir().unwrap();

        let call = SkillCall {
            skill_name: "File".to_string(),
            method: "edit".to_string(),
            args: vec![json_str(""), json_str("old"), json_str("new")],
        };
        let result = file::execute_file(&call, Some(dir.path()));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("path is required"));
    }

    #[test]
    fn test_file_edit_empty_old_content() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, "hello").unwrap();

        let call = SkillCall {
            skill_name: "File".to_string(),
            method: "edit".to_string(),
            args: vec![json_str("test.txt"), json_str(""), json_str("new")],
        };
        let result = file::execute_file(&call, Some(dir.path()));
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("old content is required"));
    }

    #[test]
    fn test_file_edit_path_traversal_blocked() {
        let dir = tempfile::tempdir().unwrap();

        let call = SkillCall {
            skill_name: "File".to_string(),
            method: "edit".to_string(),
            args: vec![
                json_str("../../../etc/passwd"),
                json_str("root"),
                json_str("hacked"),
            ],
        };
        let result = file::execute_file(&call, Some(dir.path()));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("traversal"));
    }

    // ── process::truncate_utf8 tests ──

    #[test]
    fn test_truncate_utf8_short_input() {
        let result = process::truncate_utf8(b"hello", 100);
        assert_eq!(result, "hello");
    }

    #[test]
    fn test_truncate_utf8_exact_limit() {
        let result = process::truncate_utf8(b"hello", 5);
        assert_eq!(result, "hello");
    }

    #[test]
    fn test_truncate_utf8_over_limit() {
        let result = process::truncate_utf8(b"hello world", 5);
        assert!(result.starts_with("hello"));
        assert!(result.contains("(truncated)"));
    }

    #[test]
    fn test_truncate_utf8_multibyte() {
        // "안녕" = 6 bytes in UTF-8, truncate at 4 should not panic
        let input = "안녕".as_bytes();
        let result = process::truncate_utf8(input, 4);
        assert!(result.contains("(truncated)"));
    }

    // ── Agent.delegate tests ──

    #[tokio::test]
    async fn test_agent_delegate_empty_task() {
        let call = SkillCall {
            skill_name: "Agent".to_string(),
            method: "delegate".to_string(),
            args: vec![json_str("")],
        };
        let config = kittypaw_core::config::Config::default();
        let result = agent::execute_agent(&call, &config).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("task description is required"));
    }

    #[tokio::test]
    async fn test_agent_delegate_depth_exceeded() {
        let call = SkillCall {
            skill_name: "Agent".to_string(),
            method: "delegate".to_string(),
            args: vec![json_str("do something"), serde_json::json!(2)],
        };
        let config = kittypaw_core::config::Config::default();
        let result = agent::execute_agent(&call, &config).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("max depth"));
    }

    #[tokio::test]
    async fn test_agent_delegate_no_provider() {
        let call = SkillCall {
            skill_name: "Agent".to_string(),
            method: "delegate".to_string(),
            args: vec![json_str("do something")],
        };
        let config = kittypaw_core::config::Config::default();
        let result = agent::execute_agent(&call, &config).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("no LLM provider"));
    }

    #[tokio::test]
    async fn test_agent_unknown_method() {
        let call = SkillCall {
            skill_name: "Agent".to_string(),
            method: "spawn".to_string(),
            args: vec![],
        };
        let config = kittypaw_core::config::Config::default();
        let result = agent::execute_agent(&call, &config).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Unknown Agent method"));
    }

    #[test]
    fn test_is_not_read_only_agent_delegate() {
        let call = SkillCall {
            skill_name: "Agent".to_string(),
            method: "delegate".to_string(),
            args: vec![],
        };
        assert!(!is_read_only_skill_call(&call));
    }
}
