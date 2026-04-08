use kittypaw_core::config::{ChannelConfig, ChannelType};

use crate::channel::Channel;
use crate::discord::DiscordChannel;
use crate::kakao::KakaoChannel;
use crate::slack::SlackChannel;
use crate::telegram::TelegramChannel;

/// Registry that creates Channel instances from config.
/// Adding a new channel type requires only:
/// 1. Adding a variant to ChannelType enum
/// 2. Implementing Channel trait
/// 3. Adding a match arm in `create()`
pub struct ChannelRegistry;

impl ChannelRegistry {
    /// Create a channel from its config. Returns None for types that
    /// don't support polling (Web, Desktop) or lack required config.
    pub fn create(config: &ChannelConfig) -> Option<Box<dyn Channel>> {
        // KakaoTalk uses relay_url + user_token, not a single token — handle separately
        if config.channel_type == ChannelType::KakaoTalk {
            return create_kakao(config);
        }

        // Resolve token: config field → env var → secrets store
        let token = resolve_token(&config.channel_type, &config.token);
        if token.is_empty() {
            tracing::debug!(
                channel = %config.channel_type,
                "Skipping channel: no token configured"
            );
            return None;
        }

        match config.channel_type {
            ChannelType::Telegram => Some(Box::new(TelegramChannel::new(&token))),
            ChannelType::Slack => {
                let app_token = std::env::var("KITTYPAW_SLACK_APP_TOKEN").unwrap_or_default();
                if app_token.is_empty() {
                    tracing::warn!("Slack bot token found but KITTYPAW_SLACK_APP_TOKEN missing");
                    return None;
                }
                Some(Box::new(SlackChannel::new(&token, &app_token)))
            }
            ChannelType::Discord => Some(Box::new(DiscordChannel::new(&token))),
            // KakaoTalk handled above
            ChannelType::KakaoTalk => unreachable!(),
            // Web and Desktop are handled by WebSocket/GUI, not polling channels
            ChannelType::Web | ChannelType::Desktop => None,
        }
    }

    /// Create all configured channels, skipping any that fail to initialize.
    pub fn create_all(configs: &[ChannelConfig]) -> Vec<Box<dyn Channel>> {
        configs.iter().filter_map(Self::create).collect()
    }
}

/// Create a KakaoTalk channel from config.
fn create_kakao(config: &ChannelConfig) -> Option<Box<dyn Channel>> {
    let kakao_cfg = config.kakao.as_ref()?;
    let relay_url = kakao_cfg.relay_url.clone();
    if relay_url.is_empty() {
        tracing::warn!("KakaoTalk channel: relay_url is not configured");
        return None;
    }
    let user_token = if !kakao_cfg.user_token.is_empty() {
        kakao_cfg.user_token.clone()
    } else {
        resolve_kakao_token()
    };
    if user_token.is_empty() {
        tracing::debug!("Skipping KakaoTalk channel: no user_token configured");
        return None;
    }
    Some(Box::new(KakaoChannel::new(relay_url, user_token)))
}

/// Resolve a channel token using priority: config field → env var → secrets store.
fn resolve_token(channel_type: &ChannelType, config_token: &str) -> String {
    if !config_token.is_empty() {
        return config_token.to_string();
    }

    let (env_var, secret_key) = match channel_type {
        ChannelType::Telegram => ("KITTYPAW_TELEGRAM_TOKEN", "telegram_token"),
        ChannelType::Slack => ("KITTYPAW_SLACK_BOT_TOKEN", "slack_token"),
        ChannelType::Discord => ("KITTYPAW_DISCORD_TOKEN", "discord_token"),
        // KakaoTalk uses relay_url + user_token, not a single token
        _ => return String::new(),
    };

    // Try env var
    if let Ok(val) = std::env::var(env_var) {
        if !val.is_empty() {
            return val;
        }
    }

    // Try secrets store
    if let Ok(Some(val)) = kittypaw_core::secrets::get_secret("channels", secret_key) {
        if !val.is_empty() {
            return val;
        }
    }

    String::new()
}

/// Resolve the KakaoTalk user_token from env var or secrets store.
fn resolve_kakao_token() -> String {
    if let Ok(val) = std::env::var("KITTYPAW_KAKAO_TOKEN") {
        if !val.is_empty() {
            return val;
        }
    }
    if let Ok(Some(val)) = kittypaw_core::secrets::get_secret("channels", "kakao_token") {
        if !val.is_empty() {
            return val;
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_skips_web_desktop() {
        let web = ChannelConfig {
            channel_type: ChannelType::Web,
            token: String::new(),
            bind_addr: None,
            kakao: None,
        };
        assert!(ChannelRegistry::create(&web).is_none());

        let desktop = ChannelConfig {
            channel_type: ChannelType::Desktop,
            token: String::new(),
            bind_addr: None,
            kakao: None,
        };
        assert!(ChannelRegistry::create(&desktop).is_none());
    }

    #[test]
    fn test_create_skips_empty_token() {
        let tg = ChannelConfig {
            channel_type: ChannelType::Telegram,
            token: String::new(),
            bind_addr: None,
            kakao: None,
        };
        // Without env vars set, should return None
        assert!(ChannelRegistry::create(&tg).is_none());
    }

    #[test]
    fn test_create_all_filters() {
        let configs = vec![
            ChannelConfig {
                channel_type: ChannelType::Web,
                token: String::new(),
                bind_addr: None,
                kakao: None,
            },
            ChannelConfig {
                channel_type: ChannelType::Desktop,
                token: String::new(),
                bind_addr: None,
                kakao: None,
            },
        ];
        assert!(ChannelRegistry::create_all(&configs).is_empty());
    }
}
