use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use kittypaw_core::capability::CapabilityChecker;
use kittypaw_core::error::{KittypawError, Result};
use kittypaw_core::types::SkillCall;
use kittypaw_store::Store;

const LLM_MAX_CALLS_PER_EXECUTION: u32 = 3;

/// Check capability and log denial. Returns Err(message) if denied.
fn check_capability(
    checker: &mut CapabilityChecker,
    call: &SkillCall,
) -> std::result::Result<(), String> {
    if let Err(e) = checker.check(call) {
        let msg = e.to_string();
        tracing::warn!(
            "Capability denied for {}.{}: {}",
            call.skill_name,
            call.method,
            msg
        );
        Err(msg)
    } else {
        Ok(())
    }
}

/// Execute a single skill call inline (for use as a SkillResolver callback).
/// Returns a string result that flows back to JS during sandbox execution.
/// When `checker` is provided, the call is verified against the capability allowlist
/// before execution. If `None`, all calls are permitted (permissive/legacy mode).
pub async fn resolve_skill_call(
    call: &SkillCall,
    config: &kittypaw_core::config::Config,
    store: &Arc<Mutex<Store>>,
    checker: Option<&Arc<Mutex<CapabilityChecker>>>,
) -> String {
    if let Some(cap) = checker {
        match cap.lock() {
            Ok(mut guard) => {
                if let Err(msg) = check_capability(&mut guard, call) {
                    return serde_json::to_string(&serde_json::json!({"error": msg}))
                        .unwrap_or_else(|_| "null".to_string());
                }
            }
            Err(_) => {
                return serde_json::to_string(
                    &serde_json::json!({"error": "capability checker lock poisoned"}),
                )
                .unwrap_or_else(|_| "null".to_string());
            }
        }
    }

    // File calls are synchronous
    if call.skill_name == "File" {
        return match execute_file(call, None) {
            Ok(val) => serde_json::to_string(&val).unwrap_or_else(|_| "null".to_string()),
            Err(e) => serde_json::to_string(&serde_json::json!({"error": e.to_string()}))
                .unwrap_or_else(|_| "null".to_string()),
        };
    }

    // Env calls are synchronous
    if call.skill_name == "Env" {
        return match execute_env(call, None) {
            Ok(val) => serde_json::to_string(&val).unwrap_or_else(|_| "null".to_string()),
            Err(e) => serde_json::to_string(&serde_json::json!({"error": e.to_string()}))
                .unwrap_or_else(|_| "null".to_string()),
        };
    }

    // Web calls are async (HTTP), go through the async path
    if call.skill_name == "Web" {
        let result = execute_web(call, &config.sandbox.allowed_hosts).await;
        return match result {
            Ok(val) => serde_json::to_string(&val).unwrap_or_else(|_| "null".to_string()),
            Err(e) => serde_json::to_string(&serde_json::json!({"error": e.to_string()}))
                .unwrap_or_else(|_| "null".to_string()),
        };
    }

    // Storage calls need synchronous Store access
    if call.skill_name == "Storage" {
        let s = store.lock().unwrap();
        return match execute_storage(call, &s, None) {
            Ok(val) => serde_json::to_string(&val).unwrap_or_else(|_| "null".to_string()),
            Err(e) => serde_json::to_string(&serde_json::json!({"error": e.to_string()}))
                .unwrap_or_else(|_| "null".to_string()),
        };
    }

    let llm_call_count = AtomicU32::new(0);
    let result = execute_single_call(
        call,
        &config.sandbox.allowed_hosts,
        config,
        None,
        &llm_call_count,
        None,
    )
    .await;

    if result.success {
        serde_json::to_string(&result.result).unwrap_or_else(|_| "null".to_string())
    } else {
        serde_json::to_string(&serde_json::json!({"error": result.error}))
            .unwrap_or_else(|_| "null".to_string())
    }
}

/// Resolve storage skill calls synchronously using the store.
/// Returns a parallel Vec of Option<SkillResult> — Some for Storage calls, None for others.
/// This is separated from the async path so &Store is never captured in a Send future.
pub fn resolve_storage_calls(
    skill_calls: &[SkillCall],
    store: &Store,
    skill_context: Option<&str>,
) -> Vec<Option<SkillResult>> {
    skill_calls
        .iter()
        .map(|call| {
            if call.skill_name == "Storage" {
                Some(make_skill_result(
                    call,
                    execute_storage(call, store, skill_context),
                ))
            } else {
                None
            }
        })
        .collect()
}

