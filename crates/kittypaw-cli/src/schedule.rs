use chrono::{DateTime, Utc};
use cron::Schedule as CronSchedule;
use kittypaw_core::package::SkillPackage;
use kittypaw_core::skill::Skill;
use rusqlite::{params, Connection};
use std::str::FromStr;

const MAX_RESULT_LEN: usize = 500;

/// Validate a cron expression and enforce minimum 5-minute interval.
pub fn validate_cron(expr: &str) -> Result<(), String> {
    let schedule =
        CronSchedule::from_str(expr).map_err(|e| format!("Invalid cron expression: {e}"))?;

    // Check minimum interval: get next 2 occurrences and ensure gap >= 5 min
    let now = Utc::now();
    let mut upcoming = schedule.upcoming(Utc).take(2);
    if let (Some(first), Some(second)) = (upcoming.next(), upcoming.next()) {
        let gap = second - first;
        if gap.num_minutes() < 5 {
            return Err(format!(
                "Schedule interval too short ({} minutes). Minimum is 5 minutes.",
                gap.num_minutes()
            ));
        }
    }
    let _ = now;
    Ok(())
}

/// Check if a package is due to run based on its cron trigger.
pub fn is_package_due(pkg: &SkillPackage, last_run: Option<DateTime<Utc>>) -> bool {
    let trigger = match &pkg.trigger {
        Some(t) if t.trigger_type == "schedule" => t,
        _ => return false,
    };
    let cron_expr = match &trigger.cron {
        Some(c) => c,
        None => return false,
    };
    let schedule = match CronSchedule::from_str(cron_expr) {
        Ok(s) => s,
        Err(_) => return false,
    };

    let reference = last_run.unwrap_or_else(|| Utc::now() - chrono::Duration::hours(24));
    schedule
        .after(&reference)
        .take_while(|t| *t <= Utc::now())
        .next()
        .is_some()
}

/// Check if a skill is due to run based on its cron schedule.
pub fn is_due(skill: &Skill, last_run: Option<DateTime<Utc>>) -> bool {
    if skill.trigger.trigger_type != "schedule" || !skill.enabled {
        return false;
    }
    let cron_expr = match &skill.trigger.cron {
        Some(c) => c,
        None => return false,
    };
    let schedule = match CronSchedule::from_str(cron_expr) {
        Ok(s) => s,
        Err(_) => return false,
    };

    let reference = last_run.unwrap_or_else(|| Utc::now() - chrono::Duration::hours(24));
    // If any scheduled time between last_run and now, it's due
    schedule
        .after(&reference)
        .take_while(|t| *t <= Utc::now())
        .next()
        .is_some()
}

// --- Schedule persistence ---

fn open_schedule_db(db_path: &str) -> Result<Connection, String> {
    let conn = Connection::open(db_path).map_err(|e| format!("Failed to open schedule db: {e}"))?;
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|e| format!("Failed to set busy timeout: {e}"))?;
    Ok(conn)
}

pub fn init_schedule_db(db_path: &str) -> Result<(), String> {
    let conn = open_schedule_db(db_path)?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS skill_schedule (
            skill_name TEXT PRIMARY KEY,
            last_run_at TEXT,
            failure_count INTEGER DEFAULT 0
        );",
    )
    .map_err(|e| format!("Failed to create skill_schedule table: {e}"))?;
    Ok(())
}

pub fn get_last_run(db_path: &str, skill_name: &str) -> Option<DateTime<Utc>> {
    let conn = open_schedule_db(db_path).ok()?;
    let result: rusqlite::Result<String> = conn.query_row(
        "SELECT last_run_at FROM skill_schedule WHERE skill_name = ?1",
        params![skill_name],
        |row| row.get(0),
    );
    match result {
        Ok(s) => s.parse::<DateTime<Utc>>().ok(),
        Err(_) => None,
    }
}

pub fn set_last_run(db_path: &str, skill_name: &str, time: DateTime<Utc>) -> Result<(), String> {
    let conn = open_schedule_db(db_path)?;
    conn.execute(
        "INSERT OR REPLACE INTO skill_schedule (skill_name, last_run_at, failure_count)
         VALUES (?1, ?2, COALESCE((SELECT failure_count FROM skill_schedule WHERE skill_name = ?1), 0))",
        params![skill_name, time.to_rfc3339()],
    )
    .map_err(|e| format!("Failed to set last_run: {e}"))?;
    Ok(())
}

