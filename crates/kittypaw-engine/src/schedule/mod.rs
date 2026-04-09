mod auto_fix;
mod cron;
mod execution;
mod notification;
mod persistence;

// Public re-exports to maintain backward-compatible API
pub use cron::{is_due, is_once_due, is_package_due, validate_cron};
pub use execution::{
    append_execution_log, execute_chain_steps, execute_scheduled_package, execute_scheduled_skill,
    handle_execution_failure, handle_run_failure, handle_run_success, prepare_package_context,
};
pub use notification::NotificationSender;
pub use persistence::{
    get_failure_count, get_last_run, increment_failure_count, init_schedule_db,
    reset_failure_count, set_backoff_delay, set_last_run,
};

pub async fn run_schedule_loop(
    config: &kittypaw_core::config::Config,
    sandbox: &kittypaw_sandbox::sandbox::Sandbox,
    db_path: &str,
) {
    let data_dir = std::path::Path::new(db_path)
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .to_path_buf();

    // Open the store once and wrap in Arc<Mutex> for shared async access.
    // Store::open runs all migrations (including schedule table creation).
    let store = match kittypaw_store::Store::open(db_path) {
        Ok(s) => std::sync::Arc::new(tokio::sync::Mutex::new(s)),
        Err(e) => {
            tracing::error!("Failed to open store for schedule loop: {e}");
            return;
        }
    };

    let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
    loop {
        interval.tick().await;

        // Housekeeping — lock the store briefly for sync cleanup, then drop before next await.
        // rusqlite::Connection is !Sync, so the MutexGuard must be dropped before any await.
        let (shared_ctx, pkg_contexts) = {
            let s = store.lock().await;
            let _ = s.cleanup_old_executions(30);
            let _ = s.cleanup_old_turns(30);
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
            let packages_dir = data_dir.join("packages");
            let shared_ctx = s.list_shared_context().unwrap_or_default();
            let pkg_contexts: Vec<_> = if let Ok(packages) =
                kittypaw_core::package_manager::load_all_packages(&packages_dir)
            {
                let pkg_mgr =
                    kittypaw_core::package_manager::PackageManager::new(packages_dir.clone());
                packages
                    .into_iter()
                    .filter(|(pkg, _)| {
                        let last_run = s.get_last_run(&pkg.meta.id);
                        is_package_due(pkg, last_run)
                    })
                    .map(|(pkg, js_code)| {
                        let (context, config_values, input_params) =
                            prepare_package_context(&pkg, &*s, &pkg_mgr, &shared_ctx);
                        (pkg, js_code, context, config_values, input_params)
                    })
                    .collect()
            } else {
                Vec::new()
            };
            (shared_ctx, pkg_contexts)
        };
        // store lock is now released — safe to await

        let notifier = NotificationSender::new(config);

        // --- Run scheduled and once skills ---
        if let Ok(skills) = kittypaw_core::skill::load_all_skills() {
            for (skill, js_code) in &skills {
                let trigger_type = skill.trigger.trigger_type.as_str();
                if (trigger_type != "schedule" && trigger_type != "once") || !skill.enabled {
                    continue;
                }
                let last_run = {
                    let s = store.lock().await;
                    s.get_last_run(&skill.name)
                };
                if !is_due(skill, last_run) {
                    continue;
                }
                execute_scheduled_skill(
                    skill, js_code, config, sandbox, &notifier, &data_dir, db_path, &store,
                )
                .await;
                // Once skills run exactly once — delete after execution.
                // handle_run_success already set last_run_at as a safety net in case deletion fails.
                if trigger_type == "once" {
                    if let Err(e) = kittypaw_core::skill::delete_skill(&skill.name) {
                        tracing::warn!(
                            name = skill.name.as_str(),
                            "Failed to delete once skill after execution: {e}"
                        );
                    }
                }
            }
        }

        // --- Reflection tick ---
        if config.reflection.enabled
            && config.autonomy_level != kittypaw_core::config::AutonomyLevel::Readonly
        {
            let last_reflection = {
                let s = store.lock().await;
                s.get_last_run("_reflection_")
            };
            if cron::is_cron_due(&config.reflection.cron, last_reflection) {
                tracing::info!("Reflection cron due — running analysis");
                run_reflection_tick(config, &store, &notifier).await;
                let s = store.lock().await;
                let _ = s.set_last_run("_reflection_", chrono::Utc::now());

                // Weekly report: send on configured day of week
                let today = {
                    use chrono::Datelike;
                    chrono::Utc::now()
                        .date_naive()
                        .weekday()
                        .num_days_from_sunday()
                };
                if today == config.reflection.weekly_report_day {
                    let prefs = s.list_topic_preferences(10).unwrap_or_default();
                    let parsed: Vec<(String, u32)> = prefs
                        .into_iter()
                        .filter_map(|(topic, json_str)| {
                            let v: serde_json::Value = serde_json::from_str(&json_str).ok()?;
                            let count = v["count"].as_u64()? as u32;
                            Some((topic, count))
                        })
                        .collect();
                    if !parsed.is_empty() {
                        let report = crate::reflection::build_weekly_report(&parsed);
                        notifier.notify_weekly_report(&report);
                        tracing::info!("Weekly preference report sent");
                    }
                }
            }
        }

        // --- Run scheduled packages ---
        if !pkg_contexts.is_empty() {
            let packages_dir = data_dir.join("packages");
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
                    &store,
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

/// Execute one reflection tick using the shared store.
/// Structured as lock→read→unlock→await→lock→write→unlock to keep Store !Send-safe.
async fn run_reflection_tick(
    config: &kittypaw_core::config::Config,
    store: &std::sync::Arc<tokio::sync::Mutex<kittypaw_store::Store>>,
    notifier: &NotificationSender,
) {
    // Build LLM provider (same pattern as auto_fix)
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
            tier: None,
        };
        kittypaw_llm::registry::LlmRegistry::from_configs(&[legacy])
    } else {
        tracing::warn!("Reflection: no LLM provider configured");
        return;
    };
    let provider = match registry.default_provider() {
        Some(p) => p,
        None => {
            tracing::warn!("Reflection: no default provider");
            return;
        }
    };

    // Phase 1: Read (lock → read → unlock)
    let input = {
        let s = store.lock().await;
        match crate::reflection::read_reflection_input(&*s, &config.reflection) {
            Ok(i) => i,
            Err(e) => {
                tracing::error!("Reflection read failed: {e}");
                return;
            }
        }
    };

    if input.messages.is_empty() {
        let s = store.lock().await;
        let _ = s.delete_expired_reflection(config.reflection.ttl_days);
        return;
    }

    // Phase 2: LLM call (no lock held)
    let (groups, topics) =
        match crate::reflection::call_llm_grouping(&*provider, &input, &config.reflection).await {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("Reflection LLM failed: {e}");
                return;
            }
        };

    // Phase 3: Write (lock → write → unlock)
    let s = store.lock().await;
    match crate::reflection::write_reflection_results(
        &*s,
        groups,
        topics,
        &input,
        &config.reflection,
    ) {
        Ok(result) => {
            for sg in &result.suggestions {
                tracing::info!(
                    "Reflection: suggested '{}' ({}x)",
                    sg.intent_label,
                    sg.count
                );
                notifier.notify_reflection_suggestion(&sg.intent_label, sg.count, &sg.intent_hash);
            }
            if result.swept > 0 {
                tracing::info!("Reflection: swept {} expired entries", result.swept);
            }
        }
        Err(e) => tracing::error!("Reflection write failed: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::temp_db_path;
    use chrono::Utc;
    use kittypaw_core::package::{PackageMeta, PackagePermissions as PkgPerms, SkillPackage};
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
                run_at: None,
            },
            permissions: SkillPermissions {
                primitives: vec![],
                allowed_hosts: vec![],
            },
            format: kittypaw_core::skill::SkillFormat::Native,
            model_tier: None,
        }
    }

    fn make_once_skill(run_at: &str) -> Skill {
        Skill {
            name: "test-once".into(),
            version: 1,
            description: "A one-shot skill".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
            enabled: true,
            trigger: SkillTrigger {
                trigger_type: "once".into(),
                cron: None,
                natural: Some("2m".into()),
                keyword: None,
                run_at: Some(run_at.into()),
            },
            permissions: SkillPermissions {
                primitives: vec![],
                allowed_hosts: vec![],
            },
            format: kittypaw_core::skill::SkillFormat::Native,
            model_tier: None,
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
    fn is_once_due_before_run_at_returns_false() {
        let future = (Utc::now() + chrono::Duration::minutes(5)).to_rfc3339();
        let skill = make_once_skill(&future);
        assert!(
            !is_once_due(&skill, None),
            "Should not be due before run_at"
        );
    }

    #[test]
    fn is_once_due_after_run_at_no_last_run_returns_true() {
        let past = (Utc::now() - chrono::Duration::minutes(1)).to_rfc3339();
        let skill = make_once_skill(&past);
        assert!(
            is_once_due(&skill, None),
            "Should be due after run_at with no last_run"
        );
    }

    #[test]
    fn is_once_due_with_last_run_returns_false() {
        let past = (Utc::now() - chrono::Duration::minutes(1)).to_rfc3339();
        let skill = make_once_skill(&past);
        // 이미 실행됐으면 재실행 금지
        assert!(
            !is_once_due(&skill, Some(Utc::now())),
            "Should not be due after last_run is set"
        );
    }

    #[test]
    fn is_due_includes_once_trigger() {
        let past = (Utc::now() - chrono::Duration::minutes(1)).to_rfc3339();
        let skill = make_once_skill(&past);
        assert!(
            is_due(&skill, None),
            "is_due should return true for due once skill"
        );
    }

    #[test]
    fn new_recurring_skill_not_immediately_due() {
        // 새로 생성된 스킬(last_run == None)은 즉시 실행되지 않아야 한다.
        // 버그: 이전엔 now-24h를 기준으로 삼아 크론이 과거에 발화했으면 즉시 due로 판정됐음.
        let skill = make_schedule_skill("0 0 * * * *"); // every hour
        assert!(
            !is_due(&skill, None),
            "New skill with no last_run must not fire immediately"
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
        let p = temp_db_path();
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
                run_at: None,
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
