use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::error::{KittypawError, Result};
use crate::package::{parse_package_toml, SkillPackage};

/// Load all installed packages from the given packages directory.
/// Returns Vec of (SkillPackage, js_code_string) tuples.
/// Skips packages with corrupt TOML or missing main.js, logging warnings.
pub fn load_all_packages(packages_dir: &Path) -> Result<Vec<(SkillPackage, String)>> {
    let mgr = PackageManager::new(packages_dir.to_path_buf());
    let packages = mgr.list_installed()?;
    let mut result = Vec::new();
    for pkg in packages {
        let js_path = packages_dir.join(&pkg.meta.id).join("main.js");
        match std::fs::read_to_string(&js_path) {
            Ok(js_code) => result.push((pkg, js_code)),
            Err(e) => {
                tracing::warn!("Missing main.js for package '{}': {e}", pkg.meta.id);
            }
        }
    }
    Ok(result)
}

/// Manages installed skill packages in `.kittypaw/packages/`.
pub struct PackageManager {
    packages_dir: PathBuf,
}

impl PackageManager {
    pub fn new(packages_dir: PathBuf) -> Self {
        Self { packages_dir }
    }

    /// Install a package from a source directory (copies `package.toml` + `main.js`).
    pub fn install_package(&self, source_dir: &Path) -> Result<SkillPackage> {
        let source_toml = source_dir.join("package.toml");
        let source_js = source_dir.join("main.js");

        if !source_toml.exists() {
            return Err(KittypawError::Config(
                "Source directory missing package.toml".into(),
            ));
        }
        if !source_js.exists() {
            return Err(KittypawError::Config(
                "Source directory missing main.js".into(),
            ));
        }

        let toml_content = std::fs::read_to_string(&source_toml)?;
        let pkg = parse_package_toml(&toml_content)?;

        let dest = self.packages_dir.join(&pkg.meta.id);
        if dest.exists() {
            return Err(KittypawError::Config(format!(
                "Package '{}' is already installed",
                pkg.meta.id
            )));
        }

        std::fs::create_dir_all(&dest)?;
        std::fs::copy(&source_toml, dest.join("package.toml"))?;
        std::fs::copy(&source_js, dest.join("main.js"))?;

        // Create empty config.toml
        std::fs::write(dest.join("config.toml"), "")?;

        Ok(pkg)
    }

    /// Uninstall a package by ID.
    pub fn uninstall_package(&self, id: &str) -> Result<()> {
        let dir = self.packages_dir.join(id);
        if !dir.exists() {
            return Err(KittypawError::Config(format!(
                "Package '{id}' is not installed"
            )));
        }
        std::fs::remove_dir_all(&dir)?;
        Ok(())
    }

    /// List all installed packages.
    pub fn list_installed(&self) -> Result<Vec<SkillPackage>> {
        let mut packages = Vec::new();

        let entries = match std::fs::read_dir(&self.packages_dir) {
            Ok(e) => e,
            Err(_) => return Ok(packages),
        };

        for entry in entries {
            let entry = entry?;
            if !entry.path().is_dir() {
                continue;
            }
            let toml_path = entry.path().join("package.toml");
            if !toml_path.exists() {
                continue;
            }
            match std::fs::read_to_string(&toml_path) {
                Ok(content) => match parse_package_toml(&content) {
                    Ok(pkg) => packages.push(pkg),
                    Err(e) => {
                        tracing::warn!("Corrupt package.toml in {}: {e}", entry.path().display());
                    }
                },
                Err(e) => {
                    tracing::warn!("Failed to read {}: {e}", toml_path.display());
                }
            }
        }

        Ok(packages)
    }

    /// Load a single package by ID.
    pub fn load_package(&self, id: &str) -> Result<SkillPackage> {
        let toml_path = self.packages_dir.join(id).join("package.toml");
        if !toml_path.exists() {
            return Err(KittypawError::Config(format!("Package '{id}' not found")));
        }
        let content = std::fs::read_to_string(&toml_path)?;
        parse_package_toml(&content)
    }

