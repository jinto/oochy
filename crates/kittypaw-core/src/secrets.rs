use crate::error::{KittypawError, Result};

const SERVICE_NAME: &str = "kittypaw";

/// Store a secret in the OS keychain.
/// Key format: "{namespace}/{key}" (e.g. "settings/api_key", "packages/macro-economy-report/telegram_token")
pub fn set_secret(namespace: &str, key: &str, value: &str) -> Result<()> {
    let entry_key = format!("{namespace}/{key}");
    let entry = keyring::Entry::new(SERVICE_NAME, &entry_key)
        .map_err(|e| KittypawError::Config(format!("Keyring error: {e}")))?;
    entry
        .set_password(value)
        .map_err(|e| KittypawError::Config(format!("Failed to store secret: {e}")))?;
    Ok(())
}

/// Get a secret from the OS keychain.
pub fn get_secret(namespace: &str, key: &str) -> Result<Option<String>> {
    let entry_key = format!("{namespace}/{key}");
    let entry = keyring::Entry::new(SERVICE_NAME, &entry_key)
        .map_err(|e| KittypawError::Config(format!("Keyring error: {e}")))?;
    match entry.get_password() {
        Ok(password) => Ok(Some(password)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(KittypawError::Config(format!("Failed to read secret: {e}"))),
    }
}

/// Delete a secret from the OS keychain.
pub fn delete_secret(namespace: &str, key: &str) -> Result<()> {
    let entry_key = format!("{namespace}/{key}");
    let entry = keyring::Entry::new(SERVICE_NAME, &entry_key)
        .map_err(|e| KittypawError::Config(format!("Keyring error: {e}")))?;
    match entry.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()), // Already deleted
        Err(e) => Err(KittypawError::Config(format!(
            "Failed to delete secret: {e}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // These tests interact with the OS keychain and may prompt for permission
    // on macOS. Run explicitly with: cargo test -p kittypaw-core secrets -- --ignored

    #[test]
    #[ignore = "requires OS keychain access, may prompt for permission"]
    fn test_set_and_get_secret() {
        set_secret("test", "sgkey", "sgvalue").unwrap();
        let val = get_secret("test", "sgkey").unwrap();
        assert_eq!(val, Some("sgvalue".to_string()));
        // Cleanup
        let _ = delete_secret("test", "sgkey");
    }

    #[test]
    #[ignore = "requires OS keychain access, may prompt for permission"]
    fn test_get_nonexistent() {
        let val = get_secret("test", "nonexistent_key_12345").unwrap();
        assert_eq!(val, None);
    }

    #[test]
    #[ignore = "requires OS keychain access, may prompt for permission"]
    fn test_delete_secret() {
        set_secret("test", "delkey", "delvalue").unwrap();
        delete_secret("test", "delkey").unwrap();
        let val = get_secret("test", "delkey").unwrap();
        assert_eq!(val, None);
    }
}
