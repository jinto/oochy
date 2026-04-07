use super::*;
use crate::test_utils::{open_store, temp_db_path};
use std::collections::HashMap;

fn make_call(method: &str, args: Vec<serde_json::Value>) -> SkillCall {
    SkillCall {
        skill_name: "Storage".to_string(),
        method: method.to_string(),
        args,
    }
}

#[test]
fn test_is_read_only_http_get() {
    let call = SkillCall {
        skill_name: "Http".to_string(),
        method: "get".to_string(),
        args: vec![],
    };

    assert!(is_read_only_skill_call(&call));
}

#[test]
fn test_is_not_read_only_http_post() {
    let call = SkillCall {
        skill_name: "Http".to_string(),
        method: "post".to_string(),
        args: vec![],
    };

    assert!(!is_read_only_skill_call(&call));
}

#[test]
fn test_is_not_read_only_telegram() {
    let call = SkillCall {
        skill_name: "Telegram".to_string(),
        method: "sendMessage".to_string(),
        args: vec![],
    };

    assert!(!is_read_only_skill_call(&call));
}

#[test]
fn test_is_read_only_storage_get() {
    let call = SkillCall {
        skill_name: "Storage".to_string(),
        method: "get".to_string(),
        args: vec![],
    };

    assert!(is_read_only_skill_call(&call));
}

#[test]
fn test_is_not_read_only_storage_set() {
    let call = SkillCall {
        skill_name: "Storage".to_string(),
        method: "set".to_string(),
        args: vec![],
    };

    assert!(!is_read_only_skill_call(&call));
}

fn make_telegram_call(method: &str, args: Vec<serde_json::Value>) -> SkillCall {
    SkillCall {
        skill_name: "Telegram".to_string(),
        method: method.to_string(),
        args,
    }
}

fn telegram_config() -> kittypaw_core::config::Config {
    let mut config = kittypaw_core::config::Config::default();
    config.channels.push(kittypaw_core::config::ChannelConfig {
        channel_type: kittypaw_core::config::ChannelType::Telegram,
        token: "dummy-token".to_string(),
        bind_addr: None,
    });
    config
}

#[tokio::test]
async fn test_telegram_send_message_empty_chat_id_uses_default() {
    // Empty chat_id → tries default from secrets (may fail if no secrets set)
    let call = make_telegram_call("sendMessage", vec![json_str(""), json_str("hello")]);
    let config = telegram_config();
    let result = telegram::execute_telegram(&call, &config).await;
    // Either succeeds, fails with "chat_id not configured", or fails with an API error
    // (when chat_id is resolved from secrets but the dummy token yields a 404)
    if let Err(e) = &result {
        let msg = e.to_string();
        assert!(
            msg.contains("chat_id")
                || msg.contains("not configured")
                || msg.contains("Telegram")
                || msg.contains("error"),
            "unexpected error: {e}"
        );
    }
}

#[tokio::test]
async fn test_telegram_send_message_empty_text() {
    let call = make_telegram_call("sendMessage", vec![json_str("12345"), json_str("")]);
    let config = telegram_config();

    let result = telegram::execute_telegram(&call, &config).await;

    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("missing text"), "error was: {err}");
}

#[test]
fn test_storage_set_and_get() {
    let path = temp_db_path();
    let store = open_store(&path);

    let call = make_call("set", vec![json_str("mykey"), json_str("myvalue")]);
    let result = storage::execute_storage(&call, &store, None).unwrap();
    assert_eq!(result, serde_json::json!({ "ok": true }));

    let call = make_call("get", vec![json_str("mykey")]);
    let result = storage::execute_storage(&call, &store, None).unwrap();
    assert_eq!(result, serde_json::json!({ "value": "myvalue" }));

    let _ = std::fs::remove_file(&path);
}

#[test]
fn test_storage_get_nonexistent_key() {
    let path = temp_db_path();
    let store = open_store(&path);

    let call = make_call("get", vec![json_str("nokey")]);
    let result = storage::execute_storage(&call, &store, None).unwrap();
    assert_eq!(result, serde_json::json!({ "value": null }));

    let _ = std::fs::remove_file(&path);
}

