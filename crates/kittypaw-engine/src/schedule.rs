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

/// Check whether a cron expression has fired since the last run.
fn is_cron_due(cron_expr: &str, last_run: Option<DateTime<Utc>>) -> bool {
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

/// Check if a package is due to run based on its cron trigger.
pub fn is_package_due(pkg: &SkillPackage, last_run: Option<DateTime<Utc>>) -> bool {
    let trigger = match &pkg.trigger {
        Some(t) if t.trigger_type == "schedule" => t,
        _ => return false,
    };
    match &trigger.cron {
        Some(c) => is_cron_due(c, last_run),
        None => false,
    }
}

/// Check if a skill is due to run based on its cron schedule.
pub fn is_due(skill: &Skill, last_run: Option<DateTime<Utc>>) -> bool {
    if skill.trigger.trigger_type != "schedule" || !skill.enabled {
        return false;
    }
    match &skill.trigger.cron {
        Some(c) => is_cron_due(c, last_run),
        None => false,
    }
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
    fn new(config: &kittypaw_core::config::Config) -> Self {
        // Helper: resolve a value using secrets → env
        let resolve = |secret_key: &str, env_var: &str| -> Option<String> {
            kittypaw_core::secrets::get_secret("channels", secret_key)
                .ok()
                .flatten()
                .filter(|s| !s.is_empty())
                .or_else(|| std::env::var(env_var).ok().filter(|s| !s.is_empty()))
        };

        let tg_token = resolve("telegram_token", "KITTYPAW_TELEGRAM_TOKEN").or_else(|| {
            config
                .channels
                .iter()
                .find(|c| c.channel_type == kittypaw_core::config::ChannelType::Telegram)
                .map(|c| c.token.clone())
                .filter(|s| !s.is_empty())
        });
        let tg_chat_id = resolve("chat_id", "KITTYPAW_TELEGRAM_CHAT_ID");
        let telegram = match (tg_token, tg_chat_id) {
            (Some(token), Some(chat_id)) => Some((token, chat_id)),
            _ => None,
        };

        let slack_token = resolve("slack_token", "KITTYPAW_SLACK_TOKEN").or_else(|| {
            config
                .channels
                .iter()
                .find(|c| c.channel_type == kittypaw_core::config::ChannelType::Slack)
                .map(|c| c.token.clone())
                .filter(|s| !s.is_empty())
        });
        let slack_channel = resolve("slack_channel", "KITTYPAW_SLACK_CHANNEL");
        let slack = match (slack_token, slack_channel) {
            (Some(token), Some(channel)) => Some((token, channel)),
            _ => None,
        };

        Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
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

    fn notify_fix_applied(&self, name: &str, error: &str, fix_id: i64) {
        self.send(&format!(
            "🔧 *{}* 자동 수정 적용됨\n에러: {}\n`kittypaw fixes show {}`",
            name,
            error.chars().take(100).collect::<String>(),
            fix_id
        ));
    }

    fn notify_fix_pending(&self, name: &str, error: &str, fix_id: i64) {
        self.send(&format!(
            "🔧 *{}* 수정안 생성됨 (승인 대기)\n에러: {}\n`kittypaw fixes approve {}`",
            name,
            error.chars().take(100).collect::<String>(),
            fix_id
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
fn handle_run_failure(
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
fn handle_run_success(
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

/// Result of an auto-fix attempt.
struct AutoFixResult {
    fix_id: i64,
    applied: bool,
}

/// Attempt to auto-fix a broken skill using LLM code generation.
/// In Full mode: applies immediately. In Supervised mode: records for approval.
/// Returns Some(AutoFixResult) on success, None on failure or skip.
async fn attempt_auto_fix(
    skill_id: &str,
    error_msg: &str,
    config: &kittypaw_core::config::Config,
    sandbox: &kittypaw_sandbox::sandbox::Sandbox,
    db_path: &str,
) -> Option<AutoFixResult> {
    // Only in Full or Supervised autonomy mode
    let is_full = config.autonomy_level == kittypaw_core::config::AutonomyLevel::Full;
    if config.autonomy_level == kittypaw_core::config::AutonomyLevel::Readonly {
        return None;
    }

    // Load current skill code
    let (_skill, old_code) = match kittypaw_core::skill::load_skill(skill_id) {
        Ok(Some(pair)) => pair,
        _ => return None,
    };

    // Build a provider (handles legacy [llm] section + [[models]])
    let registry = if !config.models.is_empty() {
        let mut models = config.models.clone();
        if !config.llm.api_key.is_empty() {
            for model in &mut models {
                if model.api_key.is_empty()
                    && matches!(model.provider.as_str(), "claude" | "anthropic" | "openai")
                {
                    model.api_key = config.llm.api_key.clone();
                }
            }
        }
        kittypaw_llm::registry::LlmRegistry::from_configs(&models)
    } else if !config.llm.api_key.is_empty() {
        let legacy = kittypaw_core::config::ModelConfig {
            name: config.llm.provider.clone(),
            provider: config.llm.provider.clone(),
            model: config.llm.model.clone(),
            api_key: config.llm.api_key.clone(),
            max_tokens: config.llm.max_tokens,
            default: true,
            base_url: None,
            context_window: None,
        };
        kittypaw_llm::registry::LlmRegistry::from_configs(&[legacy])
    } else {
        kittypaw_llm::registry::LlmRegistry::new()
    };
    let provider = match registry.default_provider() {
        Some(p) => p,
        None => return None,
    };

    // Generate fix via teach_loop
    let fix_prompt = format!(
        "Fix this KittyPaw skill that failed with error: {}\n\nSkill name: {}\n\nCurrent code:\n```javascript\n{}\n```\n\nWrite the corrected code. Keep the same logic, only fix the error.",
        error_msg, skill_id, old_code
    );

    match crate::teach_loop::handle_teach(&fix_prompt, "auto-fix", &*provider, sandbox, config)
        .await
    {
        Ok(ref result @ crate::teach_loop::TeachResult::Generated { ref code, .. }) => {
            let new_code = code.clone();
            let error_short = error_msg.chars().take(500).collect::<String>();

            // Open store for recording the fix
            let store = match kittypaw_store::Store::open(db_path) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!("Cannot open store for fix recording: {e}");
                    // Still try to apply in full mode even without recording
                    if is_full {
                        let _ = crate::teach_loop::approve_skill(result);
                    }
                    return None;
                }
            };

            if is_full {
                // Full mode: apply immediately
                match crate::teach_loop::approve_skill(result) {
                    Ok(()) => {
                        let fix_id = store
                            .record_fix(skill_id, &error_short, &old_code, &new_code, true)
                            .unwrap_or(0);
                        tracing::info!("Auto-fix applied for skill '{skill_id}' (fix #{fix_id})");
                        Some(AutoFixResult {
                            fix_id,
                            applied: true,
                        })
                    }
                    Err(e) => {
                        tracing::warn!("Auto-fix save failed for '{skill_id}': {e}");
                        None
                    }
                }
            } else {
                // Supervised mode: record but don't apply
                let fix_id = store
                    .record_fix(skill_id, &error_short, &old_code, &new_code, false)
                    .unwrap_or(0);
                tracing::info!(
                    "Auto-fix generated for skill '{skill_id}' (fix #{fix_id}, pending approval)"
                );
                Some(AutoFixResult {
                    fix_id,
                    applied: false,
                })
            }
        }
        _ => {
            tracing::warn!("Auto-fix generation failed for '{skill_id}'");
            None
        }
    }
}

/// Execute a single scheduled skill and handle the result.
async fn execute_scheduled_skill(
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
    match sandbox.execute(&wrapped, context).await {
        Ok(result) if result.success => {
            // Open a fresh store after the await point (Store is !Sync)
            let store = match kittypaw_store::Store::open(db_path) {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!("Failed to open store for skill '{}': {e}", skill.name);
                    return;
                }
            };
            let call_error: Option<String> = if !result.skill_calls.is_empty() {
                let preresolved = crate::skill_executor::resolve_storage_calls(
                    &result.skill_calls,
                    &store,
                    Some(&skill.name),
                );
                let mut checker =
                    kittypaw_core::capability::CapabilityChecker::from_skill_permissions(
                        &skill.permissions,
                    );
                match crate::skill_executor::execute_skill_calls(
                    &result.skill_calls,
                    config,
                    preresolved,
                    Some(&skill.name),
                    Some(&mut checker),
                    None,
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
                    "Scheduled skill '{}' skill_call failed: {}",
                    skill.name,
                    err_msg
                );
                handle_run_failure(
                    &store,
                    notifier,
                    data_dir,
                    db_path,
                    &skill.name,
                    &skill.name,
                    started_at,
                    err_msg,
                    &input_params,
                    true,
                    None,
                );
            } else {
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
                            notifier.notify_fix_applied(&skill.name, &error_msg, fix_result.fix_id);
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
async fn execute_chain_steps(
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
                tracing::error!("Chain step '{}' execution error: {e}", chain_pkg.meta.id);
                break;
            }
        }
    }
}

/// Collect pattern-detected defaults and build config for a package.
fn prepare_package_context(
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
async fn execute_scheduled_package(
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

        // Housekeeping — open store briefly for cleanup, then drop before await.
        let (shared_ctx, pkg_contexts) = {
            let store = match kittypaw_store::Store::open(db_path) {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!("Failed to open store for schedule loop: {e}");
                    continue;
                }
            };
            let _ = store.cleanup_old_executions(30);
            let _ = store.cleanup_old_turns(30);
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

            // Pre-compute package contexts while we have the store
            let packages_dir = std::path::PathBuf::from(".kittypaw/packages");
            let shared_ctx = store.list_shared_context().unwrap_or_default();
            let pkg_contexts: Vec<_> = if let Ok(packages) =
                kittypaw_core::package_manager::load_all_packages(&packages_dir)
            {
                let pkg_mgr =
                    kittypaw_core::package_manager::PackageManager::new(packages_dir.clone());
                packages
                    .into_iter()
                    .filter(|(pkg, _)| {
                        let last_run = get_last_run(db_path, &pkg.meta.id);
                        is_package_due(pkg, last_run)
                    })
                    .map(|(pkg, js_code)| {
                        let (context, config_values, input_params) =
                            prepare_package_context(&pkg, &store, &pkg_mgr, &shared_ctx);
                        (pkg, js_code, context, config_values, input_params)
                    })
                    .collect()
            } else {
                Vec::new()
            };
            (shared_ctx, pkg_contexts)
        };
        // store is now dropped — safe to await

        let notifier = NotificationSender::new(config);

        // --- Run scheduled skills ---
        if let Ok(skills) = kittypaw_core::skill::load_all_skills() {
            for (skill, js_code) in &skills {
                if skill.trigger.trigger_type != "schedule" || !skill.enabled {
                    continue;
                }
                let last_run = get_last_run(db_path, &skill.name);
                if !is_due(skill, last_run) {
                    continue;
                }
                execute_scheduled_skill(
                    skill, js_code, config, sandbox, &notifier, &data_dir, db_path,
                )
                .await;
            }
        }

        // --- Run scheduled packages ---
        if !pkg_contexts.is_empty() {
            let packages_dir = std::path::PathBuf::from(".kittypaw/packages");
            let pkg_mgr = kittypaw_core::package_manager::PackageManager::new(packages_dir);
            for (pkg, js_code, context, config_values, input_params) in &pkg_contexts {
                tracing::info!("Running scheduled package: {}", pkg.meta.id);
                execute_scheduled_package(
                    pkg,
                    js_code,
                    config,
                    sandbox,
                    &notifier,
                    &data_dir,
                    db_path,
                    &pkg_mgr,
                    &shared_ctx,
                    context.clone(),
                    config_values,
                    input_params,
                )
                .await;
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
            format: kittypaw_core::skill::SkillFormat::Native,
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
                setup_notes: None,
            },
            config_schema: vec![],
            permissions: PkgPerms {
                primitives: vec![],
                allowed_hosts: vec![],
                allowed_mcp_servers: vec![],
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
