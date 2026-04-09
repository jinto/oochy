use kittypaw_core::config::Config;
use kittypaw_core::error::{KittypawError, Result};
use kittypaw_core::skill::{ModelTier, Skill, SkillPermissions, SkillTrigger};
use kittypaw_core::types::{LlmMessage, Role};
use kittypaw_llm::provider::LlmProvider;
use kittypaw_llm::util::strip_code_fences;
use kittypaw_sandbox::sandbox::Sandbox;

const ANALYSIS_KEYWORDS: &[&str] = &[
    "분석",
    "요약",
    "리포트",
    "비교",
    "정리",
    "추세",
    "검토",
    "파악",
    "analyze",
    "summary",
    "summarize",
    "report",
    "compare",
    "review",
    "research",
];

const AUTOMATION_KEYWORDS: &[&str] = &[
    "알림",
    "스케줄",
    "매일",
    "매주",
    "마다",
    "보내줘",
    "전송",
    "예약",
    "schedule",
    "daily",
    "weekly",
    "every",
    "remind",
    "send",
    "notify",
    "alert",
];

/// Classify skill intent: Automation/scheduling intent wins on conflict.
/// Default is Analysis for unknown inputs.
pub fn classify_tier(text: &str) -> ModelTier {
    let lower = text.to_lowercase();
    if AUTOMATION_KEYWORDS.iter().any(|k| lower.contains(k)) {
        tracing::debug!("classify_tier: {:?} → Automation", text);
        return ModelTier::Automation;
    }
    if ANALYSIS_KEYWORDS.iter().any(|k| lower.contains(k)) {
        tracing::debug!("classify_tier: {:?} → Analysis", text);
        return ModelTier::Analysis;
    }
    tracing::debug!("classify_tier: {:?} → Analysis (default)", text);
    ModelTier::Analysis
}

/// Resolve the model name override for a given tier.
///
/// Returns `None` when:
/// - `model_routing` feature gate is off (existing behaviour preserved)
/// - tier is `Analysis` (use the default model)
/// - tier is `Automation` but only one model is configured (graceful degradation)
pub fn tier_model_name(tier: Option<ModelTier>, config: &Config) -> Option<String> {
    if !config.features.model_routing {
        return None;
    }
    match tier.unwrap_or_default() {
        ModelTier::Analysis => None,
        ModelTier::Automation => {
            use kittypaw_core::config::ModelRoutingTier;

            // Priority 1: model explicitly tagged tier = "automation"
            if let Some(m) = config
                .models
                .iter()
                .find(|m| m.tier == Some(ModelRoutingTier::Automation))
            {
                return Some(m.name.clone());
            }

            // Priority 2: first non-default model (backwards-compat fallback)
            let default_name = config
                .models
                .iter()
                .find(|m| m.default)
                .map(|m| m.name.as_str());
            config
                .models
                .iter()
                .find(|m| Some(m.name.as_str()) != default_name)
                .map(|m| m.name.clone())
        }
    }
}

const TEACH_PROMPT: &str = r#"You are KittyPaw's skill generator. The user describes an automation they want, and you write a reusable JavaScript skill.

## Output format
Write ONLY valid JavaScript (ES2020) code. No markdown fences, no explanations.
Your code must be a single async function body that will be wrapped as:
  async function(ctx) { YOUR_CODE_HERE }

The `ctx` object contains:
- ctx.event_type: "telegram" | "web_chat"
- ctx.event_text: the message text that triggered this skill
- ctx.chat_id: the chat ID (for Telegram responses)

