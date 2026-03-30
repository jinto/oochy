use dioxus::prelude::*;

// ── Mock data types ──────────────────────────────────────────────────────────

#[derive(Clone, PartialEq)]
enum SkillStatus {
    Running,
    Scheduled,
    Error,
}

#[derive(Clone)]
struct SkillRow {
    status: SkillStatus,
    name: &'static str,
    last_result: &'static str,
    schedule: &'static str,
    tag: &'static str,
}

#[derive(Clone)]
struct ActivityEntry {
    time: &'static str,
    skill: &'static str,
    description: &'static str,
    is_quiet: bool,
}

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

    let month_ko = match month {
        1 => "1월",
        2 => "2월",
        3 => "3월",
        4 => "4월",
        5 => "5월",
        6 => "6월",
        7 => "7월",
        8 => "8월",
        9 => "9월",
        10 => "10월",
        11 => "11월",
        _ => "12월",
    };
    let weekday_ko = match weekday {
        0 => "월요일",
        1 => "화요일",
        2 => "수요일",
        3 => "목요일",
        4 => "금요일",
        5 => "토요일",
        _ => "일요일",
    };

    let date_str = format!("{}년 {} {}일 {}", year, month_ko, day, weekday_ko);
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

// ── Mock data ────────────────────────────────────────────────────────────────

fn mock_skills() -> Vec<SkillRow> {
    vec![
        SkillRow {
            status: SkillStatus::Running,
            name: "weather-briefing",
            last_result: "서울 8°C, 맑음 — 30분 전",
            schedule: "매일 07:00",
            tag: "Active",
        },
        SkillRow {
            status: SkillStatus::Running,
            name: "url-monitor",
            last_result: "변경 없음 — example.jp — 1시간 전",
            schedule: "매 2시간",
            tag: "Active",
        },
        SkillRow {
            status: SkillStatus::Running,
            name: "rss-digest",
            last_result: "새 글 3건 요약 완료 — 2시간 전",
            schedule: "매일 09:00",
            tag: "Active",
        },
        SkillRow {
            status: SkillStatus::Scheduled,
            name: "macro-economy-report",
            last_result: "다음 실행 대기 중",
            schedule: "매주 월 08:00",
            tag: "Scheduled",
        },
        SkillRow {
            status: SkillStatus::Error,
            name: "reminder",
            last_result: "API 타임아웃 — 자동 재시도 1/3",
            schedule: "트리거 기반",
            tag: "Retrying",
        },
    ]
}

fn mock_activity() -> Vec<ActivityEntry> {
    vec![
        ActivityEntry {
            time: "09:12",
            skill: "rss-digest",
            description: "Hacker News, TechCrunch, Ars Technica에서 3건 요약",
            is_quiet: false,
        },
        ActivityEntry {
            time: "08:15",
            skill: "url-monitor",
            description: "example.jp 변경 없음",
            is_quiet: false,
        },
        ActivityEntry {
            time: "07:00",
            skill: "weather-briefing",
            description: "서울 8°C 맑음, 오사카 12°C 흐림",
            is_quiet: false,
        },
        ActivityEntry {
            time: "07:00",
            skill: "🔇 weather-briefing 위치 자동 적용",
            description: "\"서울\" 3회 입력 → 기본값으로 설정됨",
            is_quiet: true,
        },
        ActivityEntry {
            time: "06:30",
            skill: "reminder",
            description: "API 타임아웃, 자동 재시도 시작",
            is_quiet: false,
        },
    ]
}

// ── Sub-components ───────────────────────────────────────────────────────────