/// Execute captured skill calls on the host side (outside sandbox).
/// Each skill call was captured by JS stubs inside QuickJS and is now
/// executed with real API calls after capability checking.
/// Storage calls must be pre-resolved via `resolve_storage_calls` and passed as `preresolved`.
/// When `checker` is provided, each call is verified against the capability allowlist.
/// If `None`, all calls are permitted (permissive/legacy mode).
/// `model_override` selects a named model from `config.models` for LLM calls instead of the default.
pub async fn execute_skill_calls(
    skill_calls: &[SkillCall],
    config: &kittypaw_core::config::Config,
    preresolved: Vec<Option<SkillResult>>,
    skill_context: Option<&str>,
    mut checker: Option<&mut CapabilityChecker>,
    model_override: Option<&str>,
) -> Result<Vec<SkillResult>> {
    let allowed_hosts = &config.sandbox.allowed_hosts;
    // Per-execution LLM call counter (not global, avoids race between concurrent executions)
    let llm_call_count = AtomicU32::new(0);
    // Sequential execution: skill calls are ordered side-effects from JS.
    // Parallel would break ordering guarantees (message order, read-after-write).
    let mut results = Vec::new();
    for (call, precomp) in skill_calls.iter().zip(preresolved.into_iter()) {
        let result = if let Some(r) = precomp {
            r
        } else {
            if let Some(ref mut cap) = checker {
                if let Err(msg) = check_capability(cap, call) {
                    results.push(SkillResult {
                        skill_name: call.skill_name.clone(),
                        method: call.method.clone(),
                        success: false,
                        result: serde_json::Value::Null,
                        error: Some(msg),
                    });
                    continue;
                }
            }
            execute_single_call(
                call,
                allowed_hosts,
                config,
                skill_context,
                &llm_call_count,
                model_override,
            )
            .await
        };
        results.push(result);
    }

    Ok(results)
}

fn make_skill_result(call: &SkillCall, res: Result<serde_json::Value>) -> SkillResult {
    match res {
        Ok(value) => SkillResult {
            skill_name: call.skill_name.clone(),
            method: call.method.clone(),
            success: true,
            result: value,
            error: None,
        },
        Err(e) => SkillResult {
            skill_name: call.skill_name.clone(),
            method: call.method.clone(),
            success: false,
            result: serde_json::Value::Null,
            error: Some(e.to_string()),
        },
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SkillResult {
    pub skill_name: String,
    pub method: String,
    pub success: bool,
    pub result: serde_json::Value,
    pub error: Option<String>,
}

async fn execute_single_call(
    call: &SkillCall,
    allowed_hosts: &[String],
    config: &kittypaw_core::config::Config,
    _skill_context: Option<&str>,
    llm_call_count: &AtomicU32,
    model_override: Option<&str>,
) -> SkillResult {
    let result = match call.skill_name.as_str() {
        "Telegram" => execute_telegram(call).await,
        "Slack" => execute_slack(call).await,
        "Discord" => execute_discord(call).await,
        "Http" => execute_http(call, allowed_hosts).await,
        "Web" => execute_web(call, allowed_hosts).await,
        "Llm" => execute_llm(call, config, llm_call_count, model_override).await,
        "File" => execute_file(call, None),
        "Env" => execute_env(call, None),
        _ => Err(KittypawError::CapabilityDenied(format!(
            "Unknown skill: {}",
            call.skill_name
        ))),
    };

    make_skill_result(call, result)
}

async fn execute_telegram(call: &SkillCall) -> Result<serde_json::Value> {
    // Token resolution chain (token is NOT passed via args — the JS ABI is
    // Telegram.sendMessage(chatId, text), so args carry only chat content):
    // 1. global channel secret from Settings
    // 2. environment variable fallback
    let bot_token = kittypaw_core::secrets::get_secret("channels", "telegram_token")
        .ok()
        .flatten()
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("KITTYPAW_TELEGRAM_TOKEN").ok())
        .ok_or_else(|| KittypawError::Config("Telegram bot token not configured".into()))?;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| KittypawError::Skill(format!("Telegram client build error: {e}")))?;

    match call.method.as_str() {
        "sendMessage" => {
            // ABI: Telegram.sendMessage(chatId, text)
            let chat_id = call.args.first().and_then(|v| v.as_str()).unwrap_or("");
            let text = call.args.get(1).and_then(|v| v.as_str()).unwrap_or("");

            let url = format!("https://api.telegram.org/bot{bot_token}/sendMessage");
            let resp = client
                .post(&url)
                .json(&serde_json::json!({
                    "chat_id": chat_id,
                    "text": text,
                }))
                .send()
                .await
                .map_err(|e| KittypawError::Skill(format!("Telegram API error: {e}")))?;

            let status = resp.status();
            let body: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| KittypawError::Skill(format!("Telegram response parse error: {e}")))?;

            if !status.is_success() {
                let err = body
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown error");
                return Err(KittypawError::Skill(format!(
                    "Telegram sendMessage error {status}: {err}"
                )));
            }
            Ok(body)
        }
        "sendPhoto" => {
            // ABI: Telegram.sendPhoto(chatId, photoUrl)
            let chat_id = call.args.first().and_then(|v| v.as_str()).unwrap_or("");
            let photo_url = call.args.get(1).and_then(|v| v.as_str()).unwrap_or("");

            let url = format!("https://api.telegram.org/bot{bot_token}/sendPhoto");
            let resp = client
                .post(&url)
                .json(&serde_json::json!({
                    "chat_id": chat_id,
                    "photo": photo_url,
                }))
                .send()
                .await
                .map_err(|e| KittypawError::Skill(format!("Telegram API error: {e}")))?;

            let status = resp.status();
            let body: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| KittypawError::Skill(format!("Telegram response parse error: {e}")))?;
            if !status.is_success() {
                let err = body
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown error");
                return Err(KittypawError::Skill(format!(
                    "Telegram sendPhoto error {status}: {err}"
                )));
            }
            Ok(body)
        }
        "sendDocument" => {
            // ABI: Telegram.sendDocument(chatId, fileUrl, caption?)
            let chat_id = call.args.first().and_then(|v| v.as_str()).unwrap_or("");
            let file_url = call.args.get(1).and_then(|v| v.as_str()).unwrap_or("");
            let caption = call.args.get(2).and_then(|v| v.as_str()).unwrap_or("");

            let url = format!("https://api.telegram.org/bot{bot_token}/sendDocument");
            let mut payload = serde_json::json!({
                "chat_id": chat_id,
                "document": file_url,
            });
            if !caption.is_empty() {
                payload["caption"] = serde_json::Value::String(caption.to_string());
            }
            let resp = client
                .post(&url)
                .json(&payload)
                .send()
                .await
                .map_err(|e| KittypawError::Skill(format!("Telegram API error: {e}")))?;

            let status = resp.status();
            let body: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| KittypawError::Skill(format!("Telegram response parse error: {e}")))?;
            if !status.is_success() {
                let err = body
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown error");
                return Err(KittypawError::Skill(format!(
                    "Telegram sendDocument error {status}: {err}"
                )));
            }
            Ok(body)
        }
        _ => Err(KittypawError::CapabilityDenied(format!(
            "Unknown Telegram method: {}",
            call.method
        ))),
    }
}