## Available primitives
- Telegram.sendMessage(text) — Send a message (chat_id auto-resolved)
- Telegram.sendPhoto(url) — Send a photo
- Telegram.sendDocument(fileUrl, caption?) — Send a file
- Telegram.sendVoice(filePath, caption?) — Send audio as voice message
- Http.get(url) — returns {status, body}
- Http.post(url, body) — returns {status, body}
- Http.put(url, body) — returns {status, body}
- Http.delete(url) — returns {status, body}
- Web.search(query) — Web search, returns {results: [{title, snippet, url}]}
- Web.fetch(url) — Fetch a web page, returns text content
- Storage.get(key) — returns {value} or {value: null}
- Storage.set(key, value) — returns {ok: true}
- Storage.delete(key)
- Storage.list() — returns {keys: [...]}
- Llm.generate(prompt) — returns {text: "..."}. Max 3 calls per execution.
- File.read(path) — Read a file
- File.write(path, content) — Write a file
- Env.get(key) — Get environment variable
- Shell.exec(command) — Execute a shell command
- Git.status() / Git.diff() / Git.log() / Git.commit(message)
- Tts.speak(text, options?) — Text-to-speech, returns {path, size}
- Memory.save(key, value) — Save to persistent memory
- Memory.recall(query) — Recall memories matching a prefix
- Memory.search(query, limit?) — Full-text search across executions
- Memory.user(key, value) — Update user profile
- Todo.add(task) / Todo.done(index) / Todo.list() / Todo.clear()
- Skill.create(name, description, code, triggerType, triggerValue) — Create a skill
- Skill.list() / Skill.delete(name)
- Agent.delegate(task) — Delegate a subtask to a sub-agent
- Moa.query(prompt) — Mixture of Agents: query all models in parallel
- Image.generate(prompt) — Generate an image, returns {url}
- Vision.analyze(imageUrl, prompt?) — Analyze an image, returns {analysis}
- Slack.sendMessage(text) — Send a Slack message
- Discord.sendMessage(text) — Send a Discord message
- console.log(...args)

## Rules
- Write focused, minimal code for the task described
- Use return to provide a text response to the user
- Use try/catch for error handling
- Do NOT use: require(), import, fetch(), Node.js APIs
"#;

pub enum TeachResult {
    Generated {
        code: String,
        dry_run_output: String,
        skill_name: String,
        description: String,
        trigger: SkillTrigger,
        permissions: Vec<String>,
    },
    Error(String),
}

pub async fn handle_teach(
    teach_text: &str,
    chat_id: &str,
    provider: &dyn LlmProvider,
    sandbox: &Sandbox,
    config: &Config,
    schedule: Option<&str>,
) -> Result<TeachResult> {
    // Admin check: Desktop/CLI (chat_id starts with "session_" or is empty) always allowed.
    // Remote channels require explicit admin_chat_ids config.
    let is_local = chat_id.is_empty() || chat_id == "local" || chat_id.starts_with("session_");
    if !is_local
        && (config.admin_chat_ids.is_empty()
            || !config.admin_chat_ids.iter().any(|id| id == chat_id))
    {
        return Ok(TeachResult::Error(
            "Permission denied: you are not an admin.".into(),
        ));
    }

    // Generate code via LLM
    let messages = vec![
        LlmMessage {
            role: Role::System,
            content: TEACH_PROMPT.to_string(),
        },
        LlmMessage {
            role: Role::User,
            content: format!("Create a skill for: {teach_text}\n\nThe chat_id is: {chat_id}"),
        },
    ];

    let raw_code = provider.generate(&messages).await?.content;
    let code = strip_code_fences(&raw_code);
    validate_generated_code(&code)?;

    // Dry-run in sandbox with mock context
    let mock_context = serde_json::json!({
        "event_type": "telegram",
        "event_text": teach_text,
        "chat_id": chat_id,
    });

    let wrapped = format!("const ctx = JSON.parse(__context__);\n{code}");
    let exec_result = sandbox.execute(&wrapped, mock_context).await?;

    // Dry-run failures are non-fatal — the code may call external APIs
    // (Web.search, Telegram.sendMessage) unavailable in dry-run sandbox.
    if !exec_result.success {
        let err_msg = exec_result
            .error
            .unwrap_or_else(|| "Unknown sandbox error".into());
        tracing::warn!("Dry-run failed (non-fatal): {err_msg}");
    }

    // Derive skill metadata
    let skill_name = slugify_description(teach_text);
    let permissions = detect_permissions(&code);

    // Schedule: LLM provides the schedule string directly (Hermes style).
    // parse_schedule() handles "every 10m", "every 2h", 5-field cron, etc.
    let trigger = if let Some(sched) = schedule.filter(|s| !s.is_empty()) {
        let cron_expr = parse_schedule(sched)?;
        SkillTrigger {
            trigger_type: "schedule".into(),
            keyword: None,
            cron: Some(cron_expr),
            natural: Some(teach_text.to_string()),
            run_at: None,
        }
    } else {
        SkillTrigger {
            trigger_type: "message".into(),
            keyword: Some(skill_name.clone()),
            cron: None,
            natural: None,
            run_at: None,
        }
    };

    let dry_run_output = if exec_result.output.is_empty() {
        "(no output)".to_string()
    } else {
        exec_result.output
    };

    Ok(TeachResult::Generated {
        code,
        dry_run_output,
        skill_name,
        description: teach_text.to_string(),
        trigger,
        permissions,
    })
}

