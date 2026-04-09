use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use kittypaw_core::config::ModelConfig;
use kittypaw_core::error::Result;
use kittypaw_core::types::LlmMessage;

use crate::claude::ClaudeProvider;
use crate::openai::OpenAiProvider;
use crate::provider::{LlmProvider, LlmResponse};

/// Wraps an LlmProvider with a config-overridden context window.
struct ConfiguredProvider {
    inner: Arc<dyn LlmProvider>,
    context_window_override: usize,
}

#[async_trait]
impl LlmProvider for ConfiguredProvider {
    async fn generate(&self, messages: &[LlmMessage]) -> Result<LlmResponse> {
        self.inner.generate(messages).await
    }

    async fn generate_stream(
        &self,
        messages: &[LlmMessage],
        on_token: Arc<dyn Fn(String) + Send + Sync>,
    ) -> Result<LlmResponse> {
        self.inner.generate_stream(messages, on_token).await
    }

    fn context_window(&self) -> usize {
        self.context_window_override
    }

    fn max_tokens(&self) -> usize {
        self.inner.max_tokens()
    }
}

pub struct LlmRegistry {
    providers: HashMap<String, Arc<dyn LlmProvider>>,
    insertion_order: Vec<String>,
    default_name: String,
}

impl LlmRegistry {
    pub fn new() -> Self {
        Self {
            providers: HashMap::new(),
            insertion_order: Vec::new(),
            default_name: String::new(),
        }
    }

    /// Register a provider under a name (e.g. "claude-sonnet", "gpt-4o").
    /// The first registered provider becomes the default.
    pub fn register(&mut self, name: &str, provider: Arc<dyn LlmProvider>) {
        if self.default_name.is_empty() {
            self.default_name = name.to_string();
        }
        if !self.providers.contains_key(name) {
            self.insertion_order.push(name.to_string());
        }
        self.providers.insert(name.to_string(), provider);
    }

    /// Set the default provider name.
    pub fn set_default(&mut self, name: &str) {
        self.default_name = name.to_string();
    }

    /// Get a provider by name.
    pub fn get(&self, name: &str) -> Option<Arc<dyn LlmProvider>> {
        self.providers.get(name).cloned()
    }

    /// Get the default provider.
    pub fn default_provider(&self) -> Option<Arc<dyn LlmProvider>> {
        self.providers.get(&self.default_name).cloned()
    }

    /// Get the first registered provider (by insertion order) that is NOT the default.
    /// Returns None if there is only one provider or no providers.
    pub fn fallback_provider(&self) -> Option<Arc<dyn LlmProvider>> {
        self.insertion_order
            .iter()
            .find(|name| *name != &self.default_name)
            .and_then(|name| self.providers.get(name).cloned())
    }

    /// List registered provider names.
    pub fn list(&self) -> Vec<String> {
        self.providers.keys().cloned().collect()
    }

    /// Build a registry from model configs.
    /// If api_key is empty in config, tries the local secret store via kittypaw_core::secrets.
    /// Models without a resolvable API key are skipped.
    pub fn from_configs(configs: &[ModelConfig]) -> Self {
        let mut registry = Self::new();
        for cfg in configs {
            let api_key = if cfg.api_key.is_empty() {
                kittypaw_core::secrets::get_secret("models", &cfg.name)
                    .ok()
                    .flatten()
                    .unwrap_or_default()
            } else {
                cfg.api_key.clone()
            };

            let provider: Arc<dyn LlmProvider> = match cfg.provider.as_str() {
                "claude" | "anthropic" => {
                    if api_key.is_empty() {
                        continue;
                    }
                    Arc::new(ClaudeProvider::new(
                        api_key,
                        cfg.model.clone(),
                        cfg.max_tokens,
                    ))
                }
                "openai" => {
                    if api_key.is_empty() {
                        continue;
                    }
                    if let Some(ref base_url) = cfg.base_url {
                        if let Err(e) = validate_llm_base_url(base_url) {
                            tracing::warn!("Skipping model '{}': {e}", cfg.name);
                            continue;
                        }
                        // Only send API keys to trusted first-party hosts
                        let safe_key = if is_trusted_llm_host(base_url) {
                            api_key.clone()
                        } else {
                            tracing::warn!(
                                "base_url '{}' is not a trusted provider; API key will NOT be sent. \
                                 Use provider = \"ollama\" for local models.",
                                base_url
                            );
                            String::new()
                        };
                        Arc::new(OpenAiProvider::with_base_url(
                            base_url.clone(),
                            safe_key,
                            cfg.model.clone(),
                            cfg.max_tokens,
                        ))
                    } else {
                        Arc::new(OpenAiProvider::new(
                            api_key,
                            cfg.model.clone(),
                            cfg.max_tokens,
                        ))
                    }
                }
                "ollama" | "local" => {
                    let base_url = cfg
                        .base_url
                        .clone()
                        .unwrap_or_else(|| "http://localhost:11434/v1".to_string());
                    if let Err(e) = validate_llm_base_url(&base_url) {
                        tracing::warn!("Skipping model '{}': {e}", cfg.name);
                        continue;
                    }
                    Arc::new(OpenAiProvider::with_base_url(
                        base_url,
                        String::new(),
                        cfg.model.clone(),
                        cfg.max_tokens,
                    ))
                }
                _ => continue,
            };

            // Wrap with config-overridden context window if specified
            let provider = if let Some(cw) = cfg.context_window {
                Arc::new(ConfiguredProvider {
                    inner: provider,
                    context_window_override: cw as usize,
                }) as Arc<dyn LlmProvider>
            } else {
                provider
            };

            registry.register(&cfg.name, provider);
            if cfg.default {
                registry.set_default(&cfg.name);
            }
        }
        registry
    }
}