async fn execute_slack(call: &SkillCall) -> Result<serde_json::Value> {
    // Token resolution: global channel secret from Settings, then env var fallback
    let bot_token = kittypaw_core::secrets::get_secret("channels", "slack_token")
        .ok()
        .flatten()
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("KITTYPAW_SLACK_TOKEN").ok())
        .ok_or_else(|| KittypawError::Config("Slack bot token not configured".into()))?;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| KittypawError::Skill(format!("Slack client build error: {e}")))?;

    match call.method.as_str() {
        "sendMessage" => {
            // ABI: Slack.sendMessage(channelId, text)
            let channel_id = call.args.first().and_then(|v| v.as_str()).unwrap_or("");
            let text = call.args.get(1).and_then(|v| v.as_str()).unwrap_or("");

            if channel_id.is_empty() {
                return Err(KittypawError::Skill("Slack: missing channel_id".into()));
            }
            if text.is_empty() {
                return Err(KittypawError::Skill("Slack: missing text".into()));
            }

            let resp = client
                .post("https://slack.com/api/chat.postMessage")
                .header("Authorization", format!("Bearer {bot_token}"))
                .json(&serde_json::json!({
                    "channel": channel_id,
                    "text": text,
                }))
                .send()
                .await
                .map_err(|e| KittypawError::Skill(format!("Slack API error: {e}")))?;

            let body: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| KittypawError::Skill(format!("Slack response parse error: {e}")))?;

            if !body.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
                let err = body
                    .get("error")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown error");
                return Err(KittypawError::Skill(format!(
                    "Slack postMessage error: {err}"
                )));
            }

            Ok(body)
        }
        _ => Err(KittypawError::CapabilityDenied(format!(
            "Unknown Slack method: {}",
            call.method
        ))),
    }
}

async fn execute_discord(call: &SkillCall) -> Result<serde_json::Value> {
    // Token resolution: global channel secret from Settings, then env var fallback
    let bot_token = kittypaw_core::secrets::get_secret("channels", "discord_token")
        .ok()
        .flatten()
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("KITTYPAW_DISCORD_TOKEN").ok())
        .ok_or_else(|| KittypawError::Config("Discord bot token not configured".into()))?;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| KittypawError::Skill(format!("Discord client build error: {e}")))?;

    match call.method.as_str() {
        "sendMessage" => {
            // ABI: Discord.sendMessage(channelId, text)
            let channel_id = call.args.first().and_then(|v| v.as_str()).unwrap_or("");
            let text = call.args.get(1).and_then(|v| v.as_str()).unwrap_or("");

            if channel_id.is_empty() {
                return Err(KittypawError::Skill("Discord: missing channel_id".into()));
            }
            if text.is_empty() {
                return Err(KittypawError::Skill("Discord: missing text".into()));
            }

            let url = format!("https://discord.com/api/v10/channels/{channel_id}/messages");
            let resp = client
                .post(&url)
                .header("Authorization", format!("Bot {bot_token}"))
                .json(&serde_json::json!({
                    "content": text,
                }))
                .send()
                .await
                .map_err(|e| KittypawError::Skill(format!("Discord API error: {e}")))?;

            let status = resp.status();
            let body: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| KittypawError::Skill(format!("Discord response parse error: {e}")))?;

            if !status.is_success() {
                let err = body
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown error");
                return Err(KittypawError::Skill(format!(
                    "Discord send message error {status}: {err}"
                )));
            }

            Ok(body)
        }
        _ => Err(KittypawError::CapabilityDenied(format!(
            "Unknown Discord method: {}",
            call.method
        ))),
    }
}

