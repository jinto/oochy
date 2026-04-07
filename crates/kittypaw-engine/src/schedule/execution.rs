use chrono::{DateTime, Utc};
use kittypaw_core::package::SkillPackage;
use kittypaw_core::skill::Skill;

use super::auto_fix::attempt_auto_fix;
use super::notification::NotificationSender;
use super::persistence::{
    get_failure_count, increment_failure_count, reset_failure_count, set_backoff_delay,
    set_last_run,
};

const MAX_RESULT_LEN: usize = 500;

pub fn append_execution_log(
    data_dir: &std::path::Path,
    skill_id: &str,
    success: bool,
    duration_ms: i64,
    output: &str,
) {
    let log_path = data_dir.join("execution.jsonl");
    let entry = serde_json::json!({
        "ts": chrono::Utc::now().to_rfc3339(),
        "skill": skill_id,
        "success": success,
        "duration_ms": duration_ms,
        "output": output.chars().take(200).collect::<String>(),
    });
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
    {
        use std::io::Write;
        let _ = writeln!(file, "{}", entry);
    }
}

#[allow(clippy::too_many_arguments)]
pub fn handle_execution_failure(
    store: &kittypaw_store::Store,
    db_path: &str,
    id: &str,
    name: &str,
    started_at: chrono::DateTime<chrono::Utc>,
    error_msg: &str,
    input_params: Option<&str>,
    can_disable: bool,
    usage_json: Option<&str>,
) {
    increment_failure_count(db_path, id).ok();
    let failures = get_failure_count(db_path, id);
    let finished_at = chrono::Utc::now();
    let duration_ms = (finished_at - started_at).num_milliseconds();
    let _ = store.record_execution(
        id,
        name,
        &started_at.to_rfc3339(),
        &finished_at.to_rfc3339(),
        duration_ms,
        &error_msg.chars().take(MAX_RESULT_LEN).collect::<String>(),
        false,
        failures as i32,
        input_params,
        usage_json,
    );
    // Store failure hint for self-improvement
    let hint_key = format!("failure_hint:{}", id);
    let hint = format!(
        "Failed at {}: {}",
        started_at.format("%H:%M"),
        error_msg.chars().take(200).collect::<String>()
    );
    let _ = store.set_user_context(&hint_key, &hint, "auto");
    if failures >= 3 {
        if can_disable {
            tracing::warn!(
                "Skill '{}' auto-disabled after {} consecutive failures",
                name,
                failures
            );
            let _ = kittypaw_core::skill::disable_skill(id);
        } else {
            // Package cannot be disabled, but still apply backoff to prevent infinite retry loop
            set_backoff_delay(db_path, id, failures).ok();
            tracing::warn!(
                "Package '{}' failed {} times, backing off {} seconds",
                name,
                failures,
                60 * (1u64 << failures.min(10))
            );
        }
    } else {
        set_backoff_delay(db_path, id, failures).ok();
        tracing::info!(
            "Skill '{}' will retry in {} seconds (attempt {}/3)",
            name,
            60 * (1u32 << failures.min(10)),
            failures
        );
    }
}

/// Notify retry and record a failed execution run (shared by skills and packages).
pub fn handle_run_failure(
    store: &kittypaw_store::Store,
    notifier: &NotificationSender,
    data_dir: &std::path::Path,
    db_path: &str,
    id: &str,
    name: &str,
    started_at: DateTime<Utc>,
    error_msg: &str,
    input_params: &str,
    can_disable: bool,
    usage_json: Option<&str>,
) {
    let finished_at = Utc::now();
    let duration_ms = (finished_at - started_at).num_milliseconds();
    let failures = get_failure_count(db_path, id) + 1;
    let delay_secs = 60u64 * (1u64 << failures.min(10));
    notifier.notify_retry(id, failures, delay_secs);
    handle_execution_failure(
        store,
        db_path,
        id,
        name,
        started_at,
        error_msg,
        Some(input_params),
        can_disable,
        usage_json,
    );
    append_execution_log(data_dir, id, false, duration_ms, error_msg);
}

