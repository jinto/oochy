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
