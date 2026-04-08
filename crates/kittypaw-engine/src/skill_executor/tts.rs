use edge_tts_rust::{EdgeTtsClient, SpeakOptions};
use kittypaw_core::error::{KittypawError, Result};
use kittypaw_core::types::SkillCall;

const DEFAULT_VOICE: &str = "ko-KR-SunHiNeural";

/// Minimum text length to trigger the LLM polish step.
/// Short texts (greetings, single sentences) skip polishing.
const POLISH_MIN_LENGTH: usize = 100;

pub(super) async fn execute_tts(
    call: &SkillCall,
    config: &kittypaw_core::config::Config,
) -> Result<serde_json::Value> {
    match call.method.as_str() {
        "speak" => {
            let text = call.args.first().and_then(|v| v.as_str()).unwrap_or("");
            if text.is_empty() {
                return Err(KittypawError::Skill("Tts.speak: text is required".into()));
            }

            // Optional second arg: { voice, rate, pitch } or just a voice string
            let raw_voice = call
                .args
                .get(1)
                .and_then(|v| {
                    v.as_str()
                        .map(String::from)
                        .or_else(|| v.get("voice").and_then(|v| v.as_str()).map(String::from))
                })
                .unwrap_or_else(|| DEFAULT_VOICE.to_string());
            // Edge TTS voices follow the pattern "xx-XX-NameNeural" (≥2 hyphens).
            // Bare locales like "ko-KR" (1 hyphen) are not valid voice names.
            let voice = if raw_voice.matches('-').count() >= 2 {
                raw_voice
            } else {
                DEFAULT_VOICE.to_string()
            };

            let rate = call
                .args
                .get(1)
                .and_then(|v| v.get("rate"))
                .and_then(|v| v.as_str())
                .unwrap_or("+0%")
                .to_string();

            let pitch = call
                .args
                .get(1)
                .and_then(|v| v.get("pitch"))
                .and_then(|v| v.as_str())
                .unwrap_or("+0Hz")
                .to_string();

            // Pipeline: normalize → polish (LLM) → synthesize
            let cleaned = normalize_for_tts(text);
            let spoken = polish_for_speech(&cleaned, config).await;

            let client = EdgeTtsClient::new()
                .map_err(|e| KittypawError::Skill(format!("TTS client error: {e}")))?;

            let result = client
                .synthesize(
                    &spoken,
                    SpeakOptions {
                        voice,
                        rate,
                        pitch,
                        ..SpeakOptions::default()
                    },
                )
                .await
                .map_err(|e| KittypawError::Skill(format!("TTS synthesis error: {e}")))?;

            // Write to temp file
            let tts_dir = std::env::temp_dir().join("kittypaw-tts");
            std::fs::create_dir_all(&tts_dir)?;
            let filename = format!("{}.mp3", uuid_short());
            let path = tts_dir.join(&filename);
            std::fs::write(&path, &result.audio)?;

            Ok(serde_json::json!({
                "path": path.to_string_lossy(),
                "size": result.audio.len(),
            }))
        }
        _ => Err(KittypawError::CapabilityDenied(format!(
            "Unknown Tts method: {}",
            call.method
        ))),
    }
}

/// Polish raw text into a natural spoken-language script via LLM.
/// Only runs for texts longer than POLISH_MIN_LENGTH.
/// Falls back to the original text if the LLM call fails for any reason.
async fn polish_for_speech(text: &str, config: &kittypaw_core::config::Config) -> String {
    if text.len() < POLISH_MIN_LENGTH {
        return text.to_string();
    }

    let prompt = format!(
        "다음 텍스트를 라디오 뉴스 앵커가 읽는 자연스러운 음성 스크립트로 다듬어주세요.\n\
         규칙:\n\
         - 사이트 소개, URL, 메타 설명 문구는 제거\n\
         - 핵심 사실만 추출하여 자연스러운 문장으로 연결\n\
         - 전환어 사용 (\"한편\", \"또한\", \"다음 소식입니다\")\n\
         - 30초~1분 분량으로 압축\n\
         - 인사로 시작, 마무리 인사로 끝\n\
         - 스크립트 본문만 출력, 다른 설명 없이\n\n\
         텍스트:\n{text}"
    );

    match call_llm_for_polish(&prompt, config).await {
        Ok(polished) if !polished.is_empty() => {
            tracing::info!("TTS polish: {}B → {}B", text.len(), polished.len());
            polished
        }
        Ok(_) => text.to_string(),
        Err(e) => {
            tracing::warn!("TTS polish failed, using original text: {e}");
            text.to_string()
        }
    }
}