/// Record a successful execution and persist pattern-detected defaults.
pub fn handle_run_success(
    store: &kittypaw_store::Store,
    notifier: &NotificationSender,
    data_dir: &std::path::Path,
    db_path: &str,
    id: &str,
    name: &str,
    started_at: DateTime<Utc>,
    output: &str,
    input_params: &str,
    usage_json: Option<&str>,
) {
    let finished_at = Utc::now();
    let duration_ms = (finished_at - started_at).num_milliseconds();
    let _ = store.record_execution(
        id,
        name,
        &started_at.to_rfc3339(),
        &finished_at.to_rfc3339(),
        duration_ms,
        &output.chars().take(MAX_RESULT_LEN).collect::<String>(),
        true,
        0,
        Some(input_params),
        usage_json,
    );
    set_last_run(db_path, id, Utc::now()).ok();
    reset_failure_count(db_path, id).ok();

    // Clear failure hint on success (self-improvement: retry succeeded)
    let hint_key = format!("failure_hint:{}", id);
    if store.get_user_context(&hint_key).ok().flatten().is_some() {
        let _ = store.set_user_context(&hint_key, "", "cleared");
        notifier.notify_recovery(id);
    }

    append_execution_log(data_dir, id, true, duration_ms, output);

    // Detect param patterns and persist as defaults
    if let Ok(patterns) = store.detect_param_patterns(id) {
        if !patterns.is_empty() {
            notifier.notify_patterns(id, &patterns);
        }
        for (key, value) in patterns {
            let ctx_key = format!("default:{}:{}", id, key);
            let _ = store.set_user_context(&ctx_key, &value, "pattern");
        }
    }

    // Detect time patterns and notify once
    let notified_key = format!("suggest_notified:{}", id);
    let dismissed_key = format!("suggest_dismissed:{}", id);
    let accepted_key = format!("schedule_accepted:{}", id);
    let already_handled = store
        .get_user_context(&notified_key)
        .ok()
        .flatten()
        .is_some()
        || store
            .get_user_context(&dismissed_key)
            .ok()
            .flatten()
            .is_some()
        || store
            .get_user_context(&accepted_key)
            .ok()
            .flatten()
            .is_some();
    if !already_handled {
        if let Ok(Some(_cron)) = store.detect_time_pattern(id) {
            notifier.send(&format!(
                "💡 *{}* — 일정한 패턴으로 실행하고 계시네요. 자동 스케줄로 전환할까요?\n\
                 `kittypaw suggestions accept {}`",
                name, id
            ));
            let _ = store.set_user_context(&notified_key, "1", "pattern");
        }
    }
}