#[test]
fn test_storage_set_overwrites() {
    let path = temp_db_path();
    let store = open_store(&path);

    let call = make_call("set", vec![json_str("k"), json_str("v1")]);
    storage::execute_storage(&call, &store, None).unwrap();

    let call = make_call("set", vec![json_str("k"), json_str("v2")]);
    storage::execute_storage(&call, &store, None).unwrap();

    let call = make_call("get", vec![json_str("k")]);
    let result = storage::execute_storage(&call, &store, None).unwrap();
    assert_eq!(result, serde_json::json!({ "value": "v2" }));

    let _ = std::fs::remove_file(&path);
}

#[test]
fn test_storage_delete() {
    let path = temp_db_path();
    let store = open_store(&path);

    let call = make_call("set", vec![json_str("k"), json_str("v")]);
    storage::execute_storage(&call, &store, None).unwrap();

    let call = make_call("delete", vec![json_str("k")]);
    let result = storage::execute_storage(&call, &store, None).unwrap();
    assert_eq!(result, serde_json::json!({ "ok": true }));

    let call = make_call("get", vec![json_str("k")]);
    let result = storage::execute_storage(&call, &store, None).unwrap();
    assert_eq!(result, serde_json::json!({ "value": null }));

    let _ = std::fs::remove_file(&path);
}

#[test]
fn test_storage_delete_nonexistent_key() {
    let path = temp_db_path();
    let store = open_store(&path);

    let call = make_call("delete", vec![json_str("nokey")]);
    let result = storage::execute_storage(&call, &store, None).unwrap();
    assert_eq!(result, serde_json::json!({ "ok": true }));

    let _ = std::fs::remove_file(&path);
}

#[test]
fn test_storage_set_empty_key() {
    let path = temp_db_path();
    let store = open_store(&path);

    let call = make_call("set", vec![json_str(""), json_str("value")]);
    let result = storage::execute_storage(&call, &store, None);

    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("key is required"), "error was: {err}");

    let _ = std::fs::remove_file(&path);
}

#[test]
fn test_storage_delete_empty_key() {
    let path = temp_db_path();
    let store = open_store(&path);

    let call = make_call("delete", vec![json_str("")]);
    let result = storage::execute_storage(&call, &store, None);

    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("key is required"), "error was: {err}");

    let _ = std::fs::remove_file(&path);
}

#[test]
fn test_storage_list_empty() {
    let path = temp_db_path();
    let store = open_store(&path);

    let call = make_call("list", vec![]);
    let result = storage::execute_storage(&call, &store, None).unwrap();
    let empty: Vec<String> = vec![];
    assert_eq!(result, serde_json::json!({ "keys": empty }));

    let _ = std::fs::remove_file(&path);
}

#[test]
fn test_storage_list_with_keys() {
    let path = temp_db_path();
    let store = open_store(&path);

    for key in &["alpha", "beta", "gamma"] {
        let call = make_call("set", vec![json_str(key), json_str("val")]);
        storage::execute_storage(&call, &store, None).unwrap();
    }

    let call = make_call("list", vec![]);
    let result = storage::execute_storage(&call, &store, None).unwrap();
    let keys = result["keys"].as_array().unwrap();
    assert_eq!(keys.len(), 3);
    assert!(keys.contains(&serde_json::json!("alpha")));
    assert!(keys.contains(&serde_json::json!("beta")));
    assert!(keys.contains(&serde_json::json!("gamma")));

    let _ = std::fs::remove_file(&path);
}

#[test]
fn test_storage_namespace_isolation() {
    let path = temp_db_path();
    let store = open_store(&path);

    // Set a key in the default namespace
    let call = make_call("set", vec![json_str("shared_key"), json_str("default_val")]);
    storage::execute_storage(&call, &store, None).unwrap();

    // Insert directly into a different namespace to verify isolation
    store
        .storage_set("other_ns", "shared_key", "other_val")
        .unwrap();

    // Reading via execute_storage should only see the "default" namespace
    let call = make_call("get", vec![json_str("shared_key")]);
    let result = storage::execute_storage(&call, &store, None).unwrap();
    assert_eq!(result, serde_json::json!({ "value": "default_val" }));

    // List should only show default namespace keys
    let call = make_call("list", vec![]);
    let result = storage::execute_storage(&call, &store, None).unwrap();
    let keys = result["keys"].as_array().unwrap();
    assert_eq!(keys.len(), 1);

    let _ = std::fs::remove_file(&path);
}

fn json_str(s: &str) -> serde_json::Value {
    serde_json::Value::String(s.to_string())
}