#[component]
fn StatCard(label: &'static str, value: &'static str, accent: bool) -> Element {
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
    let (greeting, date_str) = greeting_and_date();
    let skills = mock_skills();
    let activity = mock_activity();

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
                StatCard { label: "Active Skills", value: "5", accent: false }
                StatCard { label: "Today's Runs", value: "12", accent: false }
                StatCard { label: "Silent Optimizations", value: "3", accent: true }
            }

            // ── Skills section title ───────────────────────────────────────
            div {
                style: "font-size: 13px; font-weight: 600; text-transform: uppercase; letter-spacing: 0.8px; color: #78716C; margin-bottom: 12px;",
                "Skills"
            }

            // ── Skills list ───────────────────────────────────────────────
            div {
                style: "background: #FFFFFF; border: 1px solid #E7E5E4; border-radius: 10px; overflow: hidden; margin-bottom: 16px;",
                for (i, row) in skills.iter().enumerate() {
                    {
                        let is_last = i == skills.len() - 1;
                        let row_border = if is_last { "none" } else { "1px solid #E7E5E4" };
                        let dot_color = match row.status {
                            SkillStatus::Running   => "#86EFAC",
                            SkillStatus::Scheduled => "#A8A29E",
                            SkillStatus::Error     => "#FCA5A5",
                        };
                        let dot_shadow = match row.status {
                            SkillStatus::Running => "0 0 6px rgba(134,239,172,0.5)",
                            _                    => "none",
                        };
                        let (tag_bg, tag_color) = match row.status {
                            SkillStatus::Running   => ("#F0FDF4", "#166534"),
                            SkillStatus::Scheduled => ("#F5F5F4", "#78716C"),
                            SkillStatus::Error     => ("#FEF2F2", "#991B1B"),
                        };
                        let name        = row.name;
                        let last_result = row.last_result;
                        let schedule    = row.schedule;
                        let tag         = row.tag;
                        rsx! {
                            div {
                                key: "{i}",
                                style: "display: grid; grid-template-columns: 8px 1fr 140px 80px; align-items: center; gap: 14px; padding: 10px 16px; border-bottom: {row_border}; font-size: 13px;",
                                // Status dot
                                div {
                                    style: "width: 8px; height: 8px; border-radius: 9999px; background: {dot_color}; box-shadow: {dot_shadow};",
                                }
                                // Name + last result
                                div {
                                    div {
                                        style: "font-weight: 500; color: #1C1917;",
                                        "{name}"
                                    }
                                    div {
                                        style: "font-size: 12px; color: #78716C; margin-top: 2px;",
                                        "{last_result}"
                                    }
                                }
                                // Schedule
                                div {
                                    style: "font-family: 'Geist Mono', 'SF Mono', monospace; font-size: 11px; color: #78716C;",
                                    "{schedule}"
                                }
                                // Status tag
                                div {
                                    style: "font-size: 11px; padding: 2px 8px; border-radius: 9999px; font-weight: 500; background: {tag_bg}; color: {tag_color}; display: inline-block;",
                                    "{tag}"
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
                    "이번 주 "
                    span {
                        style: "font-family: 'Geist Mono', 'SF Mono', monospace; font-weight: 600;",
                        "8"
                    }
                    "번의 조용한 개선이 적용됨"
                }
                span {
                    style: "margin-left: auto; font-size: 11px; color: #4ADE80; text-decoration: underline;",
                    "자세히 보기 →"
                }
            }

            // ── Recent Activity section title ─────────────────────────────
            div {
                style: "font-size: 13px; font-weight: 600; text-transform: uppercase; letter-spacing: 0.8px; color: #78716C; margin-bottom: 12px;",
                "Recent Activity"
            }

            // ── Activity log ──────────────────────────────────────────────
            div {
                for (i, entry) in activity.iter().enumerate() {
                    {
                        let is_last = i == activity.len() - 1;
                        let entry_border = if is_last { "none" } else { "1px solid #E7E5E4" };
                        let skill_color = if entry.is_quiet { "#166534" } else { "#1C1917" };
                        let time        = entry.time;
                        let skill       = entry.skill;
                        let description = entry.description;
                        rsx! {
                            div {
                                key: "{i}",
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
    }
}
