use chrono::{DateTime, Utc};
use cron::Schedule as CronSchedule;
use kittypaw_core::package::SkillPackage;
use kittypaw_core::skill::Skill;
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

/// Check whether a cron expression has fired since the last run.
pub fn is_cron_due(cron_expr: &str, last_run: Option<DateTime<Utc>>) -> bool {
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