fn validate_url(url_str: &str, allowed_hosts: &[String]) -> Result<()> {
    use std::net::IpAddr;

    let parsed =
        url::Url::parse(url_str).map_err(|_| KittypawError::Sandbox("Http: invalid URL".into()))?;

    // Block non-HTTP(S) schemes
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(KittypawError::Sandbox(
            "Http: only http/https schemes allowed".into(),
        ));
    }

    let host = parsed
        .host_str()
        .ok_or_else(|| KittypawError::Sandbox("Http: URL has no host".into()))?;

    // Block private/internal IPs (including IPv6-mapped IPv4)
    let addr_str = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = addr_str.parse::<IpAddr>() {
        let blocked = match ip {
            IpAddr::V4(v4) => {
                v4.is_loopback() || v4.is_private() || v4.is_link_local() || v4.is_unspecified()
            }
            IpAddr::V6(v6) => {
                v6.is_loopback() || v6.is_unspecified() || v6.is_multicast()
                    // ULA (fc00::/7) and link-local (fe80::/10)
                    || (v6.segments()[0] & 0xfe00) == 0xfc00  // fc00::/7
                    || (v6.segments()[0] & 0xffc0) == 0xfe80  // fe80::/10
                    || matches!(v6.to_ipv4_mapped(), Some(v4) if v4.is_loopback() || v4.is_private() || v4.is_link_local())
            }
        };
        if blocked {
            return Err(KittypawError::Sandbox(format!(
                "Http: blocked private/internal IP: {host}"
            )));
        }
    }

    // Block known private hostnames
    if matches!(host, "localhost" | "metadata.google.internal") {
        return Err(KittypawError::Sandbox(format!(
            "Http: blocked host: {host}"
        )));
    }

    // Check allowlist if configured
    if !allowed_hosts.is_empty()
        && !allowed_hosts
            .iter()
            .any(|domain| host == domain.as_str() || host.ends_with(&format!(".{domain}")))
    {
        return Err(KittypawError::Sandbox(format!(
            "Http: host '{host}' not in allowed_hosts"
        )));
    }

    Ok(())
}

async fn execute_http(call: &SkillCall, allowed_hosts: &[String]) -> Result<serde_json::Value> {
    // Disable redirects to prevent redirect-based SSRF bypass
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| KittypawError::Sandbox(format!("Http client build error: {e}")))?;
    let url = call.args.first().and_then(|v| v.as_str()).unwrap_or("");

    if url.is_empty() {
        return Err(KittypawError::Sandbox("Http: URL is required".into()));
    }

    validate_url(url, allowed_hosts)?;

    let resp = match call.method.as_str() {
        "get" => client.get(url).send().await,
        "post" => {
            let body = call.args.get(1).cloned().unwrap_or(serde_json::Value::Null);
            client.post(url).json(&body).send().await
        }
        "put" => {
            let body = call.args.get(1).cloned().unwrap_or(serde_json::Value::Null);
            client.put(url).json(&body).send().await
        }
        "delete" => client.delete(url).send().await,
        _ => {
            return Err(KittypawError::CapabilityDenied(format!(
                "Unknown Http method: {}",
                call.method
            )))
        }
    }
    .map_err(|e| KittypawError::Skill(format!("HTTP error: {e}")))?;

    let status = resp.status().as_u16();
    let body: serde_json::Value = resp
        .json()
        .await
        .unwrap_or(serde_json::Value::String("(non-JSON response)".into()));

    Ok(serde_json::json!({
        "status": status,
        "body": body,
    }))
}

async fn execute_web(call: &SkillCall, allowed_hosts: &[String]) -> Result<serde_json::Value> {
    // Disable redirects to prevent SSRF bypass via redirect to internal IPs
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());

    match call.method.as_str() {
        "search" => {
            let query = call.args.first().and_then(|v| v.as_str()).unwrap_or("");
            if query.is_empty() {
                return Err(KittypawError::Sandbox(
                    "Web.search: query is required".into(),
                ));
            }
            let max_results = call.args.get(1).and_then(|v| v.as_u64()).unwrap_or(5) as usize;

            // Use DuckDuckGo Instant Answer API (free, no API key)
            let url = format!(
                "https://api.duckduckgo.com/?q={}&format=json&no_html=1&skip_disambig=1",
                urlencoding::encode(query)
            );
            let resp = client
                .get(&url)
                .header("User-Agent", "KittyPaw/0.1")
                .send()
                .await
                .map_err(|e| KittypawError::Skill(format!("Web.search error: {e}")))?;

            let body: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| KittypawError::Skill(format!("Web.search parse error: {e}")))?;

            // Extract results from DuckDuckGo response
            let mut results = Vec::new();

            // Abstract (direct answer)
            if let Some(abstract_text) = body["AbstractText"].as_str() {
                if !abstract_text.is_empty() {
                    results.push(serde_json::json!({
                        "title": body["Heading"].as_str().unwrap_or(""),
                        "snippet": abstract_text,
                        "url": body["AbstractURL"].as_str().unwrap_or(""),
                    }));
                }
            }

            // Related topics
            if let Some(topics) = body["RelatedTopics"].as_array() {
                for topic in topics.iter().take(max_results) {
                    if let Some(text) = topic["Text"].as_str() {
                        results.push(serde_json::json!({
                            "title": text.split(" - ").next().unwrap_or(text),
                            "snippet": text,
                            "url": topic["FirstURL"].as_str().unwrap_or(""),
                        }));
                    }
                }
            }

            Ok(serde_json::json!({
                "query": query,
                "results": results,
            }))
        }
        "fetch" => {
            let url = call.args.first().and_then(|v| v.as_str()).unwrap_or("");
            if url.is_empty() {
                return Err(KittypawError::Sandbox("Web.fetch: url is required".into()));
            }

            // Validate URL (reuse existing validate_url for SSRF protection)
            validate_url(url, allowed_hosts)?;

            let resp = client
                .get(url)
                .header("User-Agent", "KittyPaw/0.1")
                .send()
                .await
                .map_err(|e| KittypawError::Skill(format!("Web.fetch error: {e}")))?;

            let status = resp.status().as_u16();
            let content_type = resp
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_string();

            // Cap download at 100KB to bound memory usage before any processing
            const DOWNLOAD_LIMIT: usize = 100 * 1024;
            let bytes = resp
                .bytes()
                .await
                .map_err(|e| KittypawError::Skill(format!("Web.fetch read error: {e}")))?;
            let capped = if bytes.len() > DOWNLOAD_LIMIT {
                &bytes[..DOWNLOAD_LIMIT]
            } else {
                &bytes[..]
            };
            let body = String::from_utf8_lossy(capped).into_owned();

            // Basic HTML to text extraction (strip tags)
            let text = if content_type.contains("html") {
                strip_html_tags(&body)
            } else {
                body
            };

            // Truncate to prevent huge responses
            let max_len = 50_000;
            let text = if text.len() > max_len {
                // Find a valid UTF-8 char boundary at or before max_len
                let mut end = max_len;
                while end > 0 && !text.is_char_boundary(end) {
                    end -= 1;
                }
                format!("{}...(truncated)", &text[..end])
            } else {
                text
            };

            Ok(serde_json::json!({
                "url": url,
                "status": status,
                "content_type": content_type,
                "text": text,
            }))
        }
        _ => Err(KittypawError::Sandbox(format!(
            "Unknown Web method: {}",
            call.method
        ))),
    }
}

