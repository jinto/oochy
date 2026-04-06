use crate::error::{KittypawError, Result};

/// Fetch the most recent chat_id from Telegram Bot API getUpdates.
///
/// The user must send at least one message to the bot before calling this.
pub async fn fetch_chat_id(token: &str) -> Result<String> {
    let url = format!("https://api.telegram.org/bot{token}/getUpdates?limit=1&offset=-1");
    let client = reqwest::Client::new();
    let resp = client
        .get(&url)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| KittypawError::Skill(format!("Telegram API 요청 실패: {e}")))?;

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| KittypawError::Skill(format!("Telegram 응답 파싱 실패: {e}")))?;

    if body["ok"].as_bool() != Some(true) {
        return Err(KittypawError::Config("봇 토큰이 유효하지 않습니다".into()));
    }

    let results = body["result"].as_array().ok_or_else(|| {
        KittypawError::Skill("결과가 비어있습니다. 봇에게 먼저 메시지를 보내주세요".into())
    })?;

    if results.is_empty() {
        return Err(KittypawError::Skill(
            "봇에게 먼저 메시지를 보내주세요".into(),
        ));
    }

    for result in results {
        if let Some(id) = result["message"]["chat"]["id"].as_i64() {
            return Ok(id.to_string());
        }
        if let Some(id) = result["channel_post"]["chat"]["id"].as_i64() {
            return Ok(id.to_string());
        }
    }

    Err(KittypawError::Skill(
        "채팅 ID를 찾을 수 없습니다. 봇에게 메시지를 보낸 후 다시 시도하세요".into(),
    ))
}

/// Split text into chunks that fit within Telegram's message size limit.
///
/// Strategy: prefer splitting at newline boundaries; fall back to char-boundary
/// splitting for lines that exceed `max_chars` on their own.
pub fn split_telegram_text(text: &str, max_chars: usize) -> Vec<String> {
    assert!(max_chars >= 1, "max_chars must be >= 1");

    if text.is_empty() {
        return Vec::new();
    }

    if text.chars().count() <= max_chars {
        return vec![text.to_string()];
    }

    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut current_len: usize = 0; // char count

    for line in text.split('\n') {
        let line_len = line.chars().count();

        // Single line exceeds max → force-split by char boundary
        if line_len > max_chars {
            // Flush current buffer first
            if !current.is_empty() {
                chunks.push(current);
                current = String::new();
                current_len = 0;
            }
            // Split long line into max_chars-sized pieces
            let mut chars = line.chars();
            let mut piece = String::new();
            let mut piece_len = 0;
            for ch in &mut chars {
                piece.push(ch);
                piece_len += 1;
                if piece_len == max_chars {
                    chunks.push(piece);
                    piece = String::new();
                    piece_len = 0;
                }
            }
            if !piece.is_empty() {
                current = piece;
                current_len = piece_len;
            }
            continue;
        }

        // Would adding this line (+ newline separator) overflow?
        let separator_cost = if current.is_empty() { 0 } else { 1 }; // '\n'
        if current_len + separator_cost + line_len > max_chars {
            // Flush current chunk
            if !current.is_empty() {
                chunks.push(current);
            }
            current = line.to_string();
            current_len = line_len;
        } else {
            if !current.is_empty() {
                current.push('\n');
                current_len += 1;
            }
            current.push_str(line);
            current_len += line_len;
        }
    }

    if !current.is_empty() {
        chunks.push(current);
    }

    chunks
}

/// Telegram's maximum message length in characters.
pub const TELEGRAM_MAX_CHARS: usize = 4096;

/// Maximum number of chunks to send in a single split message.
/// 4096 * 10 = ~40KB — practical upper bound for any reasonable message.
pub const TELEGRAM_MAX_CHUNKS: usize = 10;

