use kittypaw_core::error::{KittypawError, Result};
use kittypaw_core::types::SkillCall;

pub(super) fn validate_url(url_str: &str, allowed_hosts: &[String]) -> Result<()> {
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

pub(super) async fn execute_http(
    call: &SkillCall,
    allowed_hosts: &[String],
) -> Result<serde_json::Value> {
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

pub(super) async fn execute_web(
    call: &SkillCall,
    allowed_hosts: &[String],
) -> Result<serde_json::Value> {
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

            // Multi-backend search: auto-select based on available API keys.
            // Priority: Brave → Tavily → Exa → DuckDuckGo Instant Answer (fallback).
            let results = web_search_dispatch(query, max_results).await?;

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
pub(super) fn strip_html_tags(html: &str) -> String {
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

// ── Multi-backend web search dispatch ────────────────────────────────────

/// Auto-select search backend based on available API keys.
/// Priority: Brave → Tavily → Exa → DuckDuckGo Instant Answer (fallback).
async fn web_search_dispatch(query: &str, max_results: usize) -> Result<Vec<serde_json::Value>> {
    use kittypaw_core::secrets::get_secret;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());

    // 1. Brave Search (free 2000 queries/month)
    if let Ok(Some(key)) = get_secret("search", "brave_api_key") {
        if !key.is_empty() {
            return brave_search(&client, &key, query, max_results).await;
        }
    }

    // 2. Tavily (free 1000 queries/month)
    if let Ok(Some(key)) = get_secret("search", "tavily_api_key") {
        if !key.is_empty() {
            return tavily_search(&client, &key, query, max_results).await;
        }
    }

    // 3. Exa (AI-native search)
    if let Ok(Some(key)) = get_secret("search", "exa_api_key") {
        if !key.is_empty() {
            return exa_search(&client, &key, query, max_results).await;
        }
    }

    // 4. Fallback: DuckDuckGo Instant Answer API (free, no key, but limited)
    ddg_instant_answer(&client, query, max_results).await
}

async fn brave_search(
    client: &reqwest::Client,
    api_key: &str,
    query: &str,
    max_results: usize,
) -> Result<Vec<serde_json::Value>> {
    let resp = client
        .get("https://api.search.brave.com/res/v1/web/search")
        .header("X-Subscription-Token", api_key)
        .header("Accept", "application/json")
        .query(&[("q", query), ("count", &max_results.to_string())])
        .send()
        .await
        .map_err(|e| KittypawError::Skill(format!("Brave search error: {e}")))?;

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| KittypawError::Skill(format!("Brave parse error: {e}")))?;

    let results = body["web"]["results"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .take(max_results)
                .map(|r| {
                    serde_json::json!({
                        "title": r["title"].as_str().unwrap_or(""),
                        "snippet": r["description"].as_str().unwrap_or(""),
                        "url": r["url"].as_str().unwrap_or(""),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(results)
}

async fn tavily_search(
    client: &reqwest::Client,
    api_key: &str,
    query: &str,
    max_results: usize,
) -> Result<Vec<serde_json::Value>> {
    let resp = client
        .post("https://api.tavily.com/search")
        .json(&serde_json::json!({
            "api_key": api_key,
            "query": query,
            "max_results": max_results,
            "include_raw_content": false,
        }))
        .send()
        .await
        .map_err(|e| KittypawError::Skill(format!("Tavily search error: {e}")))?;

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| KittypawError::Skill(format!("Tavily parse error: {e}")))?;

    let results = body["results"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .take(max_results)
                .map(|r| {
                    serde_json::json!({
                        "title": r["title"].as_str().unwrap_or(""),
                        "snippet": r["content"].as_str().unwrap_or(""),
                        "url": r["url"].as_str().unwrap_or(""),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(results)
}

async fn exa_search(
    client: &reqwest::Client,
    api_key: &str,
    query: &str,
    max_results: usize,
) -> Result<Vec<serde_json::Value>> {
    let resp = client
        .post("https://api.exa.ai/search")
        .header("x-api-key", api_key)
        .json(&serde_json::json!({
            "query": query,
            "numResults": max_results,
            "type": "neural",
        }))
        .send()
        .await
        .map_err(|e| KittypawError::Skill(format!("Exa search error: {e}")))?;

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| KittypawError::Skill(format!("Exa parse error: {e}")))?;

    let results = body["results"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .take(max_results)
                .map(|r| {
                    serde_json::json!({
                        "title": r["title"].as_str().unwrap_or(""),
                        "snippet": r["text"].as_str().unwrap_or(""),
                        "url": r["url"].as_str().unwrap_or(""),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(results)
}

async fn ddg_instant_answer(
    client: &reqwest::Client,
    query: &str,
    max_results: usize,
) -> Result<Vec<serde_json::Value>> {
    let url = format!(
        "https://api.duckduckgo.com/?q={}&format=json&no_html=1&skip_disambig=1",
        urlencoding::encode(query)
    );
    let resp = client
        .get(&url)
        .header("User-Agent", "KittyPaw/0.1")
        .send()
        .await
        .map_err(|e| KittypawError::Skill(format!("DDG search error: {e}")))?;

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| KittypawError::Skill(format!("DDG parse error: {e}")))?;

    let mut results = Vec::new();

    if let Some(text) = body["AbstractText"].as_str() {
        if !text.is_empty() {
            results.push(serde_json::json!({
                "title": body["Heading"].as_str().unwrap_or(""),
                "snippet": text,
                "url": body["AbstractURL"].as_str().unwrap_or(""),
            }));
        }
    }

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

    Ok(results)
}
