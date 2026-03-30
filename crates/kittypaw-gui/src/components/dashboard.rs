use crate::state::AppState;
use dioxus::prelude::*;
use kittypaw_core::package_manager::PackageManager;
use kittypaw_store::{ExecutionRecord, ExecutionStats};

const SECTION_TITLE_STYLE: &str = "font-size: 13px; font-weight: 600; text-transform: uppercase; letter-spacing: 0.8px; color: #78716C; margin-bottom: 12px;";

// ── Time helpers ─────────────────────────────────────────────────────────────

/// Returns (greeting, korean_date_string) derived from wall-clock seconds.
/// Uses only std — no chrono dependency needed.
fn greeting_and_date() -> (&'static str, String) {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    // Shift to KST (UTC+9)
    let hour_kst = ((secs + 9 * 3600) / 3600) % 24;

    let greeting = if hour_kst < 12 {
        "Good morning"
    } else if hour_kst < 18 {
        "Good afternoon"
    } else {
        "Good evening"
    };

    // Compute calendar date in KST
    let kst_days = (secs + 9 * 3600) / 86400;
    let (year, month, day, weekday) = days_to_ymd_weekday(kst_days);

    const WEEKDAYS: [&str; 7] = [
        "월요일",
        "화요일",
        "수요일",
        "목요일",
        "금요일",
        "토요일",
        "일요일",
    ];
    let weekday_ko = WEEKDAYS[weekday as usize % 7];

    let date_str = format!("{}년 {}월 {}일 {}", year, month, day, weekday_ko);
    (greeting, date_str)
}

/// Convert days since Unix epoch to (year, month, day, weekday).
/// weekday: 0=Mon … 6=Sun  (1970-01-01 was Thursday → offset 3).
/// Algorithm: http://howardhinnant.github.io/date_algorithms.html
fn days_to_ymd_weekday(days: u64) -> (u64, u64, u64, u64) {
    let weekday = (days + 3) % 7; // 1970-01-01 is Thursday

    let z = days + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    (y, m, d, weekday)
}

/// Extract HH:MM from a started_at string like "2024-06-01 10:30:00"
fn hhmm_from_started_at(started_at: &str) -> String {
    // Expected format: "YYYY-MM-DD HH:MM:SS" or ISO "YYYY-MM-DDTHH:MM:SS"
    let time_part = if let Some(t) = started_at.find('T') {
        &started_at[t + 1..]
    } else if let Some(pos) = started_at.find(' ') {
        &started_at[pos + 1..]
    } else {
        started_at
    };
    // Take first 5 chars: "HH:MM"
    if time_part.len() >= 5 {
        time_part[..5].to_string()
    } else {
        time_part.to_string()
    }
}

// ── Sub-components ───────────────────────────────────────────────────────────

#[component]
fn StatCard(label: &'static str, value: String, accent: bool) -> Element {
    let card_bg = if accent { "#F0FDF4" } else { "#FFFFFF" };
    let card_border = if accent { "#86EFAC" } else { "#E7E5E4" };
    let value_color = if accent { "#166534" } else { "#1C1917" };

    rsx! {
        div {
            style: "background: {card_bg}; border: 1px solid {card_border}; border-radius: 10px; padding: 18px 20px;",
            div {
                style: "font-size: 12px; color: #78716C; text-transform: uppercase; letter-spacing: 0.5px; margin-bottom: 6px;",
                "{label}"
            }
            div {
                style: "font-family: 'Geist Mono', 'SF Mono', monospace; font-size: 28px; font-weight: 500; color: {value_color}; line-height: 1;",
                "{value}"
            }
        }
    }
}

// ── Main Dashboard component ─────────────────────────────────────────────────

