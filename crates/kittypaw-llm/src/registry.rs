use std::collections::HashMap;
use std::sync::Arc;

use crate::provider::LlmProvider;

pub struct LlmRegistry {
    providers: HashMap<String, Arc<dyn LlmProvider>>,
    default_name: String,
}

impl LlmRegistry {
    pub fn new() -> Self {
        Self {
            providers: HashMap::new(),
            default_name: String::new(),
        }
    }

    /// Register a provider under a name (e.g. "claude-sonnet", "gpt-4o").
    /// The first registered provider becomes the default.
    pub fn register(&mut self, name: &str, provider: Arc<dyn LlmProvider>) {
        if self.default_name.is_empty() {
            self.default_name = name.to_string();
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

    /// List registered provider names.
    pub fn list(&self) -> Vec<String> {
        self.providers.keys().cloned().collect()
    }
}

impl Default for LlmRegistry {
    fn default() -> Self {
        Self::new()
    }
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
        async fn generate(&self, _messages: &[LlmMessage]) -> Result<String> {
            Ok("mock response".into())
        }
    }

    struct MockProviderB;

    #[async_trait]
    impl LlmProvider for MockProviderB {
        async fn generate(&self, _messages: &[LlmMessage]) -> Result<String> {
            Ok("mock response B".into())
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
}
