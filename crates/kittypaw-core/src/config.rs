use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

use crate::error::{KittypawError, Result};

/// Controls what skill operations are allowed at runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AutonomyLevel {
    /// Only read operations (Http.get, Storage.get, File.read, etc.)
    #[serde(alias = "read_only")]
    Readonly,
    /// Write operations require user confirmation via permission callback
    Supervised,
    /// All operations allowed (default)
    Full,
}

impl Default for AutonomyLevel {
    fn default() -> Self {
        Self::Full
    }
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
}

impl std::fmt::Display for ChannelType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            ChannelType::Telegram => "telegram",
            ChannelType::Slack => "slack",
            ChannelType::Discord => "discord",
            ChannelType::Web => "web",
            ChannelType::Desktop => "desktop",
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
        )
    }
}

impl PartialEq<&str> for ChannelType {
    fn eq(&self, other: &&str) -> bool {
        self == *other
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelConfig {
    pub channel_type: ChannelType,
    #[serde(default)]
    pub token: String,
    #[serde(default)]
    pub bind_addr: Option<String>,
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
}