#[component]
pub fn Dashboard() -> Element {
    let app_state = use_context::<AppState>();
    let (greeting, date_str) = greeting_and_date();

    let mut stats = use_signal(|| ExecutionStats {
        total_runs: 0,
        successful: 0,
        failed: 0,
        auto_retries: 0,
    });
    let mut recent = use_signal::<Vec<ExecutionRecord>>(Vec::new);
    let mut installed_count = use_signal(|| 0u32);
    // (skill_id, skill_name, suggested_cron)
    let mut schedule_suggestions = use_signal::<Vec<(String, String)>>(Vec::new);

    let packages_dir = app_state.packages_dir.clone();
    let store_arc = app_state.store.clone();
    use_effect(move || {
        if let Ok(store) = store_arc.lock() {
            if let Ok(s) = store.today_stats() {
                stats.set(s);
            }
            if let Ok(r) = store.recent_executions(10) {
                // Collect unique skill IDs from recent executions for time pattern analysis
                let skill_ids: Vec<(String, String)> = {
                    let mut seen = std::collections::HashSet::new();
                    r.iter()
                        .filter(|e| seen.insert(e.skill_id.clone()))
                        .map(|e| (e.skill_id.clone(), e.skill_name.clone()))
                        .collect()
                };

                // Detect time patterns for each skill, skip already dismissed
                let mut suggestions = Vec::new();
                for (skill_id, skill_name) in &skill_ids {
                    let dismiss_key = format!("suggest_dismissed:{}", skill_id);
                    if store
                        .get_user_context(&dismiss_key)
                        .ok()
                        .flatten()
                        .is_some()
                    {
                        continue;
                    }
                    // Only suggest if no schedule exists (no schedule trigger in recent)
                    if let Ok(Some(_cron)) = store.detect_time_pattern(skill_id) {
                        suggestions.push((skill_id.clone(), skill_name.clone()));
                    }
                }
                schedule_suggestions.set(suggestions);
                recent.set(r);
            }
        }
        let mgr = PackageManager::new(packages_dir.clone());
        if let Ok(pkgs) = mgr.list_installed() {
            installed_count.set(pkgs.len() as u32);
        }
    });

    let active_skills_val = installed_count.read().to_string();
    let today_runs_val = stats.read().total_runs.to_string();
    let silent_opts_val = stats.read().auto_retries.to_string();
    let quiet_count = stats.read().auto_retries;
    let activity = recent.read();
    let suggestions = schedule_suggestions.read();

    rsx! {
        div {
            style: "flex: 1; overflow-y: auto; padding: 20px 28px; background: #F5F3F0; font-family: 'Inter', -apple-system, sans-serif; scrollbar-width: none;",

            // ── Header ────────────────────────────────────────────────────
            div {
                style: "display: flex; justify-content: space-between; align-items: baseline; margin-bottom: 16px;",
                h1 {
                    style: "font-family: 'Fraunces', Georgia, serif; font-size: 24px; font-weight: 600; color: #1C1917; margin: 0;",
                    "{greeting}"
                }
                span {
                    style: "font-size: 13px; color: #78716C;",
                    "{date_str}"
                }
            }

            // ── Stat Cards ────────────────────────────────────────────────
            div {
                style: "display: grid; grid-template-columns: repeat(3, 1fr); gap: 12px; margin-bottom: 18px;",
                StatCard { label: "Active Skills", value: active_skills_val, accent: false }
                StatCard { label: "Today's Runs", value: today_runs_val, accent: false }
                StatCard { label: "Silent Optimizations", value: silent_opts_val, accent: true }
            }

            // ── Recent Activity section title ─────────────────────────────
            div {
                style: "{SECTION_TITLE_STYLE}",
                "Recent Activity"
            }

            // ── Activity log ──────────────────────────────────────────────
            if activity.is_empty() {
                div {
                    style: "background: #FFFFFF; border: 1px solid #E7E5E4; border-radius: 10px; padding: 32px 20px; text-align: center; color: #A8A29E; font-size: 13px; margin-bottom: 16px;",
                    "아직 실행 기록이 없습니다. 스킬을 설치하고 실행해보세요."
                }
            } else {
                div {
                    style: "margin-bottom: 16px;",
                    for (i, entry) in activity.iter().enumerate() {
                        {
                            let is_last = i == activity.len() - 1;
                            let entry_border = if is_last { "none" } else { "1px solid #E7E5E4" };
                            let is_quiet = entry.retry_count > 0;
                            let skill_color = if is_quiet { "#166534" } else { "#1C1917" };
                            let time = hhmm_from_started_at(&entry.started_at);
                            let skill = entry.skill_name.clone();
                            let description = entry.result_summary.clone();
                            rsx! {
                                div {
                                    key: "{entry.id}",
                                    style: "display: flex; align-items: baseline; gap: 12px; padding: 8px 0; font-size: 12px; border-bottom: {entry_border};",
                                    span {
                                        style: "font-family: 'Geist Mono', 'SF Mono', monospace; font-size: 11px; color: #A8A29E; flex-shrink: 0; width: 50px;",
                                        "{time}"
                                    }
                                    span {
                                        style: "color: {skill_color};",
                                        "{skill} "
                                        span {
                                            style: "color: #78716C;",
                                            "— {description}"
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // ── Schedule suggestions ──────────────────────────────────────
            if !suggestions.is_empty() {
                div {
                    style: "{SECTION_TITLE_STYLE}",
                    "Schedule Suggestions"
                }
                for (skill_id, skill_name) in suggestions.iter() {
                    {
                        let skill_id_accept = skill_id.clone();
                        let skill_id_dismiss = skill_id.clone();
                        let app_state_dismiss = app_state.clone();
                        let mut schedule_suggestions_dismiss = schedule_suggestions.clone();
                        rsx! {
                            div {
                                key: "suggest-{skill_id}",
                                style: "background: #FFFBEB; border: 1px solid #FDE68A; border-radius: 10px; padding: 14px 20px; display: flex; align-items: center; gap: 10px; font-size: 13px; color: #92400E; margin-bottom: 10px;",
                                span { style: "font-size: 16px;", "💡" }
                                span {
                                    style: "flex: 1;",
                                    "{skill_name}을(를) 매일 자동 실행할까요?"
                                }
                                button {
                                    style: "background: #F59E0B; border: none; border-radius: 6px; padding: 4px 10px; font-size: 12px; color: #FFFFFF; cursor: pointer; margin-right: 6px;",
                                    onclick: move |_| {
                                        tracing::info!("Schedule suggestion accepted for skill: {}", skill_id_accept);
                                    },
                                    "수락"
                                }
                                button {
                                    style: "background: transparent; border: 1px solid #D97706; border-radius: 6px; padding: 4px 10px; font-size: 12px; color: #92400E; cursor: pointer;",
                                    onclick: move |_| {
                                        let dismiss_key = format!("suggest_dismissed:{}", skill_id_dismiss);
                                        if let Ok(store) = app_state_dismiss.store.lock() {
                                            let _ = store.set_user_context(&dismiss_key, "1", "user");
                                        }
                                        let id_to_remove = skill_id_dismiss.clone();
                                        schedule_suggestions_dismiss.write().retain(|(id, _)| *id != id_to_remove);
                                    },
                                    "닫기"
                                }
                            }
                        }
                    }
                }
            }

            // ── Quiet improvements banner ─────────────────────────────────
            div {
                style: "background: #F0FDF4; border: 1px solid #D1FAE5; border-radius: 10px; padding: 14px 20px; display: flex; align-items: center; gap: 10px; font-size: 13px; color: #166534; cursor: pointer; margin-bottom: 24px;",
                span { style: "font-size: 16px;", "✨" }
                span {
                    "오늘 "
                    span {
                        style: "font-family: 'Geist Mono', 'SF Mono', monospace; font-weight: 600;",
                        "{quiet_count}"
                    }
                    "번의 조용한 개선이 적용됨"
                }
                span {
                    style: "margin-left: auto; font-size: 11px; color: #4ADE80; text-decoration: underline;",
                    "자세히 보기 →"
                }
            }
        }
    }
}