/// Execute a single scheduled skill and handle the result.
pub async fn execute_scheduled_skill(
    skill: &Skill,
    js_code: &str,
    config: &kittypaw_core::config::Config,
    sandbox: &kittypaw_sandbox::sandbox::Sandbox,
    notifier: &NotificationSender,
    data_dir: &std::path::Path,
    db_path: &str,
) {
    tracing::info!("Running scheduled skill: {}", skill.name);
    let context = serde_json::json!({
        "event_type": "schedule",
        "event_text": "",
        "chat_id": "",
        "skill_name": skill.name,
    });
    let input_params = serde_json::to_string(&context).unwrap_or_default();
    let input_params = input_params
        .chars()
        .take(MAX_RESULT_LEN)
        .collect::<String>();
    let wrapped = format!("const ctx = JSON.parse(__context__);\n{js_code}");
    let started_at = Utc::now();

    // Build inline skill resolver so Http/Web/Telegram/etc. work during execution.
    let config_clone = config.clone();
    let db_path_str = db_path.to_string();
    let skill_perms = skill.permissions.clone();
    let skill_resolver: Option<kittypaw_sandbox::SkillResolver> = Some(std::sync::Arc::new(
        move |call: kittypaw_core::types::SkillCall| {
            let config = config_clone.clone();
            let db_path = db_path_str.clone();
            let perms = skill_perms.clone();
            Box::pin(async move {
                let store = match kittypaw_store::Store::open(&db_path) {
                    Ok(s) => s,
                    Err(_) => return "null".to_string(),
                };
                let store = std::sync::Arc::new(tokio::sync::Mutex::new(store));
                let checker =
                    kittypaw_core::capability::CapabilityChecker::from_skill_permissions(&perms);
                let checker = std::sync::Arc::new(std::sync::Mutex::new(checker));
                crate::skill_executor::resolve_skill_call(
                    &call,
                    &config,
                    &store,
                    Some(&checker),
                    None,
                )
                .await
            })
        },
    ));

    match sandbox
        .execute_with_resolver(&wrapped, context, skill_resolver)
        .await
    {
        Ok(result) if result.success => {
            // Skill calls are already resolved inline by the skill_resolver.
            // No need for 2-phase execute_skill_calls — just record success.
            // Store must be re-opened after the sandbox await: rusqlite::Connection is !Send.
            let store = match kittypaw_store::Store::open(db_path) {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!("Failed to open store for skill '{}': {e}", skill.name);
                    return;
                }
            };
            {
                tracing::info!(
                    "Scheduled skill '{}' completed: {}",
                    skill.name,
                    result.output
                );
                handle_run_success(
                    &store,
                    notifier,
                    data_dir,
                    db_path,
                    &skill.name,
                    &skill.name,
                    started_at,
                    &result.output,
                    &input_params,
                    None,
                );
            }
        }
        Ok(result) => {
            tracing::warn!(
                "Scheduled skill '{}' failed: {:?}",
                skill.name,
                result.error
            );
            let error_msg = result.error.unwrap_or_default();
            if let Ok(store) = kittypaw_store::Store::open(db_path) {
                handle_run_failure(
                    &store,
                    notifier,
                    data_dir,
                    db_path,
                    &skill.name,
                    &skill.name,
                    started_at,
                    &error_msg,
                    &input_params,
                    true,
                    None,
                );
                // Auto-fix: attempt on 2nd failure if not already tried
                let failures = get_failure_count(db_path, &skill.name);
                let hint = store
                    .get_user_context(&format!("failure_hint:{}", skill.name))
                    .ok()
                    .flatten()
                    .unwrap_or_default();
                if failures == 2 && !hint.contains("|auto_fix_attempted") {
                    if let Some(fix_result) =
                        attempt_auto_fix(&skill.name, &error_msg, config, sandbox, db_path).await
                    {
                        if fix_result.applied {
                            reset_failure_count(db_path, &skill.name).ok();
                            let _ = store.set_user_context(
                                &format!("failure_hint:{}", skill.name),
                                "",
                                "auto_fixed",
                            );
                            if fix_result.fix_id > 0 {
                                notifier.notify_fix_applied(
                                    &skill.name,
                                    &error_msg,
                                    fix_result.fix_id,
                                );
                            } else {
                                notifier.notify_recovery(&skill.name);
                            }
                        } else {
                            notifier.notify_fix_pending(&skill.name, &error_msg, fix_result.fix_id);
                            let _ = store.set_user_context(
                                &format!("failure_hint:{}", skill.name),
                                &format!("{hint}|auto_fix_attempted"),
                                "auto",
                            );
                        }
                    } else {
                        let _ = store.set_user_context(
                            &format!("failure_hint:{}", skill.name),
                            &format!("{hint}|auto_fix_attempted"),
                            "auto",
                        );
                    }
                }
            }
        }
        Err(e) => {
            tracing::error!("Scheduled skill '{}' execution error: {e}", skill.name);
            if let Ok(store) = kittypaw_store::Store::open(db_path) {
                handle_run_failure(
                    &store,
                    notifier,
                    data_dir,
                    db_path,
                    &skill.name,
                    &skill.name,
                    started_at,
                    &e.to_string(),
                    &input_params,
                    true,
                    None,
                );
            }
        }
    }
}