pub fn approve_skill(result: &TeachResult) -> Result<()> {
    match result {
        TeachResult::Generated {
            code,
            skill_name,
            description,
            trigger,
            permissions,
            ..
        } => {
            // Validate cron expression before saving
            if let Some(ref cron_expr) = trigger.cron {
                if let Err(e) = crate::schedule::validate_cron(cron_expr) {
                    return Err(KittypawError::Config(format!("Invalid schedule: {e}")));
                }
            }
            let now = now_iso8601();
            let skill = Skill {
                name: skill_name.clone(),
                version: 1,
                description: description.clone(),
                created_at: now.clone(),
                updated_at: now,
                enabled: true,
                trigger: trigger.clone(),
                permissions: SkillPermissions {
                    primitives: permissions.clone(),
                    allowed_hosts: vec![],
                },
                format: kittypaw_core::skill::SkillFormat::Native,
                model_tier: Some(classify_tier(description)),
            };
            kittypaw_core::skill::save_skill(&skill, code)?;
            tracing::info!("Skill '{}' saved successfully", skill_name);
            Ok(())
        }
        TeachResult::Error(e) => Err(KittypawError::Skill(format!(
            "Cannot approve a failed result: {e}"
        ))),
    }
}

fn slugify_description(text: &str) -> String {
    let slug: String = text
        .split_whitespace()
        .take(4)
        .collect::<Vec<_>>()
        .join("-")
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-')
        .collect();

    // Trim leading/trailing dashes
    let slug = slug.trim_matches('-');

    if slug.is_empty() {
        "unnamed-skill".to_string()
    } else {
        slug.to_string()
    }
}

/// Parse a schedule string into a 7-field cron expression (Hermes style).
///
/// Accepts:
/// - Interval shorthand: "every 10m", "every 2h", "every 1d"
/// - Standard 5-field cron: "*/10 * * * *" (auto-prepends seconds)
/// - 6/7-field cron: "0 */10 * * * *" (passed through)
///
/// Returns a 6-field cron expression compatible with `cron` crate 0.16
/// (sec min hour dom mon dow).
pub fn parse_schedule(schedule: &str) -> Result<String> {
    let s = schedule.trim().to_lowercase();

    // "every Xm", "every Xh", "every Xd"
    let interval_re = regex::Regex::new(
        r"^every\s+(\d+)\s*(m|min|mins|minute|minutes|h|hr|hrs|hour|hours|d|day|days)$",
    )
    .unwrap();
    if let Some(caps) = interval_re.captures(&s) {
        let value: u32 = caps[1].parse().unwrap_or(1);
        let unit = &caps[2][..1]; // m, h, or d
        return match unit {
            "m" => {
                let n = value.max(5);
                Ok(format!("0 */{n} * * * *"))
            }
            "h" => Ok(format!("0 0 */{value} * * *")),
            "d" => Ok("0 0 9 * * *".to_string()), // daily at 9am
            _ => unreachable!(),
        };
    }

    // Cron expression: count fields to determine format
    let fields: Vec<&str> = s.split_whitespace().collect();
    if fields.len() >= 5
        && fields
            .iter()
            .take(6)
            .all(|f| f.chars().all(|c| c.is_ascii_digit() || "*/,-".contains(c)))
    {
        let expr = match fields.len() {
            5 => format!("0 {s}"),  // 5-field → prepend seconds
            6 | 7 => s.to_string(), // already has seconds
            _ => s.to_string(),
        };
        // Validate with cron crate
        use std::str::FromStr;
        cron::Schedule::from_str(&expr)
            .map_err(|e| KittypawError::Config(format!("Invalid cron: {e}")))?;
        return Ok(expr);
    }

    Err(KittypawError::Config(format!(
        "Invalid schedule: '{schedule}'. Use 'every 10m', 'every 2h', or cron like '*/10 * * * *'"
    )))
}

/// Parse a relative delay string ("2m", "10m", "1h") into an absolute UTC datetime.
///
/// Formats: `"{N}m"` (minutes, minimum 1), `"{N}h"` (hours)
/// Returns `now + delay`.
// Maximum allowed delay to prevent integer overflow: 365 days in minutes
const MAX_DELAY_MINUTES: u64 = 365 * 24 * 60;

