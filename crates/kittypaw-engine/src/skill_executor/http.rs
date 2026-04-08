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

/// Search backend dispatch with fallback chain.
///
/// 1. If `search/backend` is explicitly set → use that backend
/// 2. If no backend set but API keys exist → legacy auto-detect (Brave>Tavily>Exa)
/// 3. Fallback: DuckDuckGo HTML Search (no key required)
///
/// If the configured/detected backend fails, automatically falls back to DDG.
async fn web_search_dispatch(query: &str, max_results: usize) -> Result<Vec<serde_json::Value>> {
    use kittypaw_core::secrets::get_secret;

    let max_results = max_results.min(20); // Cap to prevent abuse

    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none()) // SSRF defense
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());

    // Determine which backend to try first
    let backend = get_secret("search", "backend")
        .ok()
        .flatten()
        .unwrap_or_default();

    let primary_result = match backend.as_str() {
        // Explicit backend selection
        "brave" | "tavily" | "exa" => try_backend(&client, &backend, query, max_results).await,
        "ddg" => return ddg_html_search(&client, query, max_results).await,

        // No explicit backend → legacy auto-detect by API key presence
        _ => {
            let backends = ["brave", "tavily", "exa"];
            let mut result = None;
            for name in backends {
                if let Some(r) = try_backend(&client, name, query, max_results).await {
                    result = Some(r);
                    break;
                }
            }
            result
        }
    };

    // If primary backend succeeded, return its results
    if let Some(Ok(results)) = primary_result {
        return Ok(results);
    }

    // Log fallback if a primary backend was attempted but failed
    if let Some(Err(ref e)) = primary_result {
        tracing::warn!("Search backend failed, falling back to DuckDuckGo: {e}");
    }

    // Fallback: DuckDuckGo HTML Search (always available, no key required)
    ddg_html_search(&client, query, max_results)
        .await
        .map_err(|ddg_err| {
            let primary_msg = primary_result
                .as_ref()
                .and_then(|r| r.as_ref().err())
                .map(|e| format!("Primary: {e}. "))
                .unwrap_or_default();
            KittypawError::Skill(format!(
                "All search backends failed. {primary_msg}DuckDuckGo: {ddg_err}"
            ))
        })
}

/// Map backend name to its secret key name.
fn search_key_name(backend: &str) -> Option<&'static str> {
    match backend {
        "brave" => Some("brave_api_key"),
        "tavily" => Some("tavily_api_key"),
        "exa" => Some("exa_api_key"),
        _ => None,
    }
}

/// Try a named search backend, returning None if the API key is missing.
async fn try_backend(
    client: &reqwest::Client,
    name: &str,
    query: &str,
    max_results: usize,
) -> Option<Result<Vec<serde_json::Value>>> {
    use kittypaw_core::secrets::get_secret;

    let key_name = search_key_name(name)?;
    let key = get_secret("search", key_name)
        .ok()
        .flatten()
        .unwrap_or_default();
    if key.is_empty() {
        return None; // No key → skip silently, let fallback handle it
    }

    Some(match name {
        "brave" => brave_search(client, &key, query, max_results).await,
        "tavily" => tavily_search(client, &key, query, max_results).await,
        "exa" => exa_search(client, &key, query, max_results).await,
        _ => unreachable!(),
    })
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

/// Parse DuckDuckGo HTML search results into structured data.
///
/// Pure function — no network I/O, fully testable with HTML fixtures.
/// Extracts `<a class="result__a">` (title + URL) and `<a class="result__snippet">` (snippet).
fn parse_ddg_html(html: &str, max_results: usize) -> Vec<serde_json::Value> {
    use std::sync::LazyLock;

    // Match <a ... class="result__a" ... href="URL" ...>Title</a>
    // Handles both attribute orders: class before href, or href before class.
    static TITLE_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r#"<a[^>]*class="result__a"[^>]*href="([^"]*)"[^>]*>([\s\S]*?)</a>"#)
            .unwrap()
    });
    // Also match the reverse order: href before class
    static TITLE_RE_ALT: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r#"<a[^>]*href="([^"]*)"[^>]*class="result__a"[^>]*>([\s\S]*?)</a>"#)
            .unwrap()
    });
    static SNIPPET_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r#"<a[^>]*class="result__snippet"[^>]*>([\s\S]*?)</a>"#).unwrap()
    });

    let titles: Vec<(&str, &str)> = TITLE_RE
        .captures_iter(html)
        .chain(TITLE_RE_ALT.captures_iter(html))
        .map(|c| (c.get(1).unwrap().as_str(), c.get(2).unwrap().as_str()))
        .collect();

    let snippets: Vec<&str> = SNIPPET_RE
        .captures_iter(html)
        .map(|c| c.get(1).unwrap().as_str())
        .collect();

    titles
        .into_iter()
        .zip(snippets.into_iter().chain(std::iter::repeat_with(|| "")))
        .take(max_results)
        .map(|((url, title), snippet)| {
            // Handle potential DDG redirect URLs (//duckduckgo.com/l/?uddg=ENCODED_URL)
            let resolved_url = resolve_ddg_url(url);
            serde_json::json!({
                "title": strip_html_inline(title),
                "snippet": strip_html_inline(snippet),
                "url": resolved_url,
            })
        })
        .collect()
}

