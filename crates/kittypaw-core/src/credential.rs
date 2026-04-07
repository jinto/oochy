use crate::config::Config;
use crate::secrets::{LEGACY_KEYCHAIN_MARKER, SECRET_MARKER};

fn is_real_value(s: &str) -> bool {
    !s.is_empty() && s != SECRET_MARKER && s != LEGACY_KEYCHAIN_MARKER
}

/// Unified credential resolution with 4-step priority:
/// 1. `secrets("channels", key)` — Settings UI (shared across channels)
/// 2. `secrets(channel, key)` — channel-specific secrets namespace
/// 3. `env_var` — environment variable (skipped if `env_var` is empty)
/// 4. `config.channels[*].token` — config file fallback
pub fn resolve_credential(
    channel: &str,
    key: &str,
    env_var: &str,
    config: &Config,
) -> Option<String> {
    // "channels" is the shared namespace used in Step 1 — passing it as the
    // channel name would cause Steps 1 and 2 to perform the same lookup.
    if channel == "channels" {
        return None;
    }

    // Step 1: shared secrets namespace (Settings UI)
    crate::secrets::get_secret("channels", key)
        .ok()
        .flatten()
        .filter(|s| is_real_value(s))
        // Step 2: channel-specific secrets namespace
        .or_else(|| {
            crate::secrets::get_secret(channel, key)
                .ok()
                .flatten()
                .filter(|s| is_real_value(s))
        })
        // Step 3: environment variable
        .or_else(|| {
            if env_var.is_empty() {
                None
            } else {
                std::env::var(env_var).ok().filter(|s| is_real_value(s))
            }
        })
        // Step 4: config file token (last resort)
        .or_else(|| {
            config
                .channels
                .iter()
                .find(|c| c.channel_type == channel)
                .map(|c| c.token.clone())
                .filter(|s| is_real_value(s))
        })
}
