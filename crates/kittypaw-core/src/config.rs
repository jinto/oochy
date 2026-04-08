use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

use crate::error::{KittypawError, Result};

/// Controls what skill operations are allowed at runtime.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AutonomyLevel {
    /// Only read operations (Http.get, Storage.get, File.read, etc.)
    #[serde(alias = "read_only")]
    Readonly,
    /// Write operations require user confirmation via permission callback
    Supervised,
    /// All operations allowed (default)
    #[default]
    Full,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub llm: LlmConfig,
    pub sandbox: SandboxConfig,
    #[serde(default)]
    pub agents: Vec<AgentConfig>,
    #[serde(default)]
    pub channels: Vec<ChannelConfig>,
    #[serde(default)]
    pub admin_chat_ids: Vec<String>,
    #[serde(default)]
    pub freeform_fallback: bool,
    #[serde(default)]
    pub models: Vec<ModelConfig>,
    #[serde(default)]
    pub stt: SttConfig,
    #[serde(default)]
    pub features: FeatureFlags,
    #[serde(default)]
    pub mcp_servers: Vec<McpServerConfig>,
    /// Autonomy level: "readonly", "supervised", or "full" (default)
    #[serde(default)]
    pub autonomy_level: AutonomyLevel,
    /// Allowed Telegram chat IDs. Empty = allow all (no pairing).
    #[serde(default)]
    pub paired_chat_ids: Vec<String>,
    /// Server settings for the HTTP API.
    #[serde(default)]
    pub server: ServerConfig,
    /// Profile configurations.
    #[serde(default)]
    pub profiles: Vec<ProfileConfig>,
    /// Default profile name (used when no channel/session mapping matches).
    #[serde(default = "default_profile_name")]
    pub default_profile: String,
    /// Reflection loop settings (daily pattern analysis + skill suggestion).
    #[serde(default)]
    pub reflection: ReflectionConfig,
}

fn default_profile_name() -> String {
    "default".to_string()
}

/// Profile configuration — each profile has its own SOUL.md, USER.md, and memory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileConfig {
    pub id: String,
    /// Display name / nickname (e.g., "키티", "비서")
    #[serde(default)]
    pub nick: String,
    /// Channels that auto-bind to this profile (e.g., ["slack", "discord"])
    #[serde(default)]
    pub channels: Vec<String>,
}

/// Server configuration for the HTTP API layer.
///
/// When `api_key` is set, authenticated REST endpoints are exposed at `/api/v1/*`.
/// When empty, the API is disabled (only dashboard and WebSocket remain).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ServerConfig {
    /// API key for authenticating REST requests.
    /// Override with `KITTYPAW_SERVER_API_KEY` env var.
    #[serde(default)]
    pub api_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
}

/// Feature flags / kill switches for runtime behaviour.
///
/// All flags are opt-in or opt-out via `[features]` in `kittypaw.toml`.
/// Missing fields fall back to the defaults shown below.
///
/// Example:
/// ```toml
/// [features]
/// progressive_retry = true
/// context_compaction = false
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeatureFlags {
    /// Progressive prompt compaction on retry (default: enabled).
    #[serde(default = "default_true")]
    pub progressive_retry: bool,
    /// 3-stage context compaction (default: enabled).
    #[serde(default = "default_true")]
    pub context_compaction: bool,
    /// Per-skill automatic model selection — experimental (default: disabled).
    #[serde(default)]
    pub model_routing: bool,
    /// Background agent execution — experimental (default: disabled).
    #[serde(default)]
    pub background_agents: bool,
    /// Daily token usage limit. 0 = unlimited.
    #[serde(default)]
    pub daily_token_limit: u64,
}

fn default_true() -> bool {
    true
}

impl Default for FeatureFlags {
    fn default() -> Self {
        Self {
            progressive_retry: true,
            context_compaction: true,
            model_routing: false,
            background_agents: false,
            daily_token_limit: 0,
        }
    }
}

/// Reflection loop: daily analysis of user conversation patterns.
///
/// When enabled, a daily cron job analyzes recent conversations via LLM
/// to detect repeated intents and suggest new skills.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReflectionConfig {
    /// Enable the reflection loop (default: true).
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Cron expression for the reflection schedule (default: daily 3 AM).
    #[serde(default = "default_reflection_cron")]
    pub cron: String,
    /// Maximum input characters for LLM analysis (default: 4000 ≈ 2000 tokens for Korean).
    #[serde(default = "default_reflection_max_chars")]
    pub max_input_chars: u32,
    /// Minimum repeat count to trigger a suggestion (default: 3).
    #[serde(default = "default_reflection_threshold")]
    pub intent_threshold: u32,
    /// Days before unused reflection data expires (default: 7).
    #[serde(default = "default_reflection_ttl")]
    pub ttl_days: u32,
}