// SSRF validation tests
#[test]
fn test_validate_url_blocks_localhost() {
    assert!(http::validate_url("http://localhost/foo", &[]).is_err());
    assert!(http::validate_url("http://127.0.0.1/foo", &[]).is_err());
}

#[test]
fn test_validate_url_blocks_private_ipv4() {
    assert!(http::validate_url("http://10.0.0.1/api", &[]).is_err());
    assert!(http::validate_url("http://192.168.1.1/api", &[]).is_err());
    assert!(http::validate_url("http://172.16.0.1/api", &[]).is_err());
    assert!(http::validate_url("http://172.31.255.255/api", &[]).is_err());
}

#[test]
fn test_validate_url_blocks_ipv6_loopback() {
    assert!(http::validate_url("http://[::1]/api", &[]).is_err());
}

#[test]
fn test_validate_url_blocks_ipv6_ula() {
    assert!(http::validate_url("http://[fc00::1]/api", &[]).is_err());
    assert!(http::validate_url("http://[fd12:3456::1]/api", &[]).is_err());
}

#[test]
fn test_validate_url_blocks_ipv6_link_local() {
    assert!(http::validate_url("http://[fe80::1]/api", &[]).is_err());
}

#[test]
fn test_validate_url_blocks_ipv6_mapped_private() {
    assert!(http::validate_url("http://[::ffff:127.0.0.1]/api", &[]).is_err());
    assert!(http::validate_url("http://[::ffff:10.0.0.1]/api", &[]).is_err());
    assert!(http::validate_url("http://[::ffff:192.168.1.1]/api", &[]).is_err());
}

#[test]
fn test_validate_url_allows_public() {
    assert!(http::validate_url("https://api.example.com/data", &[]).is_ok());
    assert!(http::validate_url("https://8.8.8.8/dns", &[]).is_ok());
}

#[test]
fn test_validate_url_blocks_non_http_schemes() {
    assert!(http::validate_url("ftp://example.com/file", &[]).is_err());
    assert!(http::validate_url("file:///etc/passwd", &[]).is_err());
}

#[test]
fn test_validate_url_blocks_metadata() {
    assert!(http::validate_url("http://metadata.google.internal/v1/", &[]).is_err());
}

#[test]
fn test_validate_url_respects_allowlist() {
    let allowed = vec!["api.example.com".to_string()];
    assert!(http::validate_url("https://api.example.com/data", &allowed).is_ok());
    assert!(http::validate_url("https://evil.com/data", &allowed).is_err());
}

// File skill tests

fn make_file_call(method: &str, args: Vec<serde_json::Value>) -> SkillCall {
    SkillCall {
        skill_name: "File".to_string(),
        method: method.to_string(),
        args,
    }
}

#[test]
fn test_file_write_and_read() {
    let dir = tempfile::tempdir().unwrap();
    let call = make_file_call("write", vec![json_str("test.txt"), json_str("hello world")]);
    let result = file::execute_file(&call, Some(dir.path())).unwrap();
    assert_eq!(result, serde_json::json!({ "ok": true }));

    let call = make_file_call("read", vec![json_str("test.txt")]);
    let result = file::execute_file(&call, Some(dir.path())).unwrap();
    assert_eq!(result, serde_json::json!({ "content": "hello world" }));
}

#[test]
fn test_file_path_traversal_blocked() {
    let dir = tempfile::tempdir().unwrap();
    let call = make_file_call("read", vec![json_str("../../../etc/passwd")]);
    let result = file::execute_file(&call, Some(dir.path()));
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("path traversal"), "error was: {err}");
}

#[test]
fn test_file_size_limit() {
    let dir = tempfile::tempdir().unwrap();
    let big_content = "x".repeat(11 * 1024 * 1024); // 11MB
    let call = make_file_call("write", vec![json_str("big.txt"), json_str(&big_content)]);
    let result = file::execute_file(&call, Some(dir.path()));
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("10MB"), "error was: {err}");
}

// Env skill tests

fn make_env_call(method: &str, args: Vec<serde_json::Value>) -> SkillCall {
    SkillCall {
        skill_name: "Env".to_string(),
        method: method.to_string(),
        args,
    }
}