pub fn get_failure_count(db_path: &str, skill_name: &str) -> u32 {
    let conn = match open_schedule_db(db_path) {
        Ok(c) => c,
        Err(_) => return 0,
    };
    let result: rusqlite::Result<u32> = conn.query_row(
        "SELECT failure_count FROM skill_schedule WHERE skill_name = ?1",
        params![skill_name],
        |row| row.get(0),
    );
    result.unwrap_or(0)
}

pub fn increment_failure_count(db_path: &str, skill_name: &str) -> Result<(), String> {
    let conn = open_schedule_db(db_path)?;
    conn.execute(
        "INSERT INTO skill_schedule (skill_name, last_run_at, failure_count)
         VALUES (?1, NULL, 1)
         ON CONFLICT(skill_name) DO UPDATE SET failure_count = failure_count + 1",
        params![skill_name],
    )
    .map_err(|e| format!("Failed to increment failure_count: {e}"))?;
    Ok(())
}

/// After a failure, set last_run_at to a future time for exponential backoff.
/// retry_delay = 60 * 2^failure_count seconds (1min, 2min, 4min).
pub fn set_backoff_delay(
    db_path: &str,
    skill_name: &str,
    failure_count: u32,
) -> Result<(), String> {
    let delay_secs = 60i64 * (1i64 << failure_count.min(10));
    let backoff_time = Utc::now() + chrono::Duration::seconds(delay_secs);
    let conn = open_schedule_db(db_path)?;
    conn.execute(
        "UPDATE skill_schedule SET last_run_at = ?1 WHERE skill_name = ?2",
        params![backoff_time.to_rfc3339(), skill_name],
    )
    .map_err(|e| format!("Failed to set backoff delay: {e}"))?;
    Ok(())
}

pub fn reset_failure_count(db_path: &str, skill_name: &str) -> Result<(), String> {
    let conn = open_schedule_db(db_path)?;
    conn.execute(
        "UPDATE skill_schedule SET failure_count = 0 WHERE skill_name = ?1",
        params![skill_name],
    )
    .map_err(|e| format!("Failed to reset failure_count: {e}"))?;
    Ok(())
}

// --- Schedule loop ---

/// Caches HTTP client and channel credentials; sends notifications without
/// re-reading secrets or re-allocating a client on every call.
struct NotificationSender {
    client: reqwest::Client,
    telegram: Option<(String, String)>, // (token, chat_id)
    slack: Option<(String, String)>,    // (token, channel)
}

impl NotificationSender {
    fn new() -> Self {
        let telegram = match (
            kittypaw_core::secrets::get_secret("channels", "telegram_token")
                .ok()
                .flatten()
                .filter(|s| !s.is_empty()),
            kittypaw_core::secrets::get_secret("channels", "chat_id")
                .ok()
                .flatten()
                .filter(|s| !s.is_empty()),
        ) {
            (Some(token), Some(chat_id)) => Some((token, chat_id)),
            _ => None,
        };
        let slack = match (
            kittypaw_core::secrets::get_secret("channels", "slack_token")
                .ok()
                .flatten()
                .filter(|s| !s.is_empty()),
            kittypaw_core::secrets::get_secret("channels", "slack_channel")
                .ok()
                .flatten()
                .filter(|s| !s.is_empty()),
        ) {
            (Some(token), Some(channel)) => Some((token, channel)),
            _ => None,
        };
        Self {
            client: reqwest::Client::new(),
            telegram,
            slack,
        }
    }

    fn send(&self, message: &str) {
        if let Some((token, chat_id)) = &self.telegram {
            let url = format!("https://api.telegram.org/bot{}/sendMessage", token);
            let body =
                serde_json::json!({"chat_id": chat_id, "text": message, "parse_mode": "Markdown"});
            let client = self.client.clone();
            tokio::spawn(async move {
                if let Err(e) = client.post(&url).json(&body).send().await {
                    tracing::warn!("Notification failed: {e}");
                }
            });
            return;
        }
        if let Some((token, channel)) = &self.slack {
            let body = serde_json::json!({"channel": channel, "text": message});
            let client = self.client.clone();
            let token = token.clone();
            tokio::spawn(async move {
                if let Err(e) = client
                    .post("https://slack.com/api/chat.postMessage")
                    .header("Authorization", format!("Bearer {}", token))
                    .json(&body)
                    .send()
                    .await
                {
                    tracing::warn!("Slack notification failed: {e}");
                }
            });
        }
    }