fn default_reflection_cron() -> String {
    "0 0 3 * * *".to_string()
}
fn default_reflection_max_chars() -> u32 {
    4000
}
fn default_reflection_threshold() -> u32 {
    3
}
fn default_reflection_ttl() -> u32 {
    7
}

impl Default for ReflectionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            cron: default_reflection_cron(),
            max_input_chars: default_reflection_max_chars(),
            intent_threshold: default_reflection_threshold(),
            ttl_days: default_reflection_ttl(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmConfig {
    pub provider: String,
    pub api_key: String,
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
}

fn default_model() -> String {
    "claude-sonnet-4-20250514".into()
}

fn default_max_tokens() -> u32 {
    4096
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfig {
    pub name: String,
    pub provider: String,
    pub model: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    #[serde(default)]
    pub default: bool,
    #[serde(default)]
    pub base_url: Option<String>,
    /// Override the provider's auto-detected context window (in tokens).
    #[serde(default)]
    pub context_window: Option<u32>,
}

fn default_stt_language() -> String {
    "ko".into()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SttConfig {
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default = "default_stt_language")]
    pub language: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxConfig {
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    #[serde(default = "default_memory_mb")]
    pub memory_limit_mb: u64,
    #[serde(default)]
    pub allowed_paths: Vec<PathBuf>,
    #[serde(default)]
    pub allowed_hosts: Vec<String>,
}

fn default_timeout() -> u64 {
    30
}

fn default_memory_mb() -> u64 {
    64
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelType {
    Telegram,
    Slack,
    Discord,
    Web,
    Desktop,
    KakaoTalk,
}

impl std::fmt::Display for ChannelType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            ChannelType::Telegram => "telegram",
            ChannelType::Slack => "slack",
            ChannelType::Discord => "discord",
            ChannelType::Web => "web",
            ChannelType::Desktop => "desktop",
            ChannelType::KakaoTalk => "kakao_talk",
        };
        f.write_str(s)
    }
}

impl PartialEq<str> for ChannelType {
    fn eq(&self, other: &str) -> bool {
        matches!(
            (self, other),
            (ChannelType::Telegram, "telegram")
                | (ChannelType::Slack, "slack")
                | (ChannelType::Discord, "discord")
                | (ChannelType::Web, "web")
                | (ChannelType::Desktop, "desktop")
                | (ChannelType::KakaoTalk, "kakao_talk")
        )
    }
}

impl PartialEq<&str> for ChannelType {
    fn eq(&self, other: &&str) -> bool {
        self == *other
    }
}