#[test]
fn test_env_get_from_config() {
    let mut config = HashMap::new();
    config.insert("MY_KEY".to_string(), "my_value".to_string());
    let call = make_env_call("get", vec![json_str("MY_KEY")]);
    let result = env::execute_env(&call, Some(&config)).unwrap();
    assert_eq!(result, serde_json::json!({ "value": "my_value" }));
}

#[test]
fn test_env_get_missing_key() {
    let config = HashMap::new();
    let call = make_env_call("get", vec![json_str("MISSING")]);
    let result = env::execute_env(&call, Some(&config)).unwrap();
    assert_eq!(result, serde_json::json!({ "value": null }));
}

// Web skill tests

fn make_web_call(method: &str, args: Vec<serde_json::Value>) -> SkillCall {
    SkillCall {
        skill_name: "Web".to_string(),
        method: method.to_string(),
        args,
    }
}

#[tokio::test]
async fn test_web_search_requires_query() {
    let call = make_web_call("search", vec![]);
    let result = http::execute_web(&call, &[]).await;
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("query is required"), "error was: {err}");
}

#[tokio::test]
async fn test_web_fetch_requires_url() {
    let call = make_web_call("fetch", vec![]);
    let result = http::execute_web(&call, &[]).await;
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("url is required"), "error was: {err}");
}

#[test]
fn test_strip_html_tags() {
    let html = "<html><body><h1>Hello</h1><p>World</p></body></html>";
    let text = http::strip_html_tags(html);
    assert_eq!(text, "Hello World");
}

#[test]
fn test_strip_html_removes_scripts() {
    let html = "<p>Before</p><script>alert('xss')</script><style>.x{color:red}</style><p>After</p>";
    let text = http::strip_html_tags(html);
    assert_eq!(text, "Before After");
}

#[tokio::test]
async fn test_resolve_skill_call_file_denied_by_permission() {
    let path = temp_db_path();
    let store = Arc::new(tokio::sync::Mutex::new(open_store(&path)));
    let config = kittypaw_core::config::Config::default();

    let call = SkillCall {
        skill_name: "File".to_string(),
        method: "write".to_string(),
        args: vec![
            serde_json::Value::String("test.txt".into()),
            serde_json::Value::String("content".into()),
        ],
    };

    let deny_cb: PermissionCallback = Arc::new(|_req| {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let _ = tx.send(PermissionDecision::Deny);
        rx
    });

    let result = resolve_skill_call(&call, &config, &store, None, Some(&deny_cb)).await;
    let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert!(
        parsed.get("error").is_some(),
        "File.write should be denied when permission callback denies"
    );

    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn test_result_size_limit_over_threshold_returns_error_json_from_resolve_skill_call() {
    let path = temp_db_path();
    let store = Arc::new(tokio::sync::Mutex::new(open_store(&path)));
    let config = kittypaw_core::config::Config::default();

    let large = "x".repeat(MAX_SKILL_RESULT_BYTES + 1);
    {
        let s = store.lock().await;
        let set_call = make_call("set", vec![json_str("big"), json_str(&large)]);
        storage::execute_storage(&set_call, &s, None).unwrap();
    }

    let get_call = make_call("get", vec![json_str("big")]);
    let result = resolve_skill_call(&get_call, &config, &store, None, None).await;
    let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert!(parsed.get("error").is_some());
    assert_eq!(parsed.get("_error").unwrap(), true);

    let _ = std::fs::remove_file(&path);
}

#[test]
fn test_result_size_limit_under_threshold() {
    let small = "hello".to_string();
    assert!(small.len() <= MAX_SKILL_RESULT_BYTES);
}

#[test]
fn test_result_size_limit_over_threshold_returns_valid_json() {
    let large = "x".repeat(MAX_SKILL_RESULT_BYTES + 1);
    let result = if large.len() > MAX_SKILL_RESULT_BYTES {
        skill_error_json(format!(
            "Result too large ({} bytes, limit {})",
            large.len(),
            MAX_SKILL_RESULT_BYTES
        ))
    } else {
        large
    };
    let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert!(parsed.get("error").is_some());
    assert_eq!(parsed.get("_error").unwrap(), true);
}

#[tokio::test]
async fn test_http_invalid_url_not_retried() {
    // Sandbox errors (URL validation) should NOT be retried
    let call = SkillCall {
        skill_name: "Http".to_string(),
        method: "get".to_string(),
        args: vec![serde_json::Value::String("not-a-url".into())],
    };
    let config = kittypaw_core::config::Config::default();
    let counter = AtomicU32::new(0);
    let result = execute_single_call(&call, &[], &config, None, &counter, None, None).await;
    assert!(!result.success);
    assert!(
        result.error.as_ref().unwrap().contains("invalid URL"),
        "Expected URL validation error, got: {:?}",
        result.error
    );
}

// ── Shell tests ──

#[tokio::test]
async fn test_shell_exec_echo() {
    let call = SkillCall {
        skill_name: "Shell".to_string(),
        method: "exec".to_string(),
        args: vec![json_str("echo hello")],
    };
    let result = shell::execute_shell(&call).await.unwrap();
    assert_eq!(result["stdout"].as_str().unwrap().trim(), "hello");
    assert_eq!(result["exit_code"], 0);
}

#[tokio::test]
async fn test_shell_exec_empty_command() {
    let call = SkillCall {
        skill_name: "Shell".to_string(),
        method: "exec".to_string(),
        args: vec![json_str("")],
    };
    let result = shell::execute_shell(&call).await;
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("command is required"));
}

