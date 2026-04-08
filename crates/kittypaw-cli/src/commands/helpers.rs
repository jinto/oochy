use kittypaw_core::config::{Config, ModelConfig};
use kittypaw_llm::provider::LlmProvider;
use kittypaw_llm::registry::LlmRegistry;
use std::sync::Arc;

pub(crate) fn db_path() -> String {
    // Override: KITTYPAW_DB_PATH > KITTYPAW_HOME/kittypaw.db > "kittypaw.db"
    //
    // Migration note: prior to this change the default was `./kittypaw.db` (CWD-relative).
    // If you have existing data there, move it:
    //   mv ./kittypaw.db ~/.kittypaw/kittypaw.db
    // Or set KITTYPAW_DB_PATH to the old path to keep using it.
    std::env::var("KITTYPAW_DB_PATH").unwrap_or_else(|_| {
        kittypaw_core::secrets::data_dir()
            .map(|d| d.join("kittypaw.db").to_string_lossy().into_owned())
            .unwrap_or_else(|_| "kittypaw.db".into())
    })
}

/// Build an LlmRegistry from config.
/// Uses `[[models]]` if configured, otherwise falls back to the legacy `[llm]` section.
pub(crate) fn build_registry(config: &Config) -> LlmRegistry {
    if !config.models.is_empty() {
        let mut models = config.models.clone();
        // Inject global api_key as fallback for models that require one but don't have it
        if !config.llm.api_key.is_empty() {
            for model in &mut models {
                if model.api_key.is_empty()
                    && matches!(model.provider.as_str(), "claude" | "anthropic" | "openai")
                {
                    model.api_key = config.llm.api_key.clone();
                }
            }
        }
        LlmRegistry::from_configs(&models)
    } else if !config.llm.api_key.is_empty() {
        let legacy = ModelConfig {
            name: config.llm.provider.clone(),
            provider: config.llm.provider.clone(),
            model: config.llm.model.clone(),
            api_key: config.llm.api_key.clone(),
            max_tokens: config.llm.max_tokens,
            default: true,
            base_url: None,
            context_window: None,
        };
        LlmRegistry::from_configs(&[legacy])
    } else {
        LlmRegistry::new()
    }
}

/// Build a registry and return the default + fallback providers.
pub(crate) fn require_provider_with_fallback(
    config: &Config,
) -> (Arc<dyn LlmProvider>, Option<Arc<dyn LlmProvider>>) {
    let registry = build_registry(config);
    let default = registry.default_provider().unwrap_or_else(|| {
        eprintln!("Error: No LLM provider configured. Set KITTYPAW_API_KEY or add [[models]] to kittypaw.toml.");
        std::process::exit(1);
    });
    let fallback = registry.fallback_provider();
    if fallback.is_some() {
        tracing::info!("Fallback LLM provider available");
    }
    (default, fallback)
}

/// Build a registry and return the default provider, or exit with an error message.
pub(crate) fn require_provider(config: &Config) -> Arc<dyn LlmProvider> {
    require_provider_with_fallback(config).0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // env vars are process-wide — serialize to avoid parallel test races
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn db_path_explicit_env_overrides_all() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("KITTYPAW_HOME");
        std::env::set_var("KITTYPAW_DB_PATH", "/tmp/explicit.db");
        let path = db_path();
        std::env::remove_var("KITTYPAW_DB_PATH");
        assert_eq!(path, "/tmp/explicit.db");
    }

    #[test]
    fn db_path_uses_kittypaw_home_when_set() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("KITTYPAW_DB_PATH");
        std::env::set_var("KITTYPAW_HOME", "/tmp/test-kittypaw-home");
        let path = db_path();
        std::env::remove_var("KITTYPAW_HOME");
        assert!(
            path.contains("/tmp/test-kittypaw-home"),
            "Expected path under KITTYPAW_HOME, got: {path}"
        );
        assert!(
            path.ends_with("kittypaw.db"),
            "Expected kittypaw.db suffix, got: {path}"
        );
    }
}