/// Simple HTML tag stripper (good enough for content extraction)
fn strip_html_tags(html: &str) -> String {
    let mut result = String::with_capacity(html.len());
    let mut in_tag = false;
    let mut in_script = false;
    let mut in_style = false;

    let lower = html.to_lowercase();
    let chars: Vec<char> = html.chars().collect();
    let lower_chars: Vec<char> = lower.chars().collect();

    let mut i = 0;
    while i < chars.len() {
        if !in_tag && chars[i] == '<' {
            in_tag = true;
            // Check for script/style tags
            let remaining: String = lower_chars[i..].iter().take(10).collect();
            if remaining.starts_with("<script") {
                in_script = true;
            }
            if remaining.starts_with("<style") {
                in_style = true;
            }
            if remaining.starts_with("</script") {
                in_script = false;
            }
            if remaining.starts_with("</style") {
                in_style = false;
            }
        } else if in_tag && chars[i] == '>' {
            in_tag = false;
            // Insert space after closing tags to separate content
            result.push(' ');
        } else if !in_tag && !in_script && !in_style {
            result.push(chars[i]);
        }
        i += 1;
    }

    // Collapse whitespace
    let mut collapsed = String::new();
    let mut last_was_space = false;
    for ch in result.chars() {
        if ch.is_whitespace() {
            if !last_was_space {
                collapsed.push(' ');
                last_was_space = true;
            }
        } else {
            collapsed.push(ch);
            last_was_space = false;
        }
    }
    collapsed.trim().to_string()
}

fn execute_storage(
    call: &SkillCall,
    store: &Store,
    skill_context: Option<&str>,
) -> Result<serde_json::Value> {
    let namespace = skill_context.unwrap_or("default");

    match call.method.as_str() {
        "get" => {
            let key = call.args.first().and_then(|v| v.as_str()).unwrap_or("");
            match store.storage_get(namespace, key)? {
                Some(value) => Ok(serde_json::json!({ "value": value })),
                None => Ok(serde_json::json!({ "value": null })),
            }
        }
        "set" => {
            let key = call.args.first().and_then(|v| v.as_str()).unwrap_or("");
            let value = call.args.get(1).and_then(|v| v.as_str()).unwrap_or("");
            store.storage_set(namespace, key, value)?;
            Ok(serde_json::json!({ "ok": true }))
        }
        "delete" => {
            let key = call.args.first().and_then(|v| v.as_str()).unwrap_or("");
            store.storage_delete(namespace, key)?;
            Ok(serde_json::json!({ "ok": true }))
        }
        "list" => {
            let keys = store.storage_list(namespace)?;
            Ok(serde_json::json!({ "keys": keys }))
        }
        _ => Err(KittypawError::Skill(format!(
            "Unknown Storage method: {}",
            call.method
        ))),
    }
}