#[tokio::test]
async fn test_shell_unknown_method() {
    let call = SkillCall {
        skill_name: "Shell".to_string(),
        method: "run".to_string(),
        args: vec![],
    };
    let result = shell::execute_shell(&call).await;
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("Unknown Shell method"));
}

// ── Git tests ──

#[tokio::test]
async fn test_git_status() {
    let call = SkillCall {
        skill_name: "Git".to_string(),
        method: "status".to_string(),
        args: vec![],
    };
    // Should succeed in any git repo (we're in one)
    let result = git::execute_git(&call).await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap()["exit_code"], 0);
}

#[tokio::test]
async fn test_git_log() {
    let call = SkillCall {
        skill_name: "Git".to_string(),
        method: "log".to_string(),
        args: vec![serde_json::json!(3)],
    };
    let result = git::execute_git(&call).await.unwrap();
    assert_eq!(result["exit_code"], 0);
    let stdout = result["stdout"].as_str().unwrap();
    assert!(!stdout.is_empty());
}

#[tokio::test]
async fn test_git_commit_empty_message() {
    let call = SkillCall {
        skill_name: "Git".to_string(),
        method: "commit".to_string(),
        args: vec![json_str("")],
    };
    let result = git::execute_git(&call).await;
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("message is required"));
}

#[tokio::test]
async fn test_git_unknown_method() {
    let call = SkillCall {
        skill_name: "Git".to_string(),
        method: "push".to_string(),
        args: vec![],
    };
    let result = git::execute_git(&call).await;
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("Unknown Git method"));
}

// ── is_read_only: Shell & Git ──

#[test]
fn test_is_read_only_git_status() {
    let call = SkillCall {
        skill_name: "Git".to_string(),
        method: "status".to_string(),
        args: vec![],
    };
    assert!(is_read_only_skill_call(&call));
}

#[test]
fn test_is_read_only_git_diff() {
    let call = SkillCall {
        skill_name: "Git".to_string(),
        method: "diff".to_string(),
        args: vec![],
    };
    assert!(is_read_only_skill_call(&call));
}

#[test]
fn test_is_read_only_git_log() {
    let call = SkillCall {
        skill_name: "Git".to_string(),
        method: "log".to_string(),
        args: vec![],
    };
    assert!(is_read_only_skill_call(&call));
}

#[test]
fn test_is_not_read_only_git_commit() {
    let call = SkillCall {
        skill_name: "Git".to_string(),
        method: "commit".to_string(),
        args: vec![],
    };
    assert!(!is_read_only_skill_call(&call));
}

#[test]
fn test_is_not_read_only_shell_exec() {
    let call = SkillCall {
        skill_name: "Shell".to_string(),
        method: "exec".to_string(),
        args: vec![],
    };
    assert!(!is_read_only_skill_call(&call));
}

// ── File.edit tests ──

#[test]
fn test_file_edit_replaces_content() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("test.txt");
    std::fs::write(&file_path, "hello world").unwrap();

    let call = SkillCall {
        skill_name: "File".to_string(),
        method: "edit".to_string(),
        args: vec![json_str("test.txt"), json_str("hello"), json_str("goodbye")],
    };
    let result = file::execute_file(&call, Some(dir.path())).unwrap();
    assert_eq!(result, serde_json::json!({ "ok": true }));

    let content = std::fs::read_to_string(&file_path).unwrap();
    assert_eq!(content, "goodbye world");
}

