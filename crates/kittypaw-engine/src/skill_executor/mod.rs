mod agent;
mod discord;
mod env;
mod file;
mod git;
mod http;
mod image;
mod llm;
mod memory;
mod moa;
mod process;
mod shell;
mod skill_mgmt;
mod slack;
mod storage;
mod telegram;
mod todo;
mod tts;

use std::sync::atomic::AtomicU32;
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

/// Serialize an error as JSON with the `_error` sentinel so the JS sandbox
/// wrapper can detect it and `throw` instead of returning silently.
fn skill_error_json(msg: impl std::fmt::Display) -> String {
    serde_json::to_string(&serde_json::json!({"_error": true, "error": msg.to_string()}))
        .unwrap_or_else(|_| r#"{"_error":true,"error":"serialization failed"}"#.to_string())
}

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

/// Check file access against `config.sandbox.allowed_paths`, then fall back to
/// a UI permission callback if the path is outside the allowed set.
///
/// Three-tier check:
/// 1. If `allowed_paths` is empty → auto-allow (backward compatible, no restrictions)
/// 2. If path is within an allowed directory → allow
/// 3. Otherwise → ask user via `on_permission` callback (or deny if no callback)
async fn check_file_allowed(
    call: &SkillCall,
    config: &kittypaw_core::config::Config,
    on_permission: Option<&PermissionCallback>,
) -> std::result::Result<(), String> {
    let action = match call.method.as_str() {
        "read" => "read",
        "write" | "edit" => "write",
        _ => return Ok(()), // unknown methods are rejected later by execute_file
    };

    let path = call
        .args
        .first()
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    if path.is_empty() {
        return Ok(()); // will be caught by execute_file validation
    }

    // Tier 1: no allowed_paths configured → backward-compatible auto-allow
    let allowed = &config.sandbox.allowed_paths;
    if allowed.is_empty() {
        return Ok(());
    }

    // Tier 2: check if path falls within any allowed directory
    let target = std::path::Path::new(&path);
    if is_within_allowed_paths(target, allowed) {
        return Ok(());
    }

    // Tier 3: path is outside allowed set → ask user or deny
    let cb = match on_permission {
        Some(cb) => cb,
        None => return Err(format!("File.{action} denied: path not in allowed_paths")),
    };

    let request = PermissionRequest {
        request_id: uuid_v4(),
        resource_kind: ResourceKind::File,
        resource_path: path,
        action: action.to_string(),
        workspace_id: String::new(),
    };

    let rx = cb(request);
    match rx.await {
        Ok(PermissionDecision::AllowOnce | PermissionDecision::AllowPermanent) => Ok(()),
        Ok(PermissionDecision::Deny) => Err(format!("Permission denied: File.{action}")),
        Err(_) => Err("Permission check failed: response channel dropped".to_string()),
    }
}

/// Check if a path falls within any of the allowed directory prefixes.
fn is_within_allowed_paths(path: &std::path::Path, allowed: &[std::path::PathBuf]) -> bool {
    // Try to canonicalize for symlink resolution; fall back to raw comparison
    let check_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    allowed.iter().any(|dir| {
        let check_dir = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
        check_path.starts_with(&check_dir)
    })
}

fn uuid_v4() -> String {
    uuid::Uuid::new_v4().to_string()
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
        return skill_error_json(format!(
            "Result too large ({} bytes, limit {})",
            result.len(),
            MAX_SKILL_RESULT_BYTES
        ));
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
                    return skill_error_json(msg);
                }
            }
            Err(_) => {
                return skill_error_json("capability checker lock poisoned");
            }
        }
    }

    // File calls are synchronous — but require path + permission checks first
    if call.skill_name == "File" {
        if let Err(msg) = check_file_allowed(call, config, on_permission).await {
            return serde_json::to_string(&serde_json::json!({"error": msg}))
                .unwrap_or_else(|_| "null".to_string());
        }
        return match file::execute_file(call, None) {
            Ok(val) => serde_json::to_string(&val).unwrap_or_else(|_| "null".to_string()),
            Err(e) => skill_error_json(e),
        };
    }

    // Env calls are synchronous
    if call.skill_name == "Env" {
        return match env::execute_env(call, None) {
            Ok(val) => serde_json::to_string(&val).unwrap_or_else(|_| "null".to_string()),
            Err(e) => skill_error_json(e),
        };
    }

    // Web calls are async (HTTP), go through the async path
    if call.skill_name == "Web" {
        let result = http::execute_web(call, &config.sandbox.allowed_hosts).await;
        return match result {
            Ok(val) => serde_json::to_string(&val).unwrap_or_else(|_| "null".to_string()),
            Err(e) => skill_error_json(e),
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
            return skill_error_json("MCP not configured");
        }
    }

    // Storage calls need Store access
    if call.skill_name == "Storage" {
        let s = store.lock().await;
        return match storage::execute_storage(call, &s, None) {
            Ok(val) => serde_json::to_string(&val).unwrap_or_else(|_| "null".to_string()),
            Err(e) => skill_error_json(e),
        };
    }

    // Todo calls need Store access
    if call.skill_name == "Todo" {
        let s = store.lock().await;
        return match todo::execute_todo(call, &s) {
            Ok(val) => serde_json::to_string(&val).unwrap_or_else(|_| "null".to_string()),
            Err(e) => skill_error_json(e),
        };
    }

    // Memory calls need Store access
    if call.skill_name == "Memory" {
        let s = store.lock().await;
        return match memory::execute_memory(call, &s, None) {
            Ok(val) => serde_json::to_string(&val).unwrap_or_else(|_| "null".to_string()),
            Err(e) => skill_error_json(e),
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
        let msg = result.error.as_deref().unwrap_or("unknown");
        tracing::warn!(
            skill = %call.skill_name,
            method = %call.method,
            error = %msg,
            "skill call failed (error returned to JS sandbox)"
        );
        skill_error_json(msg)
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
            | ("Memory", "recall")
            | ("Memory", "search")
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
    tracing::debug!(total_calls = skill_calls.len(), "executing skill calls");
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
        "File" => match check_file_allowed(call, config, on_permission).await {
            Ok(()) => file::execute_file(call, None),
            Err(msg) => Err(KittypawError::CapabilityDenied(msg)),
        },
        "Env" => env::execute_env(call, None),
        "Shell" => shell::execute_shell(call).await,
        "Git" => git::execute_git(call).await,
        "Agent" => agent::execute_agent(call, config).await,
        "Skill" => skill_mgmt::execute_skill_mgmt(call).await,
        "Tts" => tts::execute_tts(call).await,
        "Moa" => moa::execute_moa(call, config).await,
        "Image" => image::execute_image(call, config).await,
        "Vision" => image::execute_vision(call, config).await,
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
fn resolve_channel_token(
    config: &kittypaw_core::config::Config,
    channel_type: &str,
    secret_key: &str,
    env_var: &str,
) -> Option<String> {
    let result =
        kittypaw_core::credential::resolve_credential(channel_type, secret_key, env_var, config)
            // Also try {channel}/bot_token for GUI onboarding path
            .or_else(|| {
                kittypaw_core::secrets::get_secret(channel_type, "bot_token")
                    .ok()
                    .flatten()
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
mod tests;