    fn notify_recovery(&self, name: &str) {
        self.send(&format!(
            "🔧 *{}* 자동 복구됨\n실패 후 자동 재시도로 정상 작동 중입니다.",
            name
        ));
    }

    fn notify_patterns(&self, name: &str, patterns: &[(String, String)]) {
        let list: Vec<String> = patterns
            .iter()
            .map(|(k, v)| format!("  → {} = {}", k, v))
            .collect();
        self.send(&format!(
            "📊 *{}* 패턴 감지\n반복 사용된 값을 기본값으로 설정했습니다:\n{}",
            name,
            list.join("\n")
        ));
    }

    fn notify_retry(&self, name: &str, failures: u32, delay_secs: u64) {
        self.send(&format!(
            "⏳ *{}* 재시도 예정\n{}초 후 자동 재시도합니다 (시도 {}/3).",
            name, delay_secs, failures
        ));
    }
}

fn append_execution_log(
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
fn handle_execution_failure(
    store: &kittypaw_store::Store,
    db_path: &str,
    id: &str,
    name: &str,
    started_at: chrono::DateTime<chrono::Utc>,
    error_msg: &str,
    input_params: Option<&str>,
    can_disable: bool,
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

pub async fn run_schedule_loop(
    config: &kittypaw_core::config::Config,
    sandbox: &kittypaw_sandbox::sandbox::Sandbox,
    db_path: &str,
) {
    init_schedule_db(db_path).ok();
    let data_dir = std::path::Path::new(db_path)
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .to_path_buf();
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
    loop {
        interval.tick().await;
        let store = match kittypaw_store::Store::open(db_path) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("Failed to open store for schedule loop: {e}");
                continue;
            }
        };
        let notifier = NotificationSender::new();
        let _ = store.cleanup_old_executions(30);
        // Clean up execution.jsonl — delete if larger than 10MB
        {
            let log_path = data_dir.join("execution.jsonl");
            if log_path.exists() {
                if let Ok(metadata) = std::fs::metadata(&log_path) {
                    if metadata.len() > 10_000_000 {
                        let _ = std::fs::remove_file(&log_path);
                    }
                }
            }
        }
        if let Ok(skills) = kittypaw_core::skill::load_all_skills() {
            for (skill, js_code) in &skills {
                if skill.trigger.trigger_type != "schedule" || !skill.enabled {
                    continue;
                }
                let last_run = get_last_run(db_path, &skill.name);
                if !is_due(skill, last_run) {
                    continue;
                }

                tracing::info!("Running scheduled skill: {}", skill.name);
                let context = serde_json::json!({
                    "event_type": "schedule",
                    "event_text": "",
                    "chat_id": "",
                    "skill_name": skill.name,
                });
                let skill_input_params = serde_json::to_string(&context).unwrap_or_default();
                let skill_input_params = skill_input_params
                    .chars()
                    .take(MAX_RESULT_LEN)
                    .collect::<String>();
                let wrapped = format!("const ctx = JSON.parse(__context__);\n{js_code}");
                let skill_started_at = Utc::now();
                match sandbox.execute(&wrapped, context).await {
                    Ok(result) if result.success => {
                        if !result.skill_calls.is_empty() {
                            let preresolved = crate::skill_executor::resolve_storage_calls(
                                &result.skill_calls,
                                &store,
                                Some(&skill.name),
                            );
                            let mut checker = kittypaw_core::capability::CapabilityChecker::from_skill_permissions(&skill.permissions);
                            let _ = crate::skill_executor::execute_skill_calls(
                                &result.skill_calls,
                                config,
                                preresolved,
                                Some(&skill.name),
                                Some(&mut checker),
                                None,
                            )
                            .await;
                        }
                        tracing::info!(
                            "Scheduled skill '{}' completed: {}",
                            skill.name,
                            result.output
                        );
                        let skill_finished_at = Utc::now();
                        let duration_ms = (skill_finished_at - skill_started_at).num_milliseconds();
                        let _ = store.record_execution(
                            &skill.name,
                            &skill.name,
                            &skill_started_at.to_rfc3339(),
                            &skill_finished_at.to_rfc3339(),
                            duration_ms,
                            &result
                                .output
                                .chars()
                                .take(MAX_RESULT_LEN)
                                .collect::<String>(),
                            true,
                            0,
                            Some(&skill_input_params),
                        );
                        set_last_run(db_path, &skill.name, Utc::now()).ok();
                        reset_failure_count(db_path, &skill.name).ok();

                        // Clear failure hint on success (self-improvement: retry succeeded)
                        let hint_key = format!("failure_hint:{}", skill.name);
                        if store.get_user_context(&hint_key).ok().flatten().is_some() {
                            let _ = store.set_user_context(&hint_key, "", "cleared");
                            notifier.notify_recovery(&skill.name);
                        }

                        append_execution_log(
                            &data_dir,
                            &skill.name,
                            true,
                            duration_ms,
                            &result.output,
                        );

                        // Detect param patterns and persist as defaults
                        if let Ok(patterns) = store.detect_param_patterns(&skill.name) {
                            if !patterns.is_empty() {
                                notifier.notify_patterns(&skill.name, &patterns);
                            }
                            for (key, value) in patterns {
                                let ctx_key = format!("default:{}:{}", skill.name, key);
                                let _ = store.set_user_context(&ctx_key, &value, "pattern");
                            }
                        }
                    }
                    Ok(result) => {
                        tracing::warn!(
                            "Scheduled skill '{}' failed: {:?}",
                            skill.name,
                            result.error
                        );
                        let error_msg = result.error.unwrap_or_default();
                        let skill_finished_at = chrono::Utc::now();
                        let duration_ms = (skill_finished_at - skill_started_at).num_milliseconds();
                        let failures = get_failure_count(db_path, &skill.name) + 1;
                        let delay_secs = 60u64 * (1u64 << failures.min(10));
                        notifier.notify_retry(&skill.name, failures, delay_secs);
                        handle_execution_failure(
                            &store,
                            db_path,
                            &skill.name,
                            &skill.name,
                            skill_started_at,
                            &error_msg,
                            Some(&skill_input_params),
                            true,
                        );
                        append_execution_log(
                            &data_dir,
                            &skill.name,
                            false,
                            duration_ms,
                            &error_msg,
                        );
                    }
                    Err(e) => {
                        tracing::error!("Scheduled skill '{}' execution error: {e}", skill.name);
                        let skill_finished_at = chrono::Utc::now();
                        let duration_ms = (skill_finished_at - skill_started_at).num_milliseconds();
                        let err_str = e.to_string();
                        let failures = get_failure_count(db_path, &skill.name) + 1;
                        let delay_secs = 60u64 * (1u64 << failures.min(10));
                        notifier.notify_retry(&skill.name, failures, delay_secs);
                        handle_execution_failure(
                            &store,
                            db_path,
                            &skill.name,
                            &skill.name,
                            skill_started_at,
                            &err_str,
                            Some(&skill_input_params),
                            true,
                        );
                        append_execution_log(&data_dir, &skill.name, false, duration_ms, &err_str);
                    }
                }
            }
        }

        // --- Run scheduled packages ---
        let packages_dir = std::path::PathBuf::from(".kittypaw/packages");
        if let Ok(packages) = kittypaw_core::package_manager::load_all_packages(&packages_dir) {
            let pkg_mgr = kittypaw_core::package_manager::PackageManager::new(packages_dir.clone());
            let shared_ctx = store.list_shared_context().unwrap_or_default();
            for (pkg, js_code) in &packages {
                let last_run = get_last_run(db_path, &pkg.meta.id);
                if !is_package_due(pkg, last_run) {
                    continue;
                }

                tracing::info!("Running scheduled package: {}", pkg.meta.id);
                // Collect pattern-detected defaults from user_context
                let pattern_defaults: std::collections::HashMap<String, String> = {
                    let prefix = format!("default:{}:", pkg.meta.id);
                    let mut map = std::collections::HashMap::new();
                    // Read all user_context keys with this prefix via store
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
                let context = pkg.build_context(&config_values, event_payload, None, &shared_ctx);
                // Filter out secret fields before recording to execution history
                let mut safe_config = config_values.clone();
                safe_config.retain(|k, _| {
                    !k.contains("token")
                        && !k.contains("secret")
                        && !k.contains("api_key")
                        && !k.starts_with("sk-")
                });
                let pkg_input_params = serde_json::to_string(&safe_config).unwrap_or_default();
                let pkg_input_params = pkg_input_params
                    .chars()
                    .take(MAX_RESULT_LEN)
                    .collect::<String>();
                let wrapped = format!("const ctx = JSON.parse(__context__);\n{js_code}");
                let pkg_started_at = Utc::now();
                match sandbox.execute(&wrapped, context).await {
                    Ok(result) if result.success => {
                        if !result.skill_calls.is_empty() {
                            let preresolved = crate::skill_executor::resolve_storage_calls(
                                &result.skill_calls,
                                &store,
                                Some(&pkg.meta.id),
                            );
                            let mut checker = kittypaw_core::capability::CapabilityChecker::from_package_permissions(&pkg.permissions);
                            let pkg_model_override = pkg.model.as_deref().or_else(|| {
                                config_values
                                    .get("_model")
                                    .map(String::as_str)
                                    .filter(|s| !s.is_empty())
                            });
                            let _ = crate::skill_executor::execute_skill_calls(
                                &result.skill_calls,
                                config,
                                preresolved,
                                Some(&pkg.meta.id),
                                Some(&mut checker),
                                pkg_model_override,
                            )
                            .await;
                        }
                        tracing::info!(
                            "Scheduled package '{}' completed: {}",
                            pkg.meta.id,
                            result.output
                        );
                        let pkg_finished_at = Utc::now();
                        let duration_ms = (pkg_finished_at - pkg_started_at).num_milliseconds();
                        let _ = store.record_execution(
                            &pkg.meta.id,
                            &pkg.meta.name,
                            &pkg_started_at.to_rfc3339(),
                            &pkg_finished_at.to_rfc3339(),
                            duration_ms,
                            &result
                                .output
                                .chars()
                                .take(MAX_RESULT_LEN)
                                .collect::<String>(),
                            true,
                            0,
                            Some(&pkg_input_params),
                        );
                        set_last_run(db_path, &pkg.meta.id, Utc::now()).ok();
                        reset_failure_count(db_path, &pkg.meta.id).ok();

                        // Clear failure hint on success (self-improvement: retry succeeded)
                        let hint_key = format!("failure_hint:{}", pkg.meta.id);
                        if store.get_user_context(&hint_key).ok().flatten().is_some() {
                            let _ = store.set_user_context(&hint_key, "", "cleared");
                            notifier.notify_recovery(&pkg.meta.id);
                        }

                        append_execution_log(
                            &data_dir,
                            &pkg.meta.id,
                            true,
                            duration_ms,
                            &result.output,
                        );

                        // Detect param patterns and persist as defaults
                        if let Ok(patterns) = store.detect_param_patterns(&pkg.meta.id) {
                            if !patterns.is_empty() {
                                notifier.notify_patterns(&pkg.meta.id, &patterns);
                            }
                            for (key, value) in patterns {
                                let ctx_key = format!("default:{}:{}", pkg.meta.id, key);
                                let _ = store.set_user_context(&ctx_key, &value, "pattern");
                            }
                        }

                        // Execute chain steps if present
                        if !pkg.chain.is_empty() {
                            if let Ok(chain_steps) = pkg_mgr.load_chain(pkg) {
                                let mut prev_output = result.output.clone();
                                for (step_idx, (chain_pkg, chain_js)) in
                                    chain_steps.iter().enumerate()
                                {
                                    let chain_config = pkg_mgr
                                        .get_config_with_defaults(&chain_pkg.meta.id)
                                        .unwrap_or_default();
                                    let chain_context = chain_pkg.build_context(
                                        &chain_config,
                                        serde_json::json!({}),
                                        Some(&prev_output),
                                        &shared_ctx,
                                    );
                                    let chain_wrapped =
                                        format!("const ctx = JSON.parse(__context__);\n{chain_js}");
                                    match sandbox.execute(&chain_wrapped, chain_context).await {
                                        Ok(chain_result) if chain_result.success => {
                                            // Execute captured skill calls (Telegram, Http, etc.)
                                            if !chain_result.skill_calls.is_empty() {
                                                let preresolved =
                                                    crate::skill_executor::resolve_storage_calls(
                                                        &chain_result.skill_calls,
                                                        &store,
                                                        Some(&chain_pkg.meta.id),
                                                    );
                                                let mut checker = kittypaw_core::capability::CapabilityChecker::from_package_permissions(&chain_pkg.permissions);
                                                // Use per-step model override from chain definition
                                                let chain_model = pkg
                                                    .chain
                                                    .get(step_idx)
                                                    .and_then(|s| s.model.as_deref())
                                                    .or(chain_pkg.model.as_deref());
                                                let _ = crate::skill_executor::execute_skill_calls(
                                                    &chain_result.skill_calls,
                                                    config,
                                                    preresolved,
                                                    Some(&chain_pkg.meta.id),
                                                    Some(&mut checker),
                                                    chain_model,
                                                )
                                                .await;
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
                                            tracing::error!(
                                                "Chain step '{}' execution error: {e}",
                                                chain_pkg.meta.id
                                            );
                                            break;
                                        }
                                    }
                                }
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
                        let pkg_finished_at = chrono::Utc::now();
                        let duration_ms = (pkg_finished_at - pkg_started_at).num_milliseconds();
                        let failures = get_failure_count(db_path, &pkg.meta.id) + 1;
                        let delay_secs = 60u64 * (1u64 << failures.min(10));
                        notifier.notify_retry(&pkg.meta.id, failures, delay_secs);
                        handle_execution_failure(
                            &store,
                            db_path,
                            &pkg.meta.id,
                            &pkg.meta.name,
                            pkg_started_at,
                            &error_msg,
                            Some(&pkg_input_params),
                            false,
                        );
                        append_execution_log(
                            &data_dir,
                            &pkg.meta.id,
                            false,
                            duration_ms,
                            &error_msg,
                        );
                    }
                    Err(e) => {
                        tracing::error!("Scheduled package '{}' execution error: {e}", pkg.meta.id);
                        let pkg_finished_at = chrono::Utc::now();
                        let duration_ms = (pkg_finished_at - pkg_started_at).num_milliseconds();
                        let err_str = e.to_string();
                        let failures = get_failure_count(db_path, &pkg.meta.id) + 1;
                        let delay_secs = 60u64 * (1u64 << failures.min(10));
                        notifier.notify_retry(&pkg.meta.id, failures, delay_secs);
                        handle_execution_failure(
                            &store,
                            db_path,
                            &pkg.meta.id,
                            &pkg.meta.name,
                            pkg_started_at,
                            &err_str,
                            Some(&pkg_input_params),
                            false,
                        );
                        append_execution_log(&data_dir, &pkg.meta.id, false, duration_ms, &err_str);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kittypaw_core::package::{PackageMeta, PackagePermissions as PkgPerms};
    use kittypaw_core::skill::{Skill, SkillPermissions, SkillTrigger};

    fn make_schedule_skill(cron_expr: &str) -> Skill {
        Skill {
            name: "test-scheduled".into(),
            version: 1,
            description: "A test scheduled skill".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
            enabled: true,
            trigger: SkillTrigger {
                trigger_type: "schedule".into(),
                cron: Some(cron_expr.into()),
                natural: None,
                keyword: None,
            },
            permissions: SkillPermissions {
                primitives: vec![],
                allowed_hosts: vec![],
            },
        }
    }

    #[test]
    fn test_validate_cron_valid() {
        // Every hour
        assert!(validate_cron("0 0 * * * *").is_ok());
        // Every day at midnight
        assert!(validate_cron("0 0 0 * * *").is_ok());
    }

    #[test]
    fn test_validate_cron_invalid() {
        assert!(validate_cron("not a cron expression").is_err());
        assert!(validate_cron("").is_err());
    }

    #[test]
    fn test_validate_cron_too_frequent() {
        // Every second (6-part cron: sec min hour dom month dow)
        let result = validate_cron("* * * * * *");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.contains("too short"),
            "Expected 'too short' error, got: {err}"
        );
    }

    #[test]
    fn test_is_due_basic() {
        let skill = make_schedule_skill("0 0 * * * *"); // every hour

        // Last run was 2 hours ago — should be due
        let two_hours_ago = Utc::now() - chrono::Duration::hours(2);
        assert!(
            is_due(&skill, Some(two_hours_ago)),
            "Skill should be due when last run was 2 hours ago"
        );

        // Last run was 30 seconds ago — should NOT be due (next occurrence is ~1 hour away)
        let just_now = Utc::now() - chrono::Duration::seconds(30);
        assert!(
            !is_due(&skill, Some(just_now)),
            "Skill should not be due when last run was 30 seconds ago"
        );
    }

    #[test]
    fn test_is_due_disabled_skill() {
        let mut skill = make_schedule_skill("0 0 * * * *");
        skill.enabled = false;
        let two_hours_ago = Utc::now() - chrono::Duration::hours(2);
        assert!(!is_due(&skill, Some(two_hours_ago)));
    }

    #[test]
    fn test_is_due_non_schedule_trigger() {
        let mut skill = make_schedule_skill("0 0 * * * *");
        skill.trigger.trigger_type = "message".into();
        let two_hours_ago = Utc::now() - chrono::Duration::hours(2);
        assert!(!is_due(&skill, Some(two_hours_ago)));
    }

    #[test]
    fn test_schedule_db_persistence() {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let mut p = std::env::temp_dir();
        p.push(format!(
            "kittypaw_sched_test_{}_{}.db",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let db_path = p.to_str().unwrap();

        init_schedule_db(db_path).unwrap();

        // No last run initially
        assert!(get_last_run(db_path, "my-skill").is_none());

        // Set last run
        let now = Utc::now();
        set_last_run(db_path, "my-skill", now).unwrap();
        let loaded = get_last_run(db_path, "my-skill").unwrap();
        assert!((loaded - now).num_seconds().abs() < 2);

        // Failure count
        assert_eq!(get_failure_count(db_path, "my-skill"), 0);
        increment_failure_count(db_path, "my-skill").unwrap();
        assert_eq!(get_failure_count(db_path, "my-skill"), 1);
        increment_failure_count(db_path, "my-skill").unwrap();
        assert_eq!(get_failure_count(db_path, "my-skill"), 2);
        reset_failure_count(db_path, "my-skill").unwrap();
        assert_eq!(get_failure_count(db_path, "my-skill"), 0);

        let _ = std::fs::remove_file(&p);
    }

    fn make_schedule_package(cron_expr: &str) -> SkillPackage {
        SkillPackage {
            meta: PackageMeta {
                id: "test-pkg".into(),
                name: "Test Package".into(),
                version: "1.0.0".into(),
                description: "A test package".into(),
                author: "tester".into(),
                category: "test".into(),
                tags: vec![],
            },
            config_schema: vec![],
            permissions: PkgPerms {
                primitives: vec![],
                allowed_hosts: vec![],
            },
            trigger: Some(SkillTrigger {
                trigger_type: "schedule".into(),
                cron: Some(cron_expr.into()),
                natural: None,
                keyword: None,
            }),
            chain: vec![],
            model: None,
        }
    }

    #[test]
    fn test_is_package_due_basic() {
        let pkg = make_schedule_package("0 0 * * * *"); // every hour
        let two_hours_ago = Utc::now() - chrono::Duration::hours(2);
        assert!(is_package_due(&pkg, Some(two_hours_ago)));

        let just_now = Utc::now() - chrono::Duration::seconds(30);
        assert!(!is_package_due(&pkg, Some(just_now)));
    }

    #[test]
    fn test_is_package_due_no_trigger() {
        let mut pkg = make_schedule_package("0 0 * * * *");
        pkg.trigger = None;
        let two_hours_ago = Utc::now() - chrono::Duration::hours(2);
        assert!(!is_package_due(&pkg, Some(two_hours_ago)));
    }

    #[test]
    fn test_is_package_due_message_trigger() {
        let mut pkg = make_schedule_package("0 0 * * * *");
        pkg.trigger.as_mut().unwrap().trigger_type = "message".into();
        let two_hours_ago = Utc::now() - chrono::Duration::hours(2);
        assert!(!is_package_due(&pkg, Some(two_hours_ago)));
    }
}