    /// Get config values for a package.
    /// Secret values stored as `<keychain>` are resolved from the OS keychain.
    pub fn get_config(&self, id: &str) -> Result<HashMap<String, String>> {
        let config_path = self.packages_dir.join(id).join("config.toml");
        if !config_path.exists() {
            return Ok(HashMap::new());
        }
        let content = std::fs::read_to_string(&config_path)?;
        let table: toml::Table = toml::from_str(&content)
            .map_err(|e| KittypawError::Config(format!("Invalid config.toml for '{id}': {e}")))?;
        let mut map = HashMap::new();
        for (k, v) in table {
            if let toml::Value::String(s) = v {
                if s == "<keychain>" {
                    if let Some(secret) = crate::secrets::get_secret(&format!("packages/{id}"), &k)?
                    {
                        map.insert(k, secret);
                    }
                } else {
                    map.insert(k, s);
                }
            }
        }
        Ok(map)
    }

    /// Set a config value for a package.
    pub fn set_config(&self, id: &str, key: &str, value: &str) -> Result<()> {
        let pkg_dir = self.packages_dir.join(id);
        if !pkg_dir.exists() {
            return Err(KittypawError::Config(format!("Package '{id}' not found")));
        }

        let config_path = pkg_dir.join("config.toml");
        let mut table: toml::Table = if config_path.exists() {
            let content = std::fs::read_to_string(&config_path)?;
            toml::from_str(&content).map_err(|e| {
                KittypawError::Config(format!("Invalid config.toml for '{id}': {e}"))
            })?
        } else {
            toml::Table::new()
        };

        // Secret fields are stored in the OS keychain
        let pkg = self.load_package(id)?;
        let is_secret = pkg.config_schema.iter().any(|f| {
            f.key == key && matches!(f.field_type, crate::package::ConfigFieldType::Secret)
        });

        if is_secret {
            crate::secrets::set_secret(&format!("packages/{id}"), key, value)?;
            table.insert(
                key.to_string(),
                toml::Value::String("<keychain>".to_string()),
            );
        } else {
            table.insert(key.to_string(), toml::Value::String(value.to_string()));
        }

        let content = toml::to_string_pretty(&table)
            .map_err(|e| KittypawError::Config(format!("Failed to serialize config: {e}")))?;
        std::fs::write(&config_path, content)?;
        Ok(())
    }

    /// Load a chain of packages for execution.
    /// Returns the packages in chain order with their JS code.
    pub fn load_chain(&self, package: &SkillPackage) -> Result<Vec<(SkillPackage, String)>> {
        let mut chain = Vec::new();
        for step in &package.chain {
            let pkg = self.load_package(&step.package_id)?;
            let js_path = self.packages_dir.join(&step.package_id).join("main.js");
            let js_code = std::fs::read_to_string(&js_path).map_err(KittypawError::Io)?;
            chain.push((pkg, js_code));
        }
        Ok(chain)
    }

    #[cfg(feature = "registry")]
    pub async fn install_from_registry(
        &self,
        client: &crate::registry::RegistryClient,
        entry: &crate::registry::RegistryEntry,
    ) -> Result<SkillPackage> {
        let temp_dir = client.download_package(entry).await?;
        let result = self.install_package(&temp_dir);
        let _ = std::fs::remove_dir_all(&temp_dir);
        result
    }

    /// Get all config values, merging defaults from schema.
    pub fn get_config_with_defaults(&self, id: &str) -> Result<HashMap<String, String>> {
        self.get_config_with_defaults_and_patterns(id, &HashMap::new())
    }