impl Default for LlmRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Check if a base_url points to a trusted first-party LLM provider
/// where it is safe to send API keys.
fn is_trusted_llm_host(base_url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(base_url) else {
        return false;
    };
    let Some(host) = parsed.host_str() else {
        return false;
    };
    is_trusted_domain(host, "openai.com")
        || is_trusted_domain(host, "anthropic.com")
        || is_trusted_domain(host, "azure.com")
        || is_trusted_domain(host, "openrouter.ai")
}

fn is_trusted_domain(host: &str, domain: &str) -> bool {
    host == domain || host.ends_with(&format!(".{domain}"))
}

/// Validate a base_url for LLM provider use.
/// Blocks non-HTTP schemes and cloud metadata endpoints.
pub fn validate_llm_base_url(base_url: &str) -> std::result::Result<(), String> {
    let parsed = url::Url::parse(base_url).map_err(|_| "Invalid base_url".to_string())?;

    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("base_url must use http or https".into());
    }

    let host = parsed
        .host_str()
        .ok_or_else(|| "base_url has no host".to_string())?;

    // Block cloud metadata endpoints
    if matches!(host, "metadata.google.internal" | "169.254.169.254") {
        return Err("base_url cannot point to cloud metadata service".into());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use kittypaw_core::error::Result;
    use kittypaw_core::types::LlmMessage;

    struct MockProvider;

    #[async_trait]
    impl LlmProvider for MockProvider {
        async fn generate(&self, _messages: &[LlmMessage]) -> Result<LlmResponse> {
            Ok(LlmResponse::text_only("mock response".into()))
        }
    }

    struct MockProviderB;

    #[async_trait]
    impl LlmProvider for MockProviderB {
        async fn generate(&self, _messages: &[LlmMessage]) -> Result<LlmResponse> {
            Ok(LlmResponse::text_only("mock response B".into()))
        }
    }

    #[test]
    fn test_register_and_get() {
        let mut registry = LlmRegistry::new();
        let provider: Arc<dyn LlmProvider> = Arc::new(MockProvider);
        registry.register("test-model", provider);

        assert!(registry.get("test-model").is_some());
    }

    #[test]
    fn test_default_provider() {
        let mut registry = LlmRegistry::new();
        let provider: Arc<dyn LlmProvider> = Arc::new(MockProvider);
        registry.register("first", provider);

        let provider_b: Arc<dyn LlmProvider> = Arc::new(MockProviderB);
        registry.register("second", provider_b);

        // First registered becomes default
        assert!(registry.default_provider().is_some());
    }

    #[test]
    fn test_set_default() {
        let mut registry = LlmRegistry::new();
        let provider: Arc<dyn LlmProvider> = Arc::new(MockProvider);
        registry.register("first", provider);

        let provider_b: Arc<dyn LlmProvider> = Arc::new(MockProviderB);
        registry.register("second", provider_b);

        registry.set_default("second");
        assert!(registry.default_provider().is_some());
    }

    #[test]
    fn test_list() {
        let mut registry = LlmRegistry::new();
        let provider: Arc<dyn LlmProvider> = Arc::new(MockProvider);
        registry.register("alpha", provider);

        let provider_b: Arc<dyn LlmProvider> = Arc::new(MockProviderB);
        registry.register("beta", provider_b);

        let mut names = registry.list();
        names.sort();
        assert_eq!(names, vec!["alpha", "beta"]);
    }

    #[test]
    fn test_get_nonexistent() {
        let registry = LlmRegistry::new();
        assert!(registry.get("nonexistent").is_none());
    }

    #[test]
    fn test_from_configs_skips_empty_key() {
        let configs = vec![ModelConfig {
            name: "test".into(),
            provider: "claude".into(),
            model: "test-model".into(),
            api_key: String::new(),
            max_tokens: 1024,
            default: false,
            base_url: None,
            context_window: None,
            tier: None,
        }];
        let registry = LlmRegistry::from_configs(&configs);
        assert!(registry.list().is_empty());
    }

    #[test]
    fn test_from_configs_ollama_no_key_needed() {
        let configs = vec![ModelConfig {
            name: "local-qwen".into(),
            provider: "ollama".into(),
            model: "qwen3.5:27b".into(),
            api_key: String::new(),
            max_tokens: 4096,
            default: true,
            base_url: None,
            context_window: None,
            tier: None,
        }];
        let registry = LlmRegistry::from_configs(&configs);
        assert_eq!(registry.list().len(), 1);
        assert!(registry.get("local-qwen").is_some());
    }

    #[test]
    fn test_is_trusted_llm_host() {
        assert!(super::is_trusted_llm_host("https://api.openai.com/v1"));
        assert!(super::is_trusted_llm_host("https://models.azure.com/v1"));
        assert!(!super::is_trusted_llm_host("http://evil.com/v1"));
        assert!(!super::is_trusted_llm_host("http://localhost:11434/v1"));
        // OpenRouter
        assert!(super::is_trusted_llm_host("https://openrouter.ai/api/v1"));
        // Subdomain spoofing must not match
        assert!(!super::is_trusted_llm_host("https://evil-openai.com/v1"));
        assert!(!super::is_trusted_llm_host("https://notrealopenai.com/v1"));
        assert!(!super::is_trusted_llm_host("https://evil-openrouter.ai/v1"));
    }

    #[test]
    fn test_validate_llm_base_url_blocks_metadata() {
        assert!(super::validate_llm_base_url("http://169.254.169.254/latest").is_err());
        assert!(super::validate_llm_base_url("http://metadata.google.internal/v1").is_err());
    }

    #[test]
    fn test_validate_llm_base_url_blocks_non_http() {
        assert!(super::validate_llm_base_url("ftp://localhost:11434/v1").is_err());
        assert!(super::validate_llm_base_url("file:///etc/passwd").is_err());
    }

    #[test]
    fn test_validate_llm_base_url_allows_valid() {
        assert!(super::validate_llm_base_url("http://localhost:11434/v1").is_ok());
        assert!(super::validate_llm_base_url("http://127.0.0.1:8080/v1").is_ok());
        assert!(super::validate_llm_base_url("https://api.openai.com/v1").is_ok());
    }

    #[test]
    fn test_configured_provider_overrides_context_window() {
        let configs = vec![ModelConfig {
            name: "local-qwen".into(),
            provider: "ollama".into(),
            model: "qwen3.5:27b".into(),
            api_key: String::new(),
            max_tokens: 4096,
            default: true,
            base_url: None,
            context_window: Some(32768),
            tier: None,
        }];
        let registry = LlmRegistry::from_configs(&configs);
        let provider = registry.default_provider().unwrap();
        assert_eq!(provider.context_window(), 32768);
    }

    #[test]
    fn test_fallback_provider_returns_non_default() {
        let mut registry = LlmRegistry::new();
        let provider_a: Arc<dyn LlmProvider> = Arc::new(MockProvider);
        let provider_b: Arc<dyn LlmProvider> = Arc::new(MockProviderB);
        registry.register("alpha", provider_a);
        registry.register("beta", provider_b);
        // "alpha" is default (first registered)
        assert!(registry.fallback_provider().is_some());
    }

    #[test]
    fn test_fallback_provider_none_when_single() {
        let mut registry = LlmRegistry::new();
        let provider: Arc<dyn LlmProvider> = Arc::new(MockProvider);
        registry.register("only", provider);
        assert!(registry.fallback_provider().is_none());
    }

    #[test]
    fn test_no_context_window_uses_provider_default() {
        let configs = vec![ModelConfig {
            name: "local-qwen".into(),
            provider: "ollama".into(),
            model: "qwen3.5:27b".into(),
            api_key: String::new(),
            max_tokens: 4096,
            default: true,
            base_url: None,
            context_window: None,
            tier: None,
        }];
        let registry = LlmRegistry::from_configs(&configs);
        let provider = registry.default_provider().unwrap();
        assert_eq!(provider.context_window(), 8_192); // OpenAiProvider default for unknown model
    }
}
