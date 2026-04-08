mod auto_fix;
mod cron;
mod execution;
mod notification;
mod persistence;

// Public re-exports to maintain backward-compatible API
pub use cron::{is_due, is_package_due, validate_cron};
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

        // --- Run scheduled skills ---
        if let Ok(skills) = kittypaw_core::skill::load_all_skills() {
            for (skill, js_code) in &skills {
                if skill.trigger.trigger_type != "schedule" || !skill.enabled {
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
