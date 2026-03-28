use chrono::{DateTime, Utc};
use cron::Schedule as CronSchedule;
use kittypaw_core::package::SkillPackage;
use kittypaw_core::skill::Skill;
use rusqlite::{params, Connection};
use std::str::FromStr;

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

pub async fn run_schedule_loop(
    config: &kittypaw_core::config::Config,
    sandbox: &kittypaw_sandbox::sandbox::Sandbox,
    db_path: &str,
) {
    init_schedule_db(db_path).ok();
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
                let wrapped = format!("const ctx = JSON.parse(__context__);\n{js_code}");
                match sandbox.execute(&wrapped, context).await {
                    Ok(result) if result.success => {
                        if !result.skill_calls.is_empty() {
                            let preresolved = crate::skill_executor::resolve_storage_calls(
                                &result.skill_calls,
                                &store,
                                Some(&skill.name),
                            );
                            let _ = crate::skill_executor::execute_skill_calls(
                                &result.skill_calls,
                                config,
                                preresolved,
                                Some(&skill.name),
                            )
                            .await;
                        }
                        tracing::info!(
                            "Scheduled skill '{}' completed: {}",
                            skill.name,
                            result.output
                        );
                        set_last_run(db_path, &skill.name, Utc::now()).ok();
                        reset_failure_count(db_path, &skill.name).ok();
                    }
                    Ok(result) => {
                        tracing::warn!(
                            "Scheduled skill '{}' failed: {:?}",
                            skill.name,
                            result.error
                        );
                        increment_failure_count(db_path, &skill.name).ok();
                        let failures = get_failure_count(db_path, &skill.name);
                        if failures >= 3 {
                            tracing::warn!(
                                "Skill '{}' auto-disabled after {} consecutive failures",
                                skill.name,
                                failures
                            );
                            let _ = kittypaw_core::skill::disable_skill(&skill.name);
                        }
                    }
                    Err(e) => {
                        tracing::error!("Scheduled skill '{}' execution error: {e}", skill.name);
                        increment_failure_count(db_path, &skill.name).ok();
                    }
                }
            }
        }

        // --- Run scheduled packages ---
        let packages_dir = std::path::PathBuf::from(".kittypaw/packages");
        if let Ok(packages) = kittypaw_core::package_manager::load_all_packages(&packages_dir) {
            let pkg_mgr = kittypaw_core::package_manager::PackageManager::new(packages_dir.clone());
            for (pkg, js_code) in &packages {
                let last_run = get_last_run(db_path, &pkg.meta.id);
                if !is_package_due(pkg, last_run) {
                    continue;
                }

                tracing::info!("Running scheduled package: {}", pkg.meta.id);
                let config_values = pkg_mgr
                    .get_config_with_defaults(&pkg.meta.id)
                    .unwrap_or_default();
                let event_payload = serde_json::json!({
                    "event_type": "schedule",
                });
                let context = pkg.build_context(&config_values, event_payload);
                let wrapped = format!("const ctx = JSON.parse(__context__);\n{js_code}");
                match sandbox.execute(&wrapped, context).await {
                    Ok(result) if result.success => {
                        if !result.skill_calls.is_empty() {
                            let preresolved = crate::skill_executor::resolve_storage_calls(
                                &result.skill_calls,
                                &store,
                                Some(&pkg.meta.id),
                            );
                            let _ = crate::skill_executor::execute_skill_calls(
                                &result.skill_calls,
                                config,
                                preresolved,
                                Some(&pkg.meta.id),
                            )
                            .await;
                        }
                        tracing::info!(
                            "Scheduled package '{}' completed: {}",
                            pkg.meta.id,
                            result.output
                        );
                        set_last_run(db_path, &pkg.meta.id, Utc::now()).ok();
                        reset_failure_count(db_path, &pkg.meta.id).ok();
                    }
                    Ok(result) => {
                        tracing::warn!(
                            "Scheduled package '{}' failed: {:?}",
                            pkg.meta.id,
                            result.error
                        );
                        increment_failure_count(db_path, &pkg.meta.id).ok();
                    }
                    Err(e) => {
                        tracing::error!("Scheduled package '{}' execution error: {e}", pkg.meta.id);
                        increment_failure_count(db_path, &pkg.meta.id).ok();
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