/// Execute chain steps sequentially, piping each output into the next.
pub async fn execute_chain_steps(
    pkg: &SkillPackage,
    initial_output: &str,
    pkg_mgr: &kittypaw_core::package_manager::PackageManager,
    config: &kittypaw_core::config::Config,
    sandbox: &kittypaw_sandbox::sandbox::Sandbox,
    db_path: &str,
    shared_ctx: &std::collections::HashMap<String, String>,
) {
    let chain_steps = match pkg_mgr.load_chain(pkg) {
        Ok(steps) => steps,
        Err(_) => return,
    };
    let mut prev_output = initial_output.to_string();
    for (step_idx, (chain_pkg, chain_js)) in chain_steps.iter().enumerate() {
        let chain_config = pkg_mgr
            .get_config_with_defaults(&chain_pkg.meta.id)
            .unwrap_or_default();
        let chain_context = chain_pkg.build_context(
            &chain_config,
            serde_json::json!({}),
            Some(&prev_output),
            shared_ctx,
        );
        let chain_wrapped = format!("const ctx = JSON.parse(__context__);\n{chain_js}");
        match sandbox.execute(&chain_wrapped, chain_context).await {
            Ok(chain_result) if chain_result.success => {
                // Execute captured skill calls (Telegram, Http, etc.)
                // Store must be re-opened after the chain await: rusqlite::Connection is !Send.
                if !chain_result.skill_calls.is_empty() {
                    let store = match kittypaw_store::Store::open(db_path) {
                        Ok(s) => s,
                        Err(e) => {
                            tracing::error!(
                                "Failed to open store for chain step '{}': {e}",
                                chain_pkg.meta.id
                            );
                            break;
                        }
                    };
                    let preresolved = crate::skill_executor::resolve_storage_calls(
                        &chain_result.skill_calls,
                        &store,
                        Some(&chain_pkg.meta.id),
                    );
                    let mut checker =
                        kittypaw_core::capability::CapabilityChecker::from_package_permissions(
                            &chain_pkg.permissions,
                        );
                    // Use per-step model override from chain definition
                    let chain_model = pkg
                        .chain
                        .get(step_idx)
                        .and_then(|s| s.model.as_deref())
                        .or(chain_pkg.model.as_deref());
                    match crate::skill_executor::execute_skill_calls(
                        &chain_result.skill_calls,
                        config,
                        preresolved,
                        Some(&chain_pkg.meta.id),
                        Some(&mut checker),
                        chain_model,
                    )
                    .await
                    {
                        Ok(call_results) => {
                            for r in call_results.iter().filter(|r| !r.success) {
                                tracing::warn!(
                                    "Chain step '{}': skill call {}.{} failed: {:?}",
                                    chain_pkg.meta.id,
                                    r.skill_name,
                                    r.method,
                                    r.error
                                );
                            }
                        }
                        Err(e) => {
                            tracing::warn!(
                                "Chain step '{}': execute_skill_calls error: {e}",
                                chain_pkg.meta.id
                            );
                        }
                    }
                }
                tracing::info!(
                    "Chain step '{}' completed: {}",
                    chain_pkg.meta.id,
                    chain_result.output
                );
                prev_output = chain_result.output;
            }
            Ok(chain_result) => {
                tracing::warn!(
                    "Chain step '{}' failed: {:?}",
                    chain_pkg.meta.id,
                    chain_result.error
                );
                break;
            }
            Err(e) => {
                tracing::error!("Chain step '{}' execution error: {e}", chain_pkg.meta.id);
                break;
            }
        }
    }
}

/// Collect pattern-detected defaults and build config for a package.
pub fn prepare_package_context(
    pkg: &SkillPackage,
    store: &kittypaw_store::Store,
    pkg_mgr: &kittypaw_core::package_manager::PackageManager,
    shared_ctx: &std::collections::HashMap<String, String>,
) -> (
    serde_json::Value,
    std::collections::HashMap<String, String>,
    String,
) {
    let pattern_defaults: std::collections::HashMap<String, String> = {
        let prefix = format!("default:{}:", pkg.meta.id);
        let mut map = std::collections::HashMap::new();
        if let Ok(ctx_keys) = store.list_user_context_prefix(&prefix) {
            for (full_key, value) in ctx_keys {
                let config_key = full_key[prefix.len()..].to_string();
                map.insert(config_key, value);
            }
        }
        map
    };
    let config_values = pkg_mgr
        .get_config_with_defaults_and_patterns(&pkg.meta.id, &pattern_defaults)
        .unwrap_or_default();
    let event_payload = serde_json::json!({
        "event_type": "schedule",
    });
    let context = pkg.build_context(&config_values, event_payload, None, shared_ctx);
    // Filter out secret fields before recording to execution history
    let mut safe_config = config_values.clone();
    safe_config.retain(|k, _| {
        !k.contains("token")
            && !k.contains("secret")
            && !k.contains("api_key")
            && !k.starts_with("sk-")
    });
    let input_params = serde_json::to_string(&safe_config).unwrap_or_default();
    let input_params = input_params
        .chars()
        .take(MAX_RESULT_LEN)
        .collect::<String>();
    (context, config_values, input_params)
}

