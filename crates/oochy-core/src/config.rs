use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::error::{OochyError, Result};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub llm: LlmConfig,
    pub sandbox: SandboxConfig,
    #[serde(default)]
    pub agents: Vec<AgentConfig>,
    #[serde(default)]
    pub channels: Vec<ChannelConfig>,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelConfig {
    pub channel_type: String, // "telegram", "discord", "web"
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
        // Layer 1: Try oochy.toml
        let config_path = PathBuf::from("oochy.toml");
        let mut config = if config_path.exists() {
            let content = std::fs::read_to_string(&config_path)?;
            toml::from_str(&content)
                .map_err(|e| OochyError::Config(format!("Invalid oochy.toml: {e}")))?
        } else {
            Config::default()
        };

        // Layer 2: Override with env vars
        if let Ok(key) = std::env::var("OOCHY_API_KEY") {
            config.llm.api_key = key;
        }
        if let Ok(provider) = std::env::var("OOCHY_LLM_PROVIDER") {
            config.llm.provider = provider;
        }
        if let Ok(model) = std::env::var("OOCHY_MODEL") {
            config.llm.model = model;
        }

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
        }
    }
}