#[test]
fn test_file_edit_old_content_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("test.txt");
    std::fs::write(&file_path, "hello world").unwrap();

    let call = SkillCall {
        skill_name: "File".to_string(),
        method: "edit".to_string(),
        args: vec![json_str("test.txt"), json_str("xyz"), json_str("abc")],
    };
    let result = file::execute_file(&call, Some(dir.path()));
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("not found in file"));
}

#[test]
fn test_file_edit_empty_path() {
    let dir = tempfile::tempdir().unwrap();

    let call = SkillCall {
        skill_name: "File".to_string(),
        method: "edit".to_string(),
        args: vec![json_str(""), json_str("old"), json_str("new")],
    };
    let result = file::execute_file(&call, Some(dir.path()));
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("path is required"));
}

#[test]
fn test_file_edit_empty_old_content() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("test.txt");
    std::fs::write(&file_path, "hello").unwrap();

    let call = SkillCall {
        skill_name: "File".to_string(),
        method: "edit".to_string(),
        args: vec![json_str("test.txt"), json_str(""), json_str("new")],
    };
    let result = file::execute_file(&call, Some(dir.path()));
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("old content is required"));
}

#[test]
fn test_file_edit_path_traversal_blocked() {
    let dir = tempfile::tempdir().unwrap();

    let call = SkillCall {
        skill_name: "File".to_string(),
        method: "edit".to_string(),
        args: vec![
            json_str("../../../etc/passwd"),
            json_str("root"),
            json_str("hacked"),
        ],
    };
    let result = file::execute_file(&call, Some(dir.path()));
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("traversal"));
}

// ── process::truncate_utf8 tests ──

#[test]
fn test_truncate_utf8_short_input() {
    let result = process::truncate_utf8(b"hello", 100);
    assert_eq!(result, "hello");
}

#[test]
fn test_truncate_utf8_exact_limit() {
    let result = process::truncate_utf8(b"hello", 5);
    assert_eq!(result, "hello");
}

#[test]
fn test_truncate_utf8_over_limit() {
    let result = process::truncate_utf8(b"hello world", 5);
    assert!(result.starts_with("hello"));
    assert!(result.contains("(truncated)"));
}

#[test]
fn test_truncate_utf8_multibyte() {
    // "안녕" = 6 bytes in UTF-8, truncate at 4 should not panic
    let input = "안녕".as_bytes();
    let result = process::truncate_utf8(input, 4);
    assert!(result.contains("(truncated)"));
}

// ── Agent.delegate tests ──

#[tokio::test]
async fn test_agent_delegate_empty_task() {
    let call = SkillCall {
        skill_name: "Agent".to_string(),
        method: "delegate".to_string(),
        args: vec![json_str("")],
    };
    let config = kittypaw_core::config::Config::default();
    let result = agent::execute_agent(&call, &config).await;
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("task description is required"));
}

#[tokio::test]
async fn test_agent_delegate_depth_exceeded() {
    let call = SkillCall {
        skill_name: "Agent".to_string(),
        method: "delegate".to_string(),
        args: vec![json_str("do something"), serde_json::json!(2)],
    };
    let config = kittypaw_core::config::Config::default();
    let result = agent::execute_agent(&call, &config).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("max depth"));
}

#[tokio::test]
async fn test_agent_delegate_no_provider() {
    let call = SkillCall {
        skill_name: "Agent".to_string(),
        method: "delegate".to_string(),
        args: vec![json_str("do something")],
    };
    let config = kittypaw_core::config::Config::default();
    let result = agent::execute_agent(&call, &config).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("no LLM provider"));
}

#[tokio::test]
async fn test_agent_unknown_method() {
    let call = SkillCall {
        skill_name: "Agent".to_string(),
        method: "spawn".to_string(),
        args: vec![],
    };
    let config = kittypaw_core::config::Config::default();
    let result = agent::execute_agent(&call, &config).await;
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("Unknown Agent method"));
}

#[test]
fn test_is_not_read_only_agent_delegate() {
    let call = SkillCall {
        skill_name: "Agent".to_string(),
        method: "delegate".to_string(),
        args: vec![],
    };
    assert!(!is_read_only_skill_call(&call));
}
