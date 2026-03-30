use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::{KittypawError, Result};

const INDEX_URL: &str =
    "https://raw.githubusercontent.com/kittypaw-skills/registry/main/index.json";
const REQUEST_TIMEOUT_SECS: u64 = 10;

/// Allowed URL prefixes for download_url. Prevents SSRF to localhost/private networks.
const ALLOWED_URL_PREFIXES: &[&str] = &[
    "https://raw.githubusercontent.com/kittypaw-skills/",
    "https://github.com/kittypaw-skills/",
];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RegistryEntry {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub author: String,
    pub category: String,
    pub tags: Vec<String>,
    pub download_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryIndex {
    pub version: u32,
    pub packages: Vec<RegistryEntry>,
}

pub struct RegistryClient {
    client: reqwest::Client,
    index_url: String,
    cache_path: PathBuf,
}

/// Validate that a package ID is safe for filesystem use.
/// Must be non-empty, ASCII alphanumeric + hyphens only, no path separators.
fn validate_package_id(id: &str) -> Result<()> {
    if id.is_empty() {
        return Err(KittypawError::Config("Package ID cannot be empty".into()));
    }
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(KittypawError::Config(format!(
            "Invalid package ID '{id}': only alphanumeric, hyphens, and underscores allowed"
        )));
    }
    if id.starts_with('-') || id.starts_with('_') {
        return Err(KittypawError::Config(format!(
            "Invalid package ID '{id}': cannot start with hyphen or underscore"
        )));
    }
    Ok(())
}

/// Validate that a download URL is from an allowed host.
fn validate_download_url(url: &str) -> Result<()> {
    if !ALLOWED_URL_PREFIXES
        .iter()
        .any(|prefix| url.starts_with(prefix))
    {
        return Err(KittypawError::Network(format!(
            "Blocked download URL '{url}': must be from an allowed registry host"
        )));
    }
    Ok(())
}

impl RegistryClient {
    pub fn new(cache_dir: &Path) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .redirect(reqwest::redirect::Policy::none()) // prevent redirect-based SSRF
            .build()
            .unwrap_or_default();
        Self {
            client,
            index_url: INDEX_URL.to_string(),
            cache_path: cache_dir.join("registry_index.json"),
        }
    }

    /// Fetch the registry index from the remote URL, caching on success.
    /// Falls back to the local cache on any fetch or parse error.
    pub async fn fetch_index(&self) -> Result<RegistryIndex> {
        match self.fetch_index_remote().await {
            Ok(index) => Ok(index),
            Err(e) => {
                tracing::warn!("Failed to fetch registry index: {e}. Falling back to cache.");
                self.load_cache()
            }
        }
    }

    async fn fetch_index_remote(&self) -> Result<RegistryIndex> {
        let response = self
            .client
            .get(&self.index_url)
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(|e| KittypawError::Network(e.to_string()))?;

        let index: RegistryIndex = response
            .json()
            .await
            .map_err(|e| KittypawError::Network(e.to_string()))?;

        if let Some(parent) = self.cache_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(&index) {
            let _ = std::fs::write(&self.cache_path, json);
        }

        Ok(index)
    }

    /// Download a package's `package.toml` and `main.js` in parallel to a temp directory.
    /// Validates the download URL against an allowlist and the package ID against path traversal.
    pub async fn download_package(&self, entry: &RegistryEntry) -> Result<PathBuf> {
        validate_package_id(&entry.id)?;
        validate_download_url(&entry.download_url)?;

        let temp_dir = std::env::temp_dir().join(format!("kittypaw-pkg-{}", entry.id));
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir)?;

        let base = entry.download_url.trim_end_matches('/');
        let toml_url = format!("{base}/package.toml");
        let js_url = format!("{base}/main.js");

        let (toml_resp, js_resp) = tokio::try_join!(
            self.client.get(&toml_url).send(),
            self.client.get(&js_url).send(),
        )
        .map_err(|e| KittypawError::Network(e.to_string()))?;

        let toml_resp = toml_resp
            .error_for_status()
            .map_err(|e| KittypawError::Network(e.to_string()))?;
        let js_resp = js_resp
            .error_for_status()
            .map_err(|e| KittypawError::Network(e.to_string()))?;

        let (toml_content, js_content) = tokio::try_join!(toml_resp.text(), js_resp.text())
            .map_err(|e| KittypawError::Network(e.to_string()))?;

        std::fs::write(temp_dir.join("package.toml"), &toml_content)?;
        std::fs::write(temp_dir.join("main.js"), js_content)?;

        // Verify the downloaded package.toml has the expected ID (prevents bait-and-switch)
        let downloaded_pkg = crate::package::parse_package_toml(&toml_content)
            .map_err(|e| KittypawError::Config(format!("Invalid remote package.toml: {e}")))?;
        if downloaded_pkg.meta.id != entry.id {
            let _ = std::fs::remove_dir_all(&temp_dir);
            return Err(KittypawError::Config(format!(
                "Package ID mismatch: expected '{}', got '{}'",
                entry.id, downloaded_pkg.meta.id
            )));
        }

        Ok(temp_dir)
    }

    fn load_cache(&self) -> Result<RegistryIndex> {
        if !self.cache_path.exists() {
            return Err(KittypawError::Network(
                "Registry index unavailable: no network and no cache".into(),
            ));
        }
        let content = std::fs::read_to_string(&self.cache_path)?;
        let index: RegistryIndex = serde_json::from_str(&content)?;
        Ok(index)
    }
}
