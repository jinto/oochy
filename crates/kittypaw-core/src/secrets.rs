use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::error::{KittypawError, Result};

const SECRETS_FILE: &str = "secrets.json";

/// Marker stored in config.toml to indicate a value lives in the secret store.
pub const SECRET_MARKER: &str = "<secret>";
/// Legacy marker from keychain-based storage; kept for backward compatibility on read.
pub const LEGACY_KEYCHAIN_MARKER: &str = "<keychain>";

/// Store a secret in KittyPaw's local secret store.
/// Key format: "{namespace}/{key}" (e.g. "settings/api_key", "packages/macro-economy-report/telegram_token")
pub fn set_secret(namespace: &str, key: &str, value: &str) -> Result<()> {
    set_secret_in(&secrets_path()?, namespace, key, value)
}

/// Get a secret from KittyPaw's local secret store.
pub fn get_secret(namespace: &str, key: &str) -> Result<Option<String>> {
    get_secret_in(&secrets_path()?, namespace, key)
}

/// Delete a secret from KittyPaw's local secret store.
pub fn delete_secret(namespace: &str, key: &str) -> Result<()> {
    delete_secret_in(&secrets_path()?, namespace, key)
}

/// Returns the KittyPaw data directory (~/.kittypaw or $KITTYPAW_HOME).
pub fn data_dir() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("KITTYPAW_HOME") {
        return Ok(PathBuf::from(path));
    }

    dirs_next::home_dir()
        .map(|p| p.join(".kittypaw"))
        .ok_or_else(|| {
            KittypawError::Config("Cannot determine home directory; set KITTYPAW_HOME".into())
        })
}

fn secrets_path() -> Result<PathBuf> {
    Ok(data_dir()?.join(SECRETS_FILE))
}

fn secret_id(namespace: &str, key: &str) -> String {
    format!("{namespace}/{key}")
}

fn load_store(path: &Path) -> Result<HashMap<String, String>> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(HashMap::new()),
        Err(e) => return Err(e.into()),
    };
    if content.trim().is_empty() {
        return Ok(HashMap::new());
    }

    serde_json::from_str(&content)
        .map_err(|e| KittypawError::Config(format!("Invalid secret store: {e}")))
}

fn save_store(path: &Path, store: &HashMap<String, String>) -> Result<()> {
    if store.is_empty() {
        match std::fs::remove_file(path) {
            Ok(()) | Err(_) => {} // ignore NotFound or any other error
        }
        return Ok(());
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let tmp_path = path.with_extension("json.tmp");
    let content = serde_json::to_vec_pretty(store)
        .map_err(|e| KittypawError::Config(format!("Failed to serialize secret store: {e}")))?;
    write_restricted(&tmp_path, &content)?;

    if std::fs::rename(&tmp_path, path).is_err() {
        // Cross-device rename; fall back to copy (never deletes target first)
        std::fs::copy(&tmp_path, path)?;
        let _ = std::fs::remove_file(&tmp_path);
        // Re-apply permissions since copy may reset them
        write_restricted_permissions(path)?;
    }

    Ok(())
}

/// Write file with restrictive permissions from creation (0600 on Unix).
#[cfg(unix)]
fn write_restricted(path: &Path, content: &[u8]) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(content)?;
    Ok(())
}

#[cfg(not(unix))]
fn write_restricted(path: &Path, content: &[u8]) -> Result<()> {
    std::fs::write(path, content)?;
    Ok(())
}

#[cfg(unix)]
fn write_restricted_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn write_restricted_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

fn set_secret_in(path: &Path, namespace: &str, key: &str, value: &str) -> Result<()> {
    let mut store = load_store(path)?;
    store.insert(secret_id(namespace, key), value.to_string());
    save_store(path, &store)
}

fn get_secret_in(path: &Path, namespace: &str, key: &str) -> Result<Option<String>> {
    let store = load_store(path)?;
    Ok(store.get(&secret_id(namespace, key)).cloned())
}

fn delete_secret_in(path: &Path, namespace: &str, key: &str) -> Result<()> {
    let mut store = load_store(path)?;
    store.remove(&secret_id(namespace, key));
    save_store(path, &store)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_set_and_get_secret() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("secrets.json");

        set_secret_in(&path, "test", "sgkey", "sgvalue").unwrap();
        let val = get_secret_in(&path, "test", "sgkey").unwrap();
        assert_eq!(val, Some("sgvalue".to_string()));
    }

    #[test]
    fn test_get_nonexistent() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("secrets.json");

        let val = get_secret_in(&path, "test", "nonexistent_key_12345").unwrap();
        assert_eq!(val, None);
    }

    #[test]
    fn test_delete_secret() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("secrets.json");

        set_secret_in(&path, "test", "delkey", "delvalue").unwrap();
        delete_secret_in(&path, "test", "delkey").unwrap();
        let val = get_secret_in(&path, "test", "delkey").unwrap();
        assert_eq!(val, None);
    }

    #[test]
    fn test_delete_last_secret_removes_store_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("secrets.json");

        set_secret_in(&path, "test", "delkey", "delvalue").unwrap();
        assert!(path.exists());

        delete_secret_in(&path, "test", "delkey").unwrap();
        assert!(!path.exists());
    }
}