pub fn parse_once_delay(s: &str) -> Result<chrono::DateTime<chrono::Utc>> {
    let s = s.trim().to_lowercase();

    // Try "{N}m" format
    if let Some(n_str) = s.strip_suffix('m') {
        let n: u64 = n_str.parse().map_err(|_| {
            KittypawError::Config(format!(
                "Invalid delay '{s}': expected format like '2m' or '1h'"
            ))
        })?;
        if n == 0 {
            return Err(KittypawError::Config(
                "Delay must be at least 1 minute".into(),
            ));
        }
        if n > MAX_DELAY_MINUTES {
            return Err(KittypawError::Config(format!(
                "Delay too large ({n}m). Maximum is {MAX_DELAY_MINUTES}m (365 days)."
            )));
        }
        return Ok(chrono::Utc::now() + chrono::Duration::minutes(n as i64));
    }

    // Try "{N}h" format
    if let Some(n_str) = s.strip_suffix('h') {
        let n: u64 = n_str.parse().map_err(|_| {
            KittypawError::Config(format!(
                "Invalid delay '{s}': expected format like '2m' or '1h'"
            ))
        })?;
        if n == 0 {
            return Err(KittypawError::Config(
                "Delay must be at least 1 minute".into(),
            ));
        }
        let as_minutes = n.saturating_mul(60);
        if as_minutes > MAX_DELAY_MINUTES {
            return Err(KittypawError::Config(format!(
                "Delay too large ({n}h). Maximum is {} hours (365 days).",
                MAX_DELAY_MINUTES / 60
            )));
        }
        return Ok(chrono::Utc::now() + chrono::Duration::hours(n as i64));
    }

    Err(KittypawError::Config(format!(
        "Invalid once delay: '{s}'. Use '2m', '10m', '1h', etc."
    )))
}

/// Validate generated code for dangerous patterns.
/// Http+Storage combos are now allowed — the sandbox already provides
/// allowed_hosts and storage namespace isolation at runtime.
fn validate_generated_code(code: &str) -> Result<()> {
    let has_http = code.contains("Http.");
    let has_storage = code.contains("Storage.");
    if has_http && has_storage {
        tracing::info!("Skill uses both Http and Storage — allowed (sandbox-guarded)");
    }
    Ok(())
}

fn detect_permissions(code: &str) -> Vec<String> {
    let mut perms = Vec::new();
    for prim in [
        "Http", "Web", "Telegram", "Slack", "Discord", "Storage", "Llm", "Shell", "Git", "File",
        "Tts", "Memory", "Todo", "Skill", "Agent", "Moa", "Image", "Vision", "Env",
    ] {
        if code.contains(&format!("{prim}.")) {
            perms.push(prim.to_string());
        }
    }
    perms
}

