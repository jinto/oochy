use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::skill::SkillTrigger;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Skill package metadata parsed from `package.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillPackage {
    pub meta: PackageMeta,
    pub config_schema: Vec<ConfigField>,
    pub permissions: PackagePermissions,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trigger: Option<SkillTrigger>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageMeta {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub author: String,
    pub category: String,
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigField {
    pub key: String,
    pub label: String,
    #[serde(rename = "type")]
    pub field_type: ConfigFieldType,
    #[serde(default)]
    pub required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigFieldType {
    String,
    Secret,
    Number,
    Boolean,
    Cron,
    Select,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackagePermissions {
    pub primitives: Vec<String>,
    #[serde(default)]
    pub allowed_hosts: Vec<String>,
}

impl SkillPackage {
    /// Build a sandbox context JSON that includes package config values.
    /// JS code accesses these via `JSON.parse(__context__).config.key_name`.
    pub fn build_context(
        &self,
        config_values: &HashMap<String, String>,
        event_payload: serde_json::Value,
    ) -> serde_json::Value {
        let config_json: serde_json::Value = config_values
            .iter()
            .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
            .collect::<serde_json::Map<String, serde_json::Value>>()
            .into();

        serde_json::json!({
            "event": event_payload,
            "config": config_json,
            "package_id": self.meta.id,
        })
    }
}

// ---------------------------------------------------------------------------
// Intermediate TOML structs (for deserialization of package.toml)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub(crate) struct PackageToml {
    pub package: PackageMeta,
    pub trigger: Option<SkillTrigger>,
    pub permissions: PackagePermissions,
    pub config: Option<PackageConfigToml>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct PackageConfigToml {
    #[serde(default)]
    pub fields: Vec<ConfigField>,
}

impl PackageToml {
    /// Parse a `package.toml` string into the intermediate representation.
    pub fn parse(s: &str) -> crate::error::Result<Self> {
        toml::from_str(s)
            .map_err(|e| crate::error::KittypawError::Config(format!("Invalid package.toml: {e}")))
    }

    /// Convert the intermediate TOML representation into a [`SkillPackage`].
    pub fn into_package(self) -> SkillPackage {
        SkillPackage {
            meta: self.package,
            config_schema: self.config.map(|c| c.fields).unwrap_or_default(),
            permissions: self.permissions,
            trigger: self.trigger,
        }
    }
}

/// Parse a `package.toml` string directly into a [`SkillPackage`].
pub fn parse_package_toml(s: &str) -> crate::error::Result<SkillPackage> {
    Ok(PackageToml::parse(s)?.into_package())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_TOML: &str = r#"
[package]
id = "macro-economy-report"
name = "Macro Economy Report"
version = "1.0.0"
description = "Daily macro-economic briefing via Telegram"
author = "KittyPaw Team"
category = "finance"
tags = ["macro", "economy", "telegram"]

[trigger]
type = "schedule"
cron = "0 8 * * *"

[permissions]
primitives = ["Http", "Telegram", "Llm", "Storage"]
allowed_hosts = ["api.telegram.org", "fred.stlouisfed.org"]

[[config.fields]]
key = "telegram_token"
label = "Telegram Bot Token"
type = "secret"
required = true

[[config.fields]]
key = "chat_id"
label = "Telegram Chat ID"
type = "string"
required = true

[[config.fields]]
key = "tickers"
label = "Stock Tickers"
type = "string"
default = "SPY,QQQ,TLT,GLD"
hint = "Comma-separated ticker symbols"
"#;

    #[test]
    fn test_parse_sample_package_toml() {
        let pkg = parse_package_toml(SAMPLE_TOML).unwrap();
        assert_eq!(pkg.meta.id, "macro-economy-report");
        assert_eq!(pkg.meta.name, "Macro Economy Report");
        assert_eq!(pkg.meta.version, "1.0.0");
        assert_eq!(pkg.meta.author, "KittyPaw Team");
        assert_eq!(pkg.meta.category, "finance");
        assert_eq!(pkg.meta.tags, vec!["macro", "economy", "telegram"]);

        let trigger = pkg.trigger.unwrap();
        assert_eq!(trigger.trigger_type, "schedule");
        assert_eq!(trigger.cron.unwrap(), "0 8 * * *");

        assert_eq!(
            pkg.permissions.primitives,
            vec!["Http", "Telegram", "Llm", "Storage"]
        );
        assert_eq!(
            pkg.permissions.allowed_hosts,
            vec!["api.telegram.org", "fred.stlouisfed.org"]
        );

        assert_eq!(pkg.config_schema.len(), 3);
        assert_eq!(pkg.config_schema[0].key, "telegram_token");
        assert!(matches!(
            pkg.config_schema[0].field_type,
            ConfigFieldType::Secret
        ));
        assert!(pkg.config_schema[0].required);

        assert_eq!(
            pkg.config_schema[2].default.as_deref(),
            Some("SPY,QQQ,TLT,GLD")
        );
        assert_eq!(
            pkg.config_schema[2].hint.as_deref(),
            Some("Comma-separated ticker symbols")
        );
    }

    #[test]
    fn test_round_trip_serialize_deserialize() {
        let pkg = parse_package_toml(SAMPLE_TOML).unwrap();
        let json = serde_json::to_string(&pkg).unwrap();
        let restored: SkillPackage = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.meta.id, pkg.meta.id);
        assert_eq!(restored.config_schema.len(), pkg.config_schema.len());
        assert_eq!(restored.permissions.primitives, pkg.permissions.primitives);
    }

    #[test]
    fn test_missing_optional_fields() {
        let minimal = r#"
[package]
id = "minimal"
name = "Minimal"
version = "0.1.0"
description = "Bare minimum"
author = "test"
category = "misc"

[permissions]
primitives = []
"#;
        let pkg = parse_package_toml(minimal).unwrap();
        assert_eq!(pkg.meta.id, "minimal");
        assert!(pkg.meta.tags.is_empty());
        assert!(pkg.trigger.is_none());
        assert!(pkg.config_schema.is_empty());
        assert!(pkg.permissions.allowed_hosts.is_empty());
    }

    #[test]
    fn test_select_field_with_options() {
        let toml_str = r#"
[package]
id = "select-test"
name = "Select Test"
version = "0.1.0"
description = "Test select field"
author = "test"
category = "misc"

[permissions]
primitives = []

[[config.fields]]
key = "region"
label = "Region"
type = "select"
required = true
options = ["us-east-1", "eu-west-1", "ap-northeast-1"]
"#;
        let pkg = parse_package_toml(toml_str).unwrap();
        assert_eq!(pkg.config_schema.len(), 1);
        assert!(matches!(
            pkg.config_schema[0].field_type,
            ConfigFieldType::Select
        ));
        assert_eq!(
            pkg.config_schema[0].options.as_deref().unwrap(),
            &["us-east-1", "eu-west-1", "ap-northeast-1"]
        );
    }

    #[test]
    fn test_build_context() {
        let pkg = parse_package_toml(SAMPLE_TOML).unwrap();
        let mut config = HashMap::new();
        config.insert("telegram_token".to_string(), "tok123".to_string());
        config.insert("chat_id".to_string(), "42".to_string());

        let event = serde_json::json!({"event_type": "schedule"});
        let ctx = pkg.build_context(&config, event.clone());

        assert_eq!(ctx["package_id"], "macro-economy-report");
        assert_eq!(ctx["event"], event);
        assert_eq!(ctx["config"]["telegram_token"], "tok123");
        assert_eq!(ctx["config"]["chat_id"], "42");
    }

    #[test]
    fn test_build_context_empty_config() {
        let pkg = parse_package_toml(SAMPLE_TOML).unwrap();
        let config = HashMap::new();
        let event = serde_json::json!({"event_type": "schedule"});
        let ctx = pkg.build_context(&config, event);

        assert_eq!(ctx["package_id"], "macro-economy-report");
        assert!(ctx["config"].as_object().unwrap().is_empty());
    }
}