/// Send a text message via Telegram Bot API, auto-splitting if needed.
///
/// This is the **single gateway** for all Telegram text sends.  Callers
/// (engine skill executor, channel adapter, core convenience fn) should
/// use this instead of reimplementing the split+loop pattern.
///
/// Returns the last chunk's API response JSON.
pub async fn send_text_chunked(
    client: &reqwest::Client,
    token: &str,
    chat_id: &str,
    text: &str,
) -> Result<serde_json::Value> {
    let chunks = split_telegram_text(text, TELEGRAM_MAX_CHARS);

    if chunks.is_empty() {
        return Ok(serde_json::json!({"ok": true}));
    }
    if chunks.len() > TELEGRAM_MAX_CHUNKS {
        return Err(KittypawError::Skill(format!(
            "메시지가 너무 깁니다 ({} 청크, 최대 {TELEGRAM_MAX_CHUNKS})",
            chunks.len()
        )));
    }

    let url = format!("https://api.telegram.org/bot{token}/sendMessage");
    let mut last_body = serde_json::json!({"ok": true});

    for chunk in &chunks {
        let resp = client
            .post(&url)
            .json(&serde_json::json!({
                "chat_id": chat_id,
                "text": chunk,
            }))
            .send()
            .await
            .map_err(|e| KittypawError::Skill(format!("Telegram 전송 실패: {e}")))?;

        let status = resp.status();
        last_body = resp
            .json()
            .await
            .map_err(|e| KittypawError::Skill(format!("Telegram 응답 파싱 실패: {e}")))?;

        if !status.is_success() {
            let err = last_body["description"].as_str().unwrap_or("unknown error");
            return Err(KittypawError::Skill(format!(
                "Telegram sendMessage error {status}: {err}"
            )));
        }
    }

    Ok(last_body)
}

/// Convenience wrapper: creates a one-off client and discards the response body.
pub async fn send_message(token: &str, chat_id: &str, text: &str) -> Result<()> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());
    send_text_chunked(&client, token, chat_id, text)
        .await
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_short_text_single_chunk() {
        let text = "a".repeat(100);
        let chunks = split_telegram_text(&text, 4096);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], text);
    }

    #[test]
    fn split_exact_limit_single_chunk() {
        let text = "b".repeat(4096);
        let chunks = split_telegram_text(&text, 4096);
        assert_eq!(chunks.len(), 1);
    }

    #[test]
    fn split_long_text_with_newlines() {
        // 80-char lines separated by newlines → ~8000 chars
        let line = "x".repeat(80);
        let text = (0..100)
            .map(|_| line.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.chars().count() > 4096);

        let chunks = split_telegram_text(&text, 4096);
        assert!(chunks.len() >= 2);
        for chunk in &chunks {
            assert!(
                chunk.chars().count() <= 4096,
                "chunk too long: {}",
                chunk.chars().count()
            );
        }
        // Reassembled content should equal original
        assert_eq!(chunks.join("\n"), text);
    }

    #[test]
    fn split_empty_string() {
        let chunks = split_telegram_text("", 4096);
        assert!(chunks.is_empty());
    }

    #[test]
    fn split_no_newlines_force_char_boundary() {
        let text = "c".repeat(5000);
        let chunks = split_telegram_text(&text, 4096);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].len(), 4096);
        assert_eq!(chunks[1].len(), 904);
    }

    #[test]
    fn split_korean_multibyte_safe() {
        // 4000 Korean chars = 12000 UTF-8 bytes but 4000 code points
        let text = "가".repeat(4000);
        let chunks = split_telegram_text(&text, 4096);
        assert_eq!(chunks.len(), 1); // 4000 < 4096

        // 5000 Korean chars → should split
        let text2 = "나".repeat(5000);
        let chunks2 = split_telegram_text(&text2, 4096);
        assert_eq!(chunks2.len(), 2);
        assert_eq!(chunks2[0].chars().count(), 4096);
        assert_eq!(chunks2[1].chars().count(), 904);
    }

    #[test]
    fn split_max_chars_one() {
        let text = "abc";
        let chunks = split_telegram_text(text, 1);
        assert_eq!(chunks, vec!["a", "b", "c"]);
    }

    #[test]
    #[should_panic(expected = "max_chars must be >= 1")]
    fn split_max_chars_zero_panics() {
        split_telegram_text("test", 0);
    }
}