async fn execute_llm(
    call: &SkillCall,
    config: &kittypaw_core::config::Config,
    llm_call_count: &AtomicU32,
    model_override: Option<&str>,
) -> Result<serde_json::Value> {
    let count = llm_call_count.fetch_add(1, Ordering::Relaxed);
    if count >= LLM_MAX_CALLS_PER_EXECUTION {
        return Err(KittypawError::Skill(
            "Llm recursion limit exceeded (max 3 calls per execution)".into(),
        ));
    }

    let prompt = call.args.first().and_then(|v| v.as_str()).unwrap_or("");

    if prompt.is_empty() {
        return Err(KittypawError::Skill(
            "Llm.generate: prompt is required".into(),
        ));
    }

    let max_tokens = call.args.get(1).and_then(|v| v.as_u64()).unwrap_or(1024) as u32;

    // Resolve model config: if a model override name is provided and matches a registered
    // model in config.models, use that model's credentials; otherwise fall back to default.
    let (api_key, model, base_url, provider) =
        if let Some(name) = model_override.filter(|s| !s.is_empty()) {
            if let Some(mc) = config.models.iter().find(|m| m.name == name) {
                let key = if mc.api_key.is_empty() {
                    kittypaw_core::secrets::get_secret("models", &mc.name)
                        .ok()
                        .flatten()
                        .unwrap_or_default()
                } else {
                    mc.api_key.clone()
                };
                (
                    key,
                    mc.model.clone(),
                    mc.base_url.clone(),
                    mc.provider.clone(),
                )
            } else {
                tracing::warn!(
                    "model_override '{}' not found in config.models, using default",
                    name
                );
                (
                    config.llm.api_key.clone(),
                    config.llm.model.clone(),
                    None,
                    config.llm.provider.clone(),
                )
            }
        } else {
            (
                config.llm.api_key.clone(),
                config.llm.model.clone(),
                None,
                config.llm.provider.clone(),
            )
        };

    let provider_lower = provider.to_lowercase();

    if api_key.is_empty() && !matches!(provider_lower.as_str(), "ollama" | "local") {
        return Err(KittypawError::Skill(format!(
            "No API key configured for model '{}'",
            model
        )));
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| KittypawError::Skill(format!("Llm client build error: {e}")))?;

    // Route to provider-specific endpoint
    let is_openai_compat = matches!(provider_lower.as_str(), "openai" | "ollama" | "local");
    let endpoint = if is_openai_compat {
        let url = base_url
            .as_deref()
            .unwrap_or("https://api.openai.com/v1")
            .trim_end_matches('/');
        format!("{url}/chat/completions")
    } else {
        "https://api.anthropic.com/v1/messages".to_string()
    };

    let resp = if is_openai_compat {
        let mut req = client
            .post(&endpoint)
            .header("content-type", "application/json")
            .json(&serde_json::json!({
                "model": model,
                "max_tokens": max_tokens,
                "messages": [{"role": "user", "content": prompt}]
            }));
        if !api_key.is_empty() {
            req = req.header("Authorization", format!("Bearer {api_key}"));
        }
        req.send()
            .await
            .map_err(|e| KittypawError::Skill(format!("Llm API error: {e}")))?
    } else {
        client
            .post(&endpoint)
            .header("x-api-key", &api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&serde_json::json!({
                "model": model,
                "max_tokens": max_tokens,
                "messages": [{"role": "user", "content": prompt}]
            }))
            .send()
            .await
            .map_err(|e| KittypawError::Skill(format!("Llm API error: {e}")))?
    };

    let status = resp.status();
    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| KittypawError::Skill(format!("Llm response parse error: {e}")))?;

    if !status.is_success() {
        let err = body
            .get("error")
            .and_then(|v| v.get("message"))
            .and_then(|v| v.as_str())
            .or_else(|| body.get("error").and_then(|v| v.as_str()))
            .unwrap_or("unknown error");
        return Err(KittypawError::Skill(format!(
            "Llm API error {status}: {err}"
        )));
    }

    let text = if is_openai_compat {
        body["choices"]
            .as_array()
            .and_then(|arr| arr.first())
            .and_then(|c| c["message"]["content"].as_str())
            .unwrap_or("")
            .to_string()
    } else {
        body["content"]
            .as_array()
            .and_then(|arr| arr.first())
            .and_then(|block| block["text"].as_str())
            .unwrap_or("")
            .to_string()
    };

    Ok(serde_json::json!({ "text": text }))
}

fn execute_file(call: &SkillCall, data_dir: Option<&Path>) -> Result<serde_json::Value> {
    let data_dir = data_dir.ok_or_else(|| {
        KittypawError::Sandbox("File operations require a package data directory".into())
    })?;

    // Create data dir if it doesn't exist
    std::fs::create_dir_all(data_dir)?;

    match call.method.as_str() {
        "read" => {
            let rel_path = call.args.first().and_then(|v| v.as_str()).unwrap_or("");
            if rel_path.is_empty() {
                return Err(KittypawError::Sandbox("File.read: path is required".into()));
            }
            let full_path = validate_file_path(data_dir, rel_path)?;
            let content = std::fs::read_to_string(&full_path)?;
            Ok(serde_json::json!({ "content": content }))
        }
        "write" => {
            let rel_path = call.args.first().and_then(|v| v.as_str()).unwrap_or("");
            let content = call.args.get(1).and_then(|v| v.as_str()).unwrap_or("");
            if rel_path.is_empty() {
                return Err(KittypawError::Sandbox(
                    "File.write: path is required".into(),
                ));
            }
            let full_path = validate_file_path(data_dir, rel_path)?;
            // Max file size: 10MB
            if content.len() > 10 * 1024 * 1024 {
                return Err(KittypawError::Sandbox(
                    "File.write: content exceeds 10MB limit".into(),
                ));
            }
            // Create parent directories
            if let Some(parent) = full_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&full_path, content)?;
            Ok(serde_json::json!({ "ok": true }))
        }
        _ => Err(KittypawError::Sandbox(format!(
            "Unknown File method: {}",
            call.method
        ))),
    }
}