/// KakaoTalk-specific channel configuration.
///
/// Unlike other channels, KakaoTalk uses a CF Worker relay instead of a direct API.
/// - `relay_url`: The base URL of the deployed CF Worker (e.g., `https://relay.example.workers.dev`)
/// - `user_token`: The per-user token used as the KV namespace key prefix
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct KakaoChannelConfig {
    pub relay_url: String,
    pub user_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelConfig {
    pub channel_type: ChannelType,
    #[serde(default)]
    pub token: String,
    #[serde(default)]
    pub bind_addr: Option<String>,
    /// KakaoTalk-specific config. Required when channel_type = kakao_talk.
    #[serde(default)]
    pub kakao: Option<KakaoChannelConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    pub id: String,
    pub name: String,
    pub system_prompt: String,
    #[serde(default)]
    pub channels: Vec<String>,
    #[serde(default)]
    pub allowed_skills: Vec<SkillPermission>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillPermission {
    pub skill: String,
    #[serde(default)]
    pub methods: Vec<String>,
    #[serde(default = "default_rate_limit")]
    pub rate_limit_per_minute: u32,
}

fn default_rate_limit() -> u32 {
    60
}

impl Config {
    pub fn load() -> Result<Self> {
        // Layer 1: Try ~/.kittypaw/kittypaw.toml, then ./kittypaw.toml
        let home_config = crate::secrets::data_dir()
            .ok()
            .map(|d| d.join("kittypaw.toml"));
        let local_config = PathBuf::from("kittypaw.toml");
        let config_path = home_config.filter(|p| p.exists()).unwrap_or(local_config);
        let mut config = if config_path.exists() {
            let content = std::fs::read_to_string(&config_path)?;
            toml::from_str(&content).map_err(|e| {
                KittypawError::Config(format!("Invalid {}: {e}", config_path.display()))
            })?
        } else {
            Config::default()
        };

        // Layer 2: Override with env vars
        if let Ok(key) = std::env::var("KITTYPAW_API_KEY") {
            config.llm.api_key = key;
        }
        if let Ok(provider) = std::env::var("KITTYPAW_LLM_PROVIDER") {
            config.llm.provider = provider;
        }
        if let Ok(model) = std::env::var("KITTYPAW_MODEL") {
            config.llm.model = model;
        }

        // Layer 2b: Server API key from env
        if let Ok(key) = std::env::var("KITTYPAW_SERVER_API_KEY") {
            config.server.api_key = key;
        }

        // Layer 3: Fall back to the local secret store if api_key is still empty
        if config.llm.api_key.is_empty() {
            if let Ok(Some(key)) = crate::secrets::get_secret("settings", "api_key") {
                config.llm.api_key = key;
            }
        }

        // Ensure default profile exists on disk
        let default_nick = config
            .profiles
            .iter()
            .find(|p| p.id == config.default_profile)
            .map(|p| p.nick.as_str())
            .unwrap_or("");
        crate::profile::ensure_default_profile(default_nick);

        Ok(config)
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            llm: LlmConfig {
                provider: "claude".into(),
                api_key: String::new(),
                model: default_model(),
                max_tokens: default_max_tokens(),
            },
            sandbox: SandboxConfig {
                timeout_secs: default_timeout(),
                memory_limit_mb: default_memory_mb(),
                allowed_paths: vec![],
                allowed_hosts: vec![],
            },
            agents: vec![],
            channels: vec![],
            admin_chat_ids: vec![],
            freeform_fallback: false,
            models: vec![],
            stt: SttConfig::default(),
            features: FeatureFlags::default(),
            mcp_servers: vec![],
            autonomy_level: AutonomyLevel::default(),
            paired_chat_ids: vec![],
            server: ServerConfig::default(),
            profiles: vec![],
            default_profile: default_profile_name(),
            reflection: ReflectionConfig::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_feature_flags_defaults() {
        let flags = FeatureFlags::default();
        assert!(
            flags.progressive_retry,
            "progressive_retry should default to true"
        );
        assert!(
            flags.context_compaction,
            "context_compaction should default to true"
        );
        assert!(
            !flags.model_routing,
            "model_routing should default to false"
        );
        assert!(
            !flags.background_agents,
            "background_agents should default to false"
        );
    }

    #[test]
    fn test_feature_flags_missing_section_gives_defaults() {
        let toml = r#"
[llm]
provider = "claude"
api_key = "test-key"

[sandbox]
timeout_secs = 30
memory_limit_mb = 64
"#;
        let config: Config = toml::from_str(toml).expect("should parse");
        assert!(config.features.progressive_retry);
        assert!(config.features.context_compaction);
        assert!(!config.features.model_routing);
        assert!(!config.features.background_agents);
    }

    #[test]
    fn test_feature_flags_explicit_values() {
        let toml = r#"
[llm]
provider = "claude"
api_key = "test-key"

[sandbox]
timeout_secs = 30
memory_limit_mb = 64

[features]
progressive_retry = true
context_compaction = false
model_routing = true
background_agents = false
"#;
        let config: Config = toml::from_str(toml).expect("should parse");
        assert!(config.features.progressive_retry);
        assert!(!config.features.context_compaction);
        assert!(config.features.model_routing);
        assert!(!config.features.background_agents);
    }

    #[test]
    fn test_feature_flags_partial_section() {
        // Only some flags specified — rest fall back to defaults
        let toml = r#"
[llm]
provider = "claude"
api_key = "test-key"

[sandbox]
timeout_secs = 30
memory_limit_mb = 64

[features]
context_compaction = false
"#;
        let config: Config = toml::from_str(toml).expect("should parse");
        // Not specified → defaults
        assert!(config.features.progressive_retry);
        assert!(!config.features.model_routing);
        assert!(!config.features.background_agents);
        // Explicitly set
        assert!(!config.features.context_compaction);
    }

    #[test]
    fn test_reflection_config_defaults() {
        let config = ReflectionConfig::default();
        assert!(config.enabled);
        assert_eq!(config.cron, "0 0 3 * * *");
        assert_eq!(config.max_input_chars, 4000);
        assert_eq!(config.intent_threshold, 3);
        assert_eq!(config.ttl_days, 7);
    }

    #[test]
    fn test_reflection_config_from_toml() {
        let toml = r#"
[llm]
provider = "claude"
api_key = "test-key"

[sandbox]
timeout_secs = 30
memory_limit_mb = 64

[reflection]
enabled = true
intent_threshold = 5
ttl_days = 14
"#;
        let config: Config = toml::from_str(toml).expect("should parse");
        assert!(config.reflection.enabled);
        assert_eq!(config.reflection.intent_threshold, 5);
        assert_eq!(config.reflection.ttl_days, 14);
        // Defaults for unspecified fields
        assert_eq!(config.reflection.cron, "0 0 3 * * *");
        assert_eq!(config.reflection.max_input_chars, 4000);
    }

    #[test]
    fn test_reflection_config_missing_section() {
        let toml = r#"
[llm]
provider = "claude"
api_key = "test-key"

[sandbox]
timeout_secs = 30
memory_limit_mb = 64
"#;
        let config: Config = toml::from_str(toml).expect("should parse");
        assert!(config.reflection.enabled);
        assert_eq!(config.reflection.intent_threshold, 3);
    }
}