/// Execute a single scheduled package and handle the result.
#[allow(clippy::too_many_arguments)]
pub async fn execute_scheduled_package(
    pkg: &SkillPackage,
    js_code: &str,
    config: &kittypaw_core::config::Config,
    sandbox: &kittypaw_sandbox::sandbox::Sandbox,
    notifier: &NotificationSender,
    data_dir: &std::path::Path,
    db_path: &str,
    pkg_mgr: &kittypaw_core::package_manager::PackageManager,
    shared_ctx: &std::collections::HashMap<String, String>,
    context: serde_json::Value,
    config_values: &std::collections::HashMap<String, String>,
    input_params: &str,
) {
    let wrapped = format!("const ctx = JSON.parse(__context__);\n{js_code}");
    let started_at = Utc::now();
    match sandbox.execute(&wrapped, context).await {
        Ok(result) if result.success => {
            // Open a fresh store after the await point (Store is !Sync)
            let store = match kittypaw_store::Store::open(db_path) {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!("Failed to open store for package '{}': {e}", pkg.meta.id);
                    return;
                }
            };
            let call_error: Option<String> = if !result.skill_calls.is_empty() {
                let preresolved = crate::skill_executor::resolve_storage_calls(
                    &result.skill_calls,
                    &store,
                    Some(&pkg.meta.id),
                );
                let mut checker =
                    kittypaw_core::capability::CapabilityChecker::from_package_permissions(
                        &pkg.permissions,
                    );
                let pkg_model_override = pkg.model.as_deref().or_else(|| {
                    config_values
                        .get("_model")
                        .map(String::as_str)
                        .filter(|s| !s.is_empty())
                });
                match crate::skill_executor::execute_skill_calls(
                    &result.skill_calls,
                    config,
                    preresolved,
                    Some(&pkg.meta.id),
                    Some(&mut checker),
                    pkg_model_override,
                )
                .await
                {
                    Ok(results) => results
                        .iter()
                        .find(|r| !r.success)
                        .and_then(|r| r.error.clone()),
                    Err(e) => Some(e.to_string()),
                }
            } else {
                None
            };
            if let Some(ref err_msg) = call_error {
                tracing::warn!(
                    "Scheduled package '{}' skill_call failed: {}",
                    pkg.meta.id,
                    err_msg
                );
                handle_run_failure(
                    &store,
                    notifier,
                    data_dir,
                    db_path,
                    &pkg.meta.id,
                    &pkg.meta.name,
                    started_at,
                    err_msg,
                    input_params,
                    false,
                    None,
                );
            } else {
                tracing::info!(
                    "Scheduled package '{}' completed: {}",
                    pkg.meta.id,
                    result.output
                );
                handle_run_success(
                    &store,
                    notifier,
                    data_dir,
                    db_path,
                    &pkg.meta.id,
                    &pkg.meta.name,
                    started_at,
                    &result.output,
                    input_params,
                    None,
                );

                // Execute chain steps if present
                if !pkg.chain.is_empty() {
                    execute_chain_steps(
                        pkg,
                        &result.output,
                        pkg_mgr,
                        config,
                        sandbox,
                        db_path,
                        shared_ctx,
                    )
                    .await;
                }
            }
        }
        Ok(result) => {
            tracing::warn!(
                "Scheduled package '{}' failed: {:?}",
                pkg.meta.id,
                result.error
            );
            let error_msg = result.error.unwrap_or_default();
            if let Ok(store) = kittypaw_store::Store::open(db_path) {
                handle_run_failure(
                    &store,
                    notifier,
                    data_dir,
                    db_path,
                    &pkg.meta.id,
                    &pkg.meta.name,
                    started_at,
                    &error_msg,
                    input_params,
                    false,
                    None,
                );
            }
        }
        Err(e) => {
            tracing::error!("Scheduled package '{}' execution error: {e}", pkg.meta.id);
            if let Ok(store) = kittypaw_store::Store::open(db_path) {
                handle_run_failure(
                    &store,
                    notifier,
                    data_dir,
                    db_path,
                    &pkg.meta.id,
                    &pkg.meta.name,
                    started_at,
                    &e.to_string(),
                    input_params,
                    false,
                    None,
                );
            }
        }
    }
}