/// Validate that a relative path stays within the data directory.
/// Rejects ".." components and symlinks escaping the boundary.
fn validate_file_path(data_dir: &Path, rel_path: &str) -> Result<PathBuf> {
    if rel_path.contains("..") {
        return Err(KittypawError::Sandbox(
            "File: path traversal not allowed".into(),
        ));
    }
    let rel = rel_path.trim_start_matches('/');
    let full = data_dir.join(rel);
    if full.exists() {
        // For existing files, canonicalize and check prefix
        let canonical = full.canonicalize()?;
        let canonical_root = data_dir.canonicalize()?;
        if !canonical.starts_with(&canonical_root) {
            return Err(KittypawError::Sandbox(
                "File: path escapes data directory".into(),
            ));
        }
        Ok(canonical)
    } else {
        // For non-existent files, canonicalize the parent and append filename
        let parent = full
            .parent()
            .ok_or_else(|| KittypawError::Sandbox("File: path has no parent directory".into()))?;
        let file_name = full
            .file_name()
            .ok_or_else(|| KittypawError::Sandbox("File: path has no filename".into()))?;
        // Parent must exist; if it doesn't, reject to prevent traversal via missing dirs
        let canonical_parent = parent
            .canonicalize()
            .map_err(|_| KittypawError::Sandbox("File: parent directory does not exist".into()))?;
        let canonical_root = data_dir.canonicalize()?;
        if !canonical_parent.starts_with(&canonical_root) {
            return Err(KittypawError::Sandbox(
                "File: path escapes data directory".into(),
            ));
        }
        Ok(canonical_parent.join(file_name))
    }
}