    /// Get all config values, merging defaults from schema, channel globals,
    /// pattern-detected defaults, and saved config (highest priority).
    pub fn get_config_with_defaults_and_patterns(
        &self,
        id: &str,
        pattern_defaults: &HashMap<String, String>,
    ) -> Result<HashMap<String, String>> {
        let pkg = self.load_package(id)?;
        let saved = self.get_config(id)?;

        let mut merged = HashMap::new();

        // Apply schema defaults first
        for field in &pkg.config_schema {
            if let Some(default) = &field.default {
                merged.insert(field.key.clone(), default.clone());
            }
        }

        // Channel global keys that can be provided from Settings
        const CHANNEL_KEYS: &[&str] = &["telegram_token", "chat_id"];

        // Apply channel globals (overrides schema defaults, overridden by per-skill saved config)
        for &key in CHANNEL_KEYS {
            if let Ok(Some(global_val)) = crate::secrets::get_secret("channels", key) {
                if !global_val.is_empty() {
                    merged.insert(key.to_string(), global_val);
                }
            }
        }

        // Apply pattern-detected defaults (overrides channel globals, overridden by saved config)
        merged.extend(pattern_defaults.iter().map(|(k, v)| (k.clone(), v.clone())));

        // Override with saved values (highest priority)
        merged.extend(saved);

        Ok(merged)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    const SAMPLE_TOML: &str = r#"
[package]
id = "test-pkg"
name = "Test Package"
version = "1.0.0"
description = "A test package"
author = "tester"
category = "test"

[permissions]
primitives = ["Http"]

[[config.fields]]
key = "api_key"
label = "API Key"
type = "secret"
required = true

[[config.fields]]
key = "region"
label = "Region"
type = "string"
default = "us-east-1"
"#;

    fn setup() -> (TempDir, PackageManager, TempDir) {
        let packages_tmp = TempDir::new().unwrap();
        let mgr = PackageManager::new(packages_tmp.path().to_path_buf());

        let source_tmp = TempDir::new().unwrap();
        std::fs::write(source_tmp.path().join("package.toml"), SAMPLE_TOML).unwrap();
        std::fs::write(
            source_tmp.path().join("main.js"),
            "export default () => {};",
        )
        .unwrap();

        (packages_tmp, mgr, source_tmp)
    }

    #[test]
    fn test_install_package() {
        let (_packages_tmp, mgr, source_tmp) = setup();
        let pkg = mgr.install_package(source_tmp.path()).unwrap();

        assert_eq!(pkg.meta.id, "test-pkg");
        assert!(mgr.packages_dir.join("test-pkg/package.toml").exists());
        assert!(mgr.packages_dir.join("test-pkg/main.js").exists());
        assert!(mgr.packages_dir.join("test-pkg/config.toml").exists());
    }

    #[test]
    fn test_list_installed() {
        let (_packages_tmp, mgr, source_tmp) = setup();
        mgr.install_package(source_tmp.path()).unwrap();

        let list = mgr.list_installed().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].meta.id, "test-pkg");
    }

    #[test]
    fn test_load_package() {
        let (_packages_tmp, mgr, source_tmp) = setup();
        mgr.install_package(source_tmp.path()).unwrap();

        let pkg = mgr.load_package("test-pkg").unwrap();
        assert_eq!(pkg.meta.name, "Test Package");
    }

    #[test]
    fn test_uninstall_package() {
        let (_packages_tmp, mgr, source_tmp) = setup();
        mgr.install_package(source_tmp.path()).unwrap();
        assert!(mgr.packages_dir.join("test-pkg").exists());

        mgr.uninstall_package("test-pkg").unwrap();
        assert!(!mgr.packages_dir.join("test-pkg").exists());
    }

    #[test]
    fn test_get_set_config() {
        let (_packages_tmp, mgr, source_tmp) = setup();
        mgr.install_package(source_tmp.path()).unwrap();

        // Initially empty
        let config = mgr.get_config("test-pkg").unwrap();
        assert!(config.is_empty());

        // Set a plain value
        mgr.set_config("test-pkg", "region", "eu-west-1").unwrap();
        let config = mgr.get_config("test-pkg").unwrap();
        assert_eq!(config.get("region").unwrap(), "eu-west-1");
    }

    #[test]
    #[ignore = "requires OS keychain access, may prompt for permission"]
    fn test_set_secret_config() {
        let (_packages_tmp, mgr, source_tmp) = setup();
        mgr.install_package(source_tmp.path()).unwrap();

        // Set a secret value — stored in keychain, config.toml gets `<keychain>` marker
        mgr.set_config("test-pkg", "api_key", "sk-123").unwrap();
        let raw_config =
            std::fs::read_to_string(mgr.packages_dir.join("test-pkg").join("config.toml")).unwrap();
        assert!(
            raw_config.contains("<keychain>"),
            "Secret field should store <keychain> marker in config.toml"
        );
        // get_config resolves from keychain
        let config = mgr.get_config("test-pkg").unwrap();
        assert_eq!(config.get("api_key").unwrap(), "sk-123");
    }

    #[test]
    fn test_config_with_defaults() {
        let (_packages_tmp, mgr, source_tmp) = setup();
        mgr.install_package(source_tmp.path()).unwrap();

        // Before setting anything, defaults should appear
        let config = mgr.get_config_with_defaults("test-pkg").unwrap();
        assert_eq!(config.get("region").unwrap(), "us-east-1");

        // Set overrides the default
        mgr.set_config("test-pkg", "region", "ap-northeast-1")
            .unwrap();
        let config = mgr.get_config_with_defaults("test-pkg").unwrap();
        assert_eq!(config.get("region").unwrap(), "ap-northeast-1");
    }

    #[test]
    fn test_install_duplicate_id_errors() {
        let (_packages_tmp, mgr, source_tmp) = setup();
        mgr.install_package(source_tmp.path()).unwrap();

        let err = mgr.install_package(source_tmp.path()).unwrap_err();
        assert!(
            err.to_string().contains("already installed"),
            "Expected duplicate error, got: {err}"
        );
    }

    #[test]
    fn test_install_missing_package_toml_errors() {
        let (_packages_tmp, mgr, _source_tmp) = setup();

        let empty = TempDir::new().unwrap();
        std::fs::write(empty.path().join("main.js"), "// js").unwrap();

        let err = mgr.install_package(empty.path()).unwrap_err();
        assert!(
            err.to_string().contains("package.toml"),
            "Expected missing package.toml error, got: {err}"
        );
    }

    #[test]
    fn test_install_missing_main_js_errors() {
        let (_packages_tmp, mgr, _source_tmp) = setup();

        let no_js = TempDir::new().unwrap();
        std::fs::write(no_js.path().join("package.toml"), SAMPLE_TOML).unwrap();

        let err = mgr.install_package(no_js.path()).unwrap_err();
        assert!(
            err.to_string().contains("main.js"),
            "Expected missing main.js error, got: {err}"
        );
    }

    #[test]
    fn test_uninstall_nonexistent_errors() {
        let (_packages_tmp, mgr, _source_tmp) = setup();
        let err = mgr.uninstall_package("ghost").unwrap_err();
        assert!(err.to_string().contains("not installed"));
    }

    #[test]
    fn test_load_nonexistent_errors() {
        let (_packages_tmp, mgr, _source_tmp) = setup();
        let err = mgr.load_package("ghost").unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[test]
    fn test_load_all_packages() {
        let (packages_tmp, mgr, source_tmp) = setup();
        mgr.install_package(source_tmp.path()).unwrap();

        let loaded = load_all_packages(packages_tmp.path()).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].0.meta.id, "test-pkg");
        assert_eq!(loaded[0].1, "export default () => {};");
    }

    #[test]
    fn test_load_all_packages_empty_dir() {
        let tmp = TempDir::new().unwrap();
        let loaded = load_all_packages(tmp.path()).unwrap();
        assert!(loaded.is_empty());
    }

    #[test]
    fn test_load_all_packages_skips_missing_js() {
        let packages_tmp = TempDir::new().unwrap();
        let pkg_dir = packages_tmp.path().join("no-js-pkg");
        std::fs::create_dir_all(&pkg_dir).unwrap();

        let toml_content = r#"
[package]
id = "no-js-pkg"
name = "No JS"
version = "1.0.0"
description = "Missing JS"
author = "test"
category = "test"

[permissions]
primitives = []
"#;
        std::fs::write(pkg_dir.join("package.toml"), toml_content).unwrap();
        // Intentionally no main.js

        let loaded = load_all_packages(packages_tmp.path()).unwrap();
        assert!(
            loaded.is_empty(),
            "Package with missing main.js should be skipped"
        );
    }

    #[test]
    fn test_load_chain() {
        let packages_tmp = TempDir::new().unwrap();
        let mgr = PackageManager::new(packages_tmp.path().to_path_buf());

        // Create chain target package "step-b"
        let step_b_dir = packages_tmp.path().join("step-b");
        std::fs::create_dir_all(&step_b_dir).unwrap();
        std::fs::write(
            step_b_dir.join("package.toml"),
            r#"
[package]
id = "step-b"
name = "Step B"
version = "1.0.0"
description = "Second step"
author = "test"
category = "test"

[permissions]
primitives = []
"#,
        )
        .unwrap();
        std::fs::write(step_b_dir.join("main.js"), "// step b code").unwrap();

        // Create parent package "step-a" with chain pointing to "step-b"
        let step_a_toml = r#"
[package]
id = "step-a"
name = "Step A"
version = "1.0.0"
description = "First step"
author = "test"
category = "test"

[permissions]
primitives = []

[[chain]]
package = "step-b"
"#;
        let pkg = crate::package::parse_package_toml(step_a_toml).unwrap();
        assert_eq!(pkg.chain.len(), 1);
        assert_eq!(pkg.chain[0].package_id, "step-b");

        let chain = mgr.load_chain(&pkg).unwrap();
        assert_eq!(chain.len(), 1);
        assert_eq!(chain[0].0.meta.id, "step-b");
        assert_eq!(chain[0].1, "// step b code");
    }
}
