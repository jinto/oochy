use std::path::PathBuf;

use crate::config::Config;

const DEFAULT_SOUL: &str = r#"나는 KittyPaw, 조용히 돕는 개인 AI 비서입니다.
사용자의 요청을 정확하게 처리하고, 불필요한 말은 하지 않습니다.
모호한 요청에는 먼저 질문하고, 사용자의 선호도를 기억합니다.
"#;

/// Profile data loaded from disk.
pub struct Profile {
    pub id: String,
    pub nick: String,
    pub soul: String,
    pub user_md: String,
}

fn profiles_dir() -> PathBuf {
    crate::secrets::data_dir()
        .unwrap_or_else(|_| PathBuf::from(".kittypaw"))
        .join("profiles")
}

/// Ensure the default profile directory and files exist.
pub fn ensure_default_profile(nick: &str) {
    let dir = profiles_dir().join("default");
    if dir.exists() {
        return;
    }
    let _ = std::fs::create_dir_all(&dir);
    let soul = if nick.is_empty() {
        DEFAULT_SOUL.to_string()
    } else {
        format!("나는 \"{nick}\"라는 이름의 개인 AI 비서입니다.\n사용자의 요청을 정확하게 처리하고, 불필요한 말은 하지 않습니다.\n모호한 요청에는 먼저 질문하고, 사용자의 선호도를 기억합니다.\n")
    };
    let _ = std::fs::write(dir.join("SOUL.md"), &soul);
    let _ = std::fs::write(dir.join("USER.md"), "");
}

/// Sanitize profile name: alphanumeric + hyphens only.
fn sanitize_profile_name(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect()
}

/// Load a profile by name. Returns (soul, user_md).
pub fn load_profile(name: &str) -> Profile {
    let safe_name = sanitize_profile_name(name);
    let dir = profiles_dir().join(&safe_name);
    let soul = std::fs::read_to_string(dir.join("SOUL.md")).unwrap_or_else(|_| DEFAULT_SOUL.into());
    let user_md = std::fs::read_to_string(dir.join("USER.md")).unwrap_or_default();

    Profile {
        id: name.to_string(),
        nick: String::new(), // filled by caller from config
        soul,
        user_md,
    }
}

/// Save content to a profile's USER.md.
pub fn save_user_md(profile_name: &str, content: &str) -> std::io::Result<()> {
    let dir = profiles_dir().join(sanitize_profile_name(profile_name));
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("USER.md"), content)
}

/// Determine which profile to use based on channel, session state, and config.
///
/// Priority:
/// 1. Session override (active_profile in user_context)
/// 2. Channel binding (profiles[].channels)
/// 3. Default profile
pub fn resolve_profile_name(
    config: &Config,
    channel_type: &str,
    active_override: Option<&str>,
) -> String {
    // 1. Session override
    if let Some(name) = active_override {
        if config.profiles.iter().any(|p| p.id == name) || name == "default" {
            return name.to_string();
        }
    }

    // 2. Channel binding
    for profile in &config.profiles {
        if profile
            .channels
            .iter()
            .any(|ch| ch.eq_ignore_ascii_case(channel_type))
        {
            return profile.id.clone();
        }
    }

    // 3. Default
    config.default_profile.clone()
}

/// Find a profile by nick name (for natural language switching).
pub fn find_profile_by_nick<'a>(config: &'a Config, nick: &str) -> Option<&'a str> {
    config
        .profiles
        .iter()
        .find(|p| p.nick.eq_ignore_ascii_case(nick))
        .map(|p| p.id.as_str())
}