/// Resolve DuckDuckGo redirect URLs to the actual target URL.
/// Direct URLs pass through unchanged.
fn resolve_ddg_url(url: &str) -> String {
    if url.contains("duckduckgo.com/l/?") {
        // Extract the `uddg` parameter (the actual URL, percent-encoded)
        if let Some(pos) = url.find("uddg=") {
            let encoded = &url[pos + 5..];
            let end = encoded.find('&').unwrap_or(encoded.len());
            return urlencoding::decode(&encoded[..end])
                .unwrap_or_else(|_| encoded[..end].into())
                .into_owned();
        }
    }
    url.to_string()
}

/// Strip inline HTML tags (e.g. `<b>`, `</b>`) and decode common entities.
fn strip_html_inline(s: &str) -> String {
    use std::sync::LazyLock;
    static TAG_RE: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"<[^>]+>").unwrap());

    let no_tags = TAG_RE.replace_all(s, "");
    no_tags
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
}

async fn ddg_html_search(
    client: &reqwest::Client,
    query: &str,
    max_results: usize,
) -> Result<Vec<serde_json::Value>> {
    let resp = client
        .post("https://html.duckduckgo.com/html/")
        .header("User-Agent", "Mozilla/5.0 (compatible; KittyPaw/0.1)")
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(format!("q={}&b=&kl=", urlencoding::encode(query)))
        .send()
        .await
        .map_err(|e| KittypawError::Skill(format!("DDG HTML search error: {e}")))?;

    let html = resp
        .text()
        .await
        .map_err(|e| KittypawError::Skill(format!("DDG HTML read error: {e}")))?;

    let results = parse_ddg_html(&html, max_results);
    if results.is_empty() {
        return Err(KittypawError::Skill(
            "DDG HTML search: no results parsed (HTML structure may have changed)".into(),
        ));
    }
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DDG_HTML_FIXTURE: &str = r#"
        <div class="result results_links results_links_deep web-result ">
          <div class="links_main links_deep result__body">
            <h2 class="result__title">
              <a rel="nofollow" class="result__a" href="https://rust-lang.org/">Rust Programming Language</a>
            </h2>
            <a class="result__snippet" href="https://rust-lang.org/">A language empowering everyone to build reliable and efficient software.</a>
          </div>
        </div>
        <div class="result results_links results_links_deep web-result ">
          <div class="links_main links_deep result__body">
            <h2 class="result__title">
              <a rel="nofollow" class="result__a" href="https://en.wikipedia.org/wiki/Rust_(programming_language)">Rust (programming language) - Wikipedia</a>
            </h2>
            <a class="result__snippet" href="https://en.wikipedia.org/wiki/Rust_(programming_language)"><b>Rust</b> is a general-purpose <b>programming</b> language noted for its emphasis on performance.</a>
          </div>
        </div>
        <div class="result results_links results_links_deep web-result ">
          <div class="links_main links_deep result__body">
            <h2 class="result__title">
              <a rel="nofollow" class="result__a" href="https://doc.rust-lang.org/book/">The Rust Programming Language - Rust</a>
            </h2>
            <a class="result__snippet" href="https://doc.rust-lang.org/book/">The official <b>Rust</b> book &#x27;The <b>Rust</b> <b>Programming</b> Language&#x27; by Steve Klabnik &amp; Carol Nichols.</a>
          </div>
        </div>
    "#;

    #[test]
    fn parse_ddg_html_extracts_results() {
        let results = parse_ddg_html(DDG_HTML_FIXTURE, 10);
        assert_eq!(results.len(), 3);

        assert_eq!(results[0]["title"], "Rust Programming Language");
        assert_eq!(results[0]["url"], "https://rust-lang.org/");
        assert!(results[0]["snippet"]
            .as_str()
            .unwrap()
            .contains("empowering everyone"));

        assert_eq!(
            results[1]["url"],
            "https://en.wikipedia.org/wiki/Rust_(programming_language)"
        );
    }

    #[test]
    fn parse_ddg_html_strips_bold_tags() {
        let results = parse_ddg_html(DDG_HTML_FIXTURE, 10);
        let snippet = results[1]["snippet"].as_str().unwrap();
        assert!(!snippet.contains("<b>"));
        assert!(snippet.contains("Rust is a general-purpose"));
    }

    #[test]
    fn parse_ddg_html_decodes_entities() {
        let results = parse_ddg_html(DDG_HTML_FIXTURE, 10);
        let snippet = results[2]["snippet"].as_str().unwrap();
        assert!(snippet.contains("'The Rust Programming Language'"));
        assert!(snippet.contains("Steve Klabnik & Carol Nichols"));
    }

    #[test]
    fn parse_ddg_html_respects_max_results() {
        let results = parse_ddg_html(DDG_HTML_FIXTURE, 2);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn parse_ddg_html_empty_html() {
        let results = parse_ddg_html("<html><body>no results</body></html>", 5);
        assert!(results.is_empty());
    }

    #[test]
    fn resolve_ddg_url_direct() {
        assert_eq!(
            resolve_ddg_url("https://rust-lang.org/"),
            "https://rust-lang.org/"
        );
    }

    #[test]
    fn resolve_ddg_url_redirect() {
        let redirect = "//duckduckgo.com/l/?uddg=https%3A%2F%2Frust-lang.org%2F&rut=abc";
        assert_eq!(resolve_ddg_url(redirect), "https://rust-lang.org/");
    }

    #[test]
    fn parse_ddg_html_reversed_attr_order() {
        let html = r#"
            <a href="https://example.com/" class="result__a">Example</a>
            <a class="result__snippet">A snippet.</a>
        "#;
        let results = parse_ddg_html(html, 5);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["url"], "https://example.com/");
        assert_eq!(results[0]["title"], "Example");
    }
}