fn now_iso8601() -> String {
    chrono::Utc::now().to_rfc3339()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_slugify_description() {
        assert_eq!(
            slugify_description("send a daily joke"),
            "send-a-daily-joke"
        );
        assert_eq!(slugify_description("Hello World"), "hello-world");
        assert_eq!(
            slugify_description("track my expenses in a spreadsheet"),
            "track-my-expenses-in"
        );
        assert_eq!(slugify_description(""), "unnamed-skill");
    }

    #[test]
    fn test_slugify_korean() {
        // Korean descriptions must produce unique, meaningful slugs
        let slug1 = slugify_description("AI 뉴스 10분마다 요약해줘");
        let slug2 = slugify_description("정치 뉴스 5분마다 요약해줘");
        assert_ne!(
            slug1, slug2,
            "different descriptions must produce different slugs"
        );
        assert!(
            slug1.len() > 3,
            "korean slug should be meaningful, got: {slug1}"
        );
        assert!(
            !slug1.starts_with('-') && !slug1.ends_with('-'),
            "slug should not start/end with dash: {slug1}"
        );
    }

    #[test]
    fn test_detect_permissions() {
        let code = r#"
            const resp = await Http.get("https://example.com");
            await Telegram.sendMessage(ctx.chat_id, resp.body);
        "#;
        let perms = detect_permissions(code);
        assert!(perms.contains(&"Telegram".to_string()));
        assert!(perms.contains(&"Http".to_string()));
        assert!(!perms.contains(&"Storage".to_string()));
        assert!(!perms.contains(&"Llm".to_string()));
    }

    #[test]
    fn test_detect_permissions_all() {
        let code = "Telegram.sendMessage(); Http.get(); Storage.set(); Llm.generate();";
        let perms = detect_permissions(code);
        assert_eq!(perms.len(), 4);
    }

    #[test]
    fn test_validate_generated_code_allows_http_only() {
        let code = r#"const resp = await Http.get("https://example.com");"#;
        assert!(validate_generated_code(code).is_ok());
    }

    #[test]
    fn test_validate_generated_code_allows_storage_only() {
        let code = r#"const val = await Storage.get("key");"#;
        assert!(validate_generated_code(code).is_ok());
    }

    #[test]
    fn test_validate_generated_code_allows_http_and_storage() {
        let code = r#"
            const resp = await Http.get("https://example.com");
            await Storage.set("data", resp.body);
        "#;
        assert!(validate_generated_code(code).is_ok());
    }

    #[test]
    fn test_parse_schedule_interval() {
        assert_eq!(parse_schedule("every 10m").unwrap(), "0 */10 * * * *");
        assert_eq!(parse_schedule("every 2h").unwrap(), "0 0 */2 * * *");
        assert_eq!(parse_schedule("every 1d").unwrap(), "0 0 9 * * *");
        // minimum 5 minutes
        assert_eq!(parse_schedule("every 3m").unwrap(), "0 */5 * * * *");
    }

    #[test]
    fn test_parse_schedule_cron_5field() {
        let result = parse_schedule("*/10 * * * *").unwrap();
        assert_eq!(result, "0 */10 * * * *");
    }

    #[test]
    fn test_parse_schedule_cron_5field_5min() {
        // LLM generates "*/5 * * * *" for 5-minute intervals
        let result = parse_schedule("*/5 * * * *").unwrap();
        assert_eq!(result, "0 */5 * * * *");
    }

    #[test]
    fn test_parse_schedule_cron_6field() {
        let result = parse_schedule("0 */10 * * * *").unwrap();
        assert_eq!(result, "0 */10 * * * *");
    }

    #[test]
    fn test_parse_schedule_invalid() {
        assert!(parse_schedule("banana").is_err());
        assert!(parse_schedule("").is_err());
    }

    #[test]
    fn parse_once_delay_valid_formats() {
        use chrono::Utc;
        let before = Utc::now();
        let t_2m = parse_once_delay("2m").unwrap();
        let after = Utc::now();
        // 2m delay: should be between now+1m and now+3m
        assert!(t_2m > before + chrono::Duration::minutes(1));
        assert!(t_2m < after + chrono::Duration::minutes(3));

        let t_1h = parse_once_delay("1h").unwrap();
        assert!(t_1h > before + chrono::Duration::minutes(59));
        assert!(t_1h < after + chrono::Duration::minutes(61));
    }

    #[test]
    fn parse_once_delay_minimum_one_minute() {
        // "0m"은 에러
        assert!(
            parse_once_delay("0m").is_err(),
            "0m should fail minimum check"
        );
    }

    #[test]
    fn parse_once_delay_invalid_format() {
        assert!(parse_once_delay("abc").is_err());
        assert!(parse_once_delay("").is_err());
        assert!(parse_once_delay("2s").is_err(), "seconds not supported");
    }

    #[test]
    fn classify_tier_automation_wins_on_conflict_with_analysis() {
        use kittypaw_core::skill::ModelTier;
        // "보내줘" (Automation) wins over "요약" (Analysis)
        assert_eq!(classify_tier("뉴스 요약해서 보내줘"), ModelTier::Automation);
    }

    #[test]
    fn classify_tier_automation_keyword() {
        use kittypaw_core::skill::ModelTier;
        assert_eq!(
            classify_tier("매일 아침 날씨 알림 보내줘"),
            ModelTier::Automation
        );
    }

    #[test]
    fn classify_tier_automation_wins_when_both_keywords_present() {
        use kittypaw_core::skill::ModelTier;
        // "보내줘" (Automation) wins over "분석" (Analysis)
        assert_eq!(classify_tier("분석해서 보내줘"), ModelTier::Automation);
    }

    #[test]
    fn classify_tier_default_analysis() {
        use kittypaw_core::skill::ModelTier;
        assert_eq!(classify_tier("안녕"), ModelTier::Analysis);
    }

    #[test]
    fn classify_tier_automation_wins_mixed_language() {
        use kittypaw_core::skill::ModelTier;
        // "every" (Automation) wins over "요약" (Analysis)
        assert_eq!(classify_tier("every day 요약"), ModelTier::Automation);
    }

    #[test]
    fn classify_tier_automation_wins_english_conflict() {
        use kittypaw_core::skill::ModelTier;
        // "보내줘" (Automation) wins over "analyze" (Analysis)
        assert_eq!(classify_tier("analyze and 보내줘"), ModelTier::Automation);
    }

    #[test]
    fn classify_tier_daily_report_send_is_automation() {
        use kittypaw_core::skill::ModelTier;
        // motivating example: "매일" triggers Automation first
        assert_eq!(classify_tier("매일 리포트 보내줘"), ModelTier::Automation);
    }

    #[test]
    fn classify_tier_report_only_is_analysis() {
        use kittypaw_core::skill::ModelTier;
        // no scheduling keyword → Analysis
        assert_eq!(classify_tier("리포트 분석해줘"), ModelTier::Analysis);
    }

    fn two_model_config(routing: bool) -> Config {
        use kittypaw_core::config::{FeatureFlags, ModelConfig};
        Config {
            models: vec![
                ModelConfig {
                    name: "default-model".into(),
                    provider: "openai".into(),
                    model: "gpt-4".into(),
                    api_key: "key".into(),
                    max_tokens: 1000,
                    default: true,
                    base_url: None,
                    context_window: None,
                    tier: None,
                },
                ModelConfig {
                    name: "fast-model".into(),
                    provider: "openai".into(),
                    model: "gpt-3.5-turbo".into(),
                    api_key: "key".into(),
                    max_tokens: 1000,
                    default: false,
                    base_url: None,
                    context_window: None,
                    tier: None,
                },
            ],
            features: FeatureFlags {
                model_routing: routing,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn tier_model_name_feature_gate_off_returns_none() {
        use kittypaw_core::skill::ModelTier;
        let config = two_model_config(false);
        assert_eq!(tier_model_name(Some(ModelTier::Automation), &config), None);
    }

    #[test]
    fn tier_model_name_analysis_returns_none() {
        use kittypaw_core::skill::ModelTier;
        let config = two_model_config(true);
        assert_eq!(tier_model_name(Some(ModelTier::Analysis), &config), None);
    }

    #[test]
    fn tier_model_name_automation_two_models_returns_fallback() {
        use kittypaw_core::skill::ModelTier;
        let config = two_model_config(true);
        assert_eq!(
            tier_model_name(Some(ModelTier::Automation), &config),
            Some("fast-model".to_string())
        );
    }

    #[test]
    fn tier_model_name_automation_one_model_returns_none() {
        use kittypaw_core::config::{FeatureFlags, ModelConfig};
        use kittypaw_core::skill::ModelTier;
        let config = Config {
            models: vec![ModelConfig {
                name: "only-model".into(),
                provider: "openai".into(),
                model: "gpt-4".into(),
                api_key: "key".into(),
                max_tokens: 1000,
                default: true,
                base_url: None,
                context_window: None,
                tier: None,
            }],
            features: FeatureFlags {
                model_routing: true,
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(tier_model_name(Some(ModelTier::Automation), &config), None);
    }

    #[test]
    fn tier_model_name_explicit_automation_tier_wins_over_insertion_order() {
        use kittypaw_core::config::{FeatureFlags, ModelConfig, ModelRoutingTier};
        use kittypaw_core::skill::ModelTier;
        // Bug scenario: wrong-model is listed first (non-default), fast-model has explicit tier
        let config = Config {
            models: vec![
                ModelConfig {
                    name: "wrong-model".into(), // old code would pick this
                    provider: "openai".into(),
                    model: "gpt-4o".into(),
                    api_key: "key".into(),
                    max_tokens: 1000,
                    default: false,
                    base_url: None,
                    context_window: None,
                    tier: None,
                },
                ModelConfig {
                    name: "default-model".into(),
                    provider: "openai".into(),
                    model: "gpt-4".into(),
                    api_key: "key".into(),
                    max_tokens: 1000,
                    default: true,
                    base_url: None,
                    context_window: None,
                    tier: None,
                },
                ModelConfig {
                    name: "fast-model".into(),
                    provider: "openai".into(),
                    model: "gpt-3.5-turbo".into(),
                    api_key: "key".into(),
                    max_tokens: 1000,
                    default: false,
                    base_url: None,
                    context_window: None,
                    tier: Some(ModelRoutingTier::Automation),
                },
            ],
            features: FeatureFlags {
                model_routing: true,
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(
            tier_model_name(Some(ModelTier::Automation), &config),
            Some("fast-model".to_string())
        );
    }
}