/// Lightweight LLM call for text polishing. Uses the default provider from config.
async fn call_llm_for_polish(
    prompt: &str,
    config: &kittypaw_core::config::Config,
) -> std::result::Result<String, String> {
    let api_key = if config.llm.api_key.is_empty() {
        kittypaw_core::secrets::get_secret("llm", "api_key")
            .ok()
            .flatten()
            .unwrap_or_default()
    } else {
        config.llm.api_key.clone()
    };

    let provider = config.llm.provider.to_lowercase();
    let model = &config.llm.model;

    if api_key.is_empty() && !matches!(provider.as_str(), "ollama" | "local") {
        return Err("No LLM API key configured".into());
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| format!("HTTP client error: {e}"))?;

    let is_openai_compat = matches!(provider.as_str(), "openai" | "ollama" | "local");
    // Check config.models for a default model with base_url (e.g. Ollama)
    let base_url: Option<&str> = config
        .models
        .iter()
        .find(|m| m.default || m.provider.to_lowercase() == provider)
        .and_then(|m| m.base_url.as_deref());
    let endpoint = if is_openai_compat {
        let url = base_url
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
                "max_tokens": 512,
                "messages": [{"role": "user", "content": prompt}]
            }));
        if !api_key.is_empty() {
            req = req.header("Authorization", format!("Bearer {api_key}"));
        }
        req.send()
            .await
            .map_err(|e| format!("LLM API error: {e}"))?
    } else {
        client
            .post(&endpoint)
            .header("x-api-key", &api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&serde_json::json!({
                "model": model,
                "max_tokens": 512,
                "messages": [{"role": "user", "content": prompt}]
            }))
            .send()
            .await
            .map_err(|e| format!("LLM API error: {e}"))?
    };

    if !resp.status().is_success() {
        return Err(format!("LLM API status: {}", resp.status()));
    }

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("LLM response parse error: {e}"))?;

    let text = if is_openai_compat {
        body["choices"]
            .as_array()
            .and_then(|arr| arr.first())
            .and_then(|c| c["message"]["content"].as_str())
    } else {
        body["content"]
            .as_array()
            .and_then(|arr| arr.first())
            .and_then(|block| block["text"].as_str())
    };

    Ok(text.unwrap_or("").to_string())
}

/// Strip URLs, markdown syntax, and emojis so TTS reads clean natural language.
fn normalize_for_tts(text: &str) -> String {
    use regex::Regex;
    use std::sync::OnceLock;

    static RE: OnceLock<Vec<(Regex, &str)>> = OnceLock::new();
    let rules = RE.get_or_init(|| {
        vec![
            // Markdown links [text](url) → text
            (Regex::new(r"\[([^\]]+)\]\([^)]+\)").unwrap(), "$1"),
            // URLs
            (Regex::new(r"https?://\S+").unwrap(), ""),
            // Markdown headings
            (Regex::new(r"#{1,6}\s*").unwrap(), ""),
            // Bold/italic/strikethrough
            (Regex::new(r"[*_~`]{1,3}").unwrap(), ""),
            // Bullet markers at line start
            (Regex::new(r"(?m)^\s*[-*•]\s+").unwrap(), ""),
            // Emoji (Unicode Emoji range)
            (
                Regex::new(r"[\p{Emoji_Presentation}\p{Extended_Pictographic}]").unwrap(),
                "",
            ),
            // Collapse whitespace
            (Regex::new(r"[ \t]+").unwrap(), " "),
            // Collapse multiple newlines
            (Regex::new(r"\n{3,}").unwrap(), "\n\n"),
        ]
    });

    let mut result = text.to_string();
    for (re, replacement) in rules {
        result = re.replace_all(&result, *replacement).to_string();
    }
    result.trim().to_string()
}

fn uuid_short() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!("{:x}{:x}", t.as_secs(), t.subsec_nanos())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_strips_urls() {
        let input = "뉴스입니다 https://example.com/news 참고하세요";
        assert_eq!(normalize_for_tts(input), "뉴스입니다 참고하세요");
    }

    #[test]
    fn test_normalize_markdown_links() {
        let input = "[블룸버그](https://bloomberg.com) 기사";
        assert_eq!(normalize_for_tts(input), "블룸버그 기사");
    }

    #[test]
    fn test_normalize_strips_markdown_formatting() {
        let input = "## 제목\n**굵은 글씨**와 *기울임*";
        assert_eq!(normalize_for_tts(input), "제목\n굵은 글씨와 기울임");
    }

    #[test]
    fn test_normalize_strips_emojis() {
        let input = "🤖 AI 뉴스 요약 🔊";
        assert_eq!(normalize_for_tts(input), "AI 뉴스 요약");
    }

    #[test]
    fn test_normalize_collapses_whitespace() {
        let input = "hello   world\n\n\n\ntest";
        assert_eq!(normalize_for_tts(input), "hello world\n\ntest");
    }

    #[tokio::test]
    async fn test_polish_skips_short_text() {
        let config = kittypaw_core::config::Config::default();
        let short = "안녕하세요, 오늘 날씨가 좋네요.";
        assert!(short.len() < POLISH_MIN_LENGTH);
        let result = polish_for_speech(short, &config).await;
        assert_eq!(result, short, "Short text should pass through unchanged");
    }

    #[tokio::test]
    async fn test_polish_fallback_on_no_api_key() {
        // Default config has empty api_key → polish should fall back to original
        let config = kittypaw_core::config::Config::default();
        let long = "AI 뉴스 요약입니다. ".repeat(20); // >100 chars
        assert!(long.len() >= POLISH_MIN_LENGTH);
        let result = polish_for_speech(&long, &config).await;
        assert_eq!(result, long, "Without API key, should return original text");
    }

    #[tokio::test]
    async fn test_call_llm_for_polish_no_key_returns_err() {
        let config = kittypaw_core::config::Config::default();
        let result = call_llm_for_polish("test prompt", &config).await;
        assert!(result.is_err());
        assert!(
            result.unwrap_err().contains("No LLM API key"),
            "Should report missing API key"
        );
    }
}