fn execute_env(
    call: &SkillCall,
    config_values: Option<&HashMap<String, String>>,
) -> Result<serde_json::Value> {
    match call.method.as_str() {
        "get" => {
            let key = call.args.first().and_then(|v| v.as_str()).unwrap_or("");
            if key.is_empty() {
                return Err(KittypawError::Sandbox("Env.get: key is required".into()));
            }
            // Read from package config, NOT from real environment variables
            let value = config_values.and_then(|m| m.get(key)).cloned();
            match value {
                Some(v) => Ok(serde_json::json!({ "value": v })),
                None => Ok(serde_json::json!({ "value": null })),
            }
        }
        _ => Err(KittypawError::Sandbox(format!(
            "Unknown Env method: {}",
            call.method
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_db_path() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let mut p = std::env::temp_dir();
        p.push(format!(
            "kittypaw_skill_test_{}_{}.db",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        p
    }

    fn open_store(path: &std::path::Path) -> Store {
        Store::open(path.to_str().unwrap()).unwrap()
    }

    fn make_call(method: &str, args: Vec<serde_json::Value>) -> SkillCall {
        SkillCall {
            skill_name: "Storage".to_string(),
            method: method.to_string(),
            args,
        }
    }

    #[test]
    fn test_storage_set_and_get() {
        let path = temp_db_path();
        let store = open_store(&path);

        let call = make_call("set", vec![json_str("mykey"), json_str("myvalue")]);
        let result = execute_storage(&call, &store, None).unwrap();
        assert_eq!(result, serde_json::json!({ "ok": true }));

        let call = make_call("get", vec![json_str("mykey")]);
        let result = execute_storage(&call, &store, None).unwrap();
        assert_eq!(result, serde_json::json!({ "value": "myvalue" }));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_storage_get_nonexistent_key() {
        let path = temp_db_path();
        let store = open_store(&path);

        let call = make_call("get", vec![json_str("nokey")]);
        let result = execute_storage(&call, &store, None).unwrap();
        assert_eq!(result, serde_json::json!({ "value": null }));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_storage_set_overwrites() {
        let path = temp_db_path();
        let store = open_store(&path);

        let call = make_call("set", vec![json_str("k"), json_str("v1")]);
        execute_storage(&call, &store, None).unwrap();

        let call = make_call("set", vec![json_str("k"), json_str("v2")]);
        execute_storage(&call, &store, None).unwrap();

        let call = make_call("get", vec![json_str("k")]);
        let result = execute_storage(&call, &store, None).unwrap();
        assert_eq!(result, serde_json::json!({ "value": "v2" }));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_storage_delete() {
        let path = temp_db_path();
        let store = open_store(&path);

        let call = make_call("set", vec![json_str("k"), json_str("v")]);
        execute_storage(&call, &store, None).unwrap();

        let call = make_call("delete", vec![json_str("k")]);
        let result = execute_storage(&call, &store, None).unwrap();
        assert_eq!(result, serde_json::json!({ "ok": true }));

        let call = make_call("get", vec![json_str("k")]);
        let result = execute_storage(&call, &store, None).unwrap();
        assert_eq!(result, serde_json::json!({ "value": null }));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_storage_delete_nonexistent_key() {
        let path = temp_db_path();
        let store = open_store(&path);

        let call = make_call("delete", vec![json_str("nokey")]);
        let result = execute_storage(&call, &store, None).unwrap();
        assert_eq!(result, serde_json::json!({ "ok": true }));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_storage_list_empty() {
        let path = temp_db_path();
        let store = open_store(&path);

        let call = make_call("list", vec![]);
        let result = execute_storage(&call, &store, None).unwrap();
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
            execute_storage(&call, &store, None).unwrap();
        }

        let call = make_call("list", vec![]);
        let result = execute_storage(&call, &store, None).unwrap();
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
        execute_storage(&call, &store, None).unwrap();

        // Insert directly into a different namespace to verify isolation
        store
            .storage_set("other_ns", "shared_key", "other_val")
            .unwrap();

        // Reading via execute_storage should only see the "default" namespace
        let call = make_call("get", vec![json_str("shared_key")]);
        let result = execute_storage(&call, &store, None).unwrap();
        assert_eq!(result, serde_json::json!({ "value": "default_val" }));

        // List should only show default namespace keys
        let call = make_call("list", vec![]);
        let result = execute_storage(&call, &store, None).unwrap();
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
        assert!(validate_url("http://localhost/foo", &[]).is_err());
        assert!(validate_url("http://127.0.0.1/foo", &[]).is_err());
    }

    #[test]
    fn test_validate_url_blocks_private_ipv4() {
        assert!(validate_url("http://10.0.0.1/api", &[]).is_err());
        assert!(validate_url("http://192.168.1.1/api", &[]).is_err());
        assert!(validate_url("http://172.16.0.1/api", &[]).is_err());
        assert!(validate_url("http://172.31.255.255/api", &[]).is_err());
    }

    #[test]
    fn test_validate_url_blocks_ipv6_loopback() {
        assert!(validate_url("http://[::1]/api", &[]).is_err());
    }

    #[test]
    fn test_validate_url_blocks_ipv6_ula() {
        assert!(validate_url("http://[fc00::1]/api", &[]).is_err());
        assert!(validate_url("http://[fd12:3456::1]/api", &[]).is_err());
    }

    #[test]
    fn test_validate_url_blocks_ipv6_link_local() {
        assert!(validate_url("http://[fe80::1]/api", &[]).is_err());
    }

    #[test]
    fn test_validate_url_blocks_ipv6_mapped_private() {
        assert!(validate_url("http://[::ffff:127.0.0.1]/api", &[]).is_err());
        assert!(validate_url("http://[::ffff:10.0.0.1]/api", &[]).is_err());
        assert!(validate_url("http://[::ffff:192.168.1.1]/api", &[]).is_err());
    }

    #[test]
    fn test_validate_url_allows_public() {
        assert!(validate_url("https://api.example.com/data", &[]).is_ok());
        assert!(validate_url("https://8.8.8.8/dns", &[]).is_ok());
    }

    #[test]
    fn test_validate_url_blocks_non_http_schemes() {
        assert!(validate_url("ftp://example.com/file", &[]).is_err());
        assert!(validate_url("file:///etc/passwd", &[]).is_err());
    }

    #[test]
    fn test_validate_url_blocks_metadata() {
        assert!(validate_url("http://metadata.google.internal/v1/", &[]).is_err());
    }

    #[test]
    fn test_validate_url_respects_allowlist() {
        let allowed = vec!["api.example.com".to_string()];
        assert!(validate_url("https://api.example.com/data", &allowed).is_ok());
        assert!(validate_url("https://evil.com/data", &allowed).is_err());
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
        let result = execute_file(&call, Some(dir.path())).unwrap();
        assert_eq!(result, serde_json::json!({ "ok": true }));

        let call = make_file_call("read", vec![json_str("test.txt")]);
        let result = execute_file(&call, Some(dir.path())).unwrap();
        assert_eq!(result, serde_json::json!({ "content": "hello world" }));
    }

    #[test]
    fn test_file_path_traversal_blocked() {
        let dir = tempfile::tempdir().unwrap();
        let call = make_file_call("read", vec![json_str("../../../etc/passwd")]);
        let result = execute_file(&call, Some(dir.path()));
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("path traversal"), "error was: {err}");
    }

    #[test]
    fn test_file_size_limit() {
        let dir = tempfile::tempdir().unwrap();
        let big_content = "x".repeat(11 * 1024 * 1024); // 11MB
        let call = make_file_call("write", vec![json_str("big.txt"), json_str(&big_content)]);
        let result = execute_file(&call, Some(dir.path()));
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
        let result = execute_env(&call, Some(&config)).unwrap();
        assert_eq!(result, serde_json::json!({ "value": "my_value" }));
    }

    #[test]
    fn test_env_get_missing_key() {
        let config = HashMap::new();
        let call = make_env_call("get", vec![json_str("MISSING")]);
        let result = execute_env(&call, Some(&config)).unwrap();
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
        let result = execute_web(&call, &[]).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("query is required"), "error was: {err}");
    }

    #[tokio::test]
    async fn test_web_fetch_requires_url() {
        let call = make_web_call("fetch", vec![]);
        let result = execute_web(&call, &[]).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("url is required"), "error was: {err}");
    }

    #[test]
    fn test_strip_html_tags() {
        let html = "<html><body><h1>Hello</h1><p>World</p></body></html>";
        let text = strip_html_tags(html);
        assert_eq!(text, "Hello World");
    }

    #[test]
    fn test_strip_html_removes_scripts() {
        let html =
            "<p>Before</p><script>alert('xss')</script><style>.x{color:red}</style><p>After</p>";
        let text = strip_html_tags(html);
        assert_eq!(text, "Before After");
    }
}
