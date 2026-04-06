use kittypaw_core::config::Config;
use kittypaw_core::error::{KittypawError, Result};
use kittypaw_core::types::SkillCall;

use super::resolve_channel_token;

/// Extract chat_id (args[0]) and a second required arg (args[1]).
/// If chat_id is missing or invalid (e.g. "me"), falls back to the default
/// chat_id from secrets store. Also supports single-arg calls like
/// `Telegram.sendMessage("text only")` where chat_id is auto-resolved.
pub(super) fn require_telegram_args(
    call: &SkillCall,
    second_name: &str,
) -> Result<(String, String)> {
    let arg0 = call.args.first().and_then(|v| v.as_str()).unwrap_or("");
    let arg1 = call.args.get(1).and_then(|v| v.as_str()).unwrap_or("");

    // If only one arg provided (args.len() == 1), treat it as the message text
    if call.args.len() == 1 && !arg0.is_empty() {
        let default_id = resolve_default_chat_id()?;
        return Ok((default_id, arg0.to_string()));
    }

    // If chat_id looks invalid (not numeric, not starting with -), use default
    let chat_id = if arg0.is_empty() || (!arg0.starts_with('-') && arg0.parse::<i64>().is_err()) {
        resolve_default_chat_id()?
    } else {
        arg0.to_string()
    };

    if arg1.is_empty() {
        return Err(KittypawError::Skill(format!(
            "Telegram: missing {second_name}"
        )));
    }

    Ok((chat_id, arg1.to_string()))
}

fn resolve_default_chat_id() -> Result<String> {
    // Try secrets: telegram/chat_id (onboarding) or channels/telegram_chat_id (settings)
    kittypaw_core::secrets::get_secret("telegram", "chat_id")
        .ok()
        .flatten()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            kittypaw_core::secrets::get_secret("channels", "telegram_chat_id")
                .ok()
                .flatten()
                .filter(|s| !s.is_empty())
        })
        .ok_or_else(|| {
            KittypawError::Config(
                "Telegram: chat_id가 설정되지 않았습니다. 설정 위자드에서 텔레그램을 연결해주세요."
                    .into(),
            )
        })
}

pub(super) async fn execute_telegram(
    call: &SkillCall,
    config: &Config,
) -> Result<serde_json::Value> {
    // Token resolution chain (token is NOT passed via args — the JS ABI is
    // Telegram.sendMessage(chatId, text), so args carry only chat content):
    // 1. global channel secret from Settings
    // 2. environment variable fallback
    // 3. config.channels[*] where channel_type == "telegram"
    let bot_token = resolve_channel_token(
        config,
        "telegram",
        "telegram_token",
        "KITTYPAW_TELEGRAM_TOKEN",
    )
    .ok_or_else(|| KittypawError::Config("Telegram bot token not configured".into()))?;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| KittypawError::Skill(format!("Telegram client build error: {e}")))?;

    match call.method.as_str() {
        "sendMessage" => {
            // ABI: Telegram.sendMessage(chatId, text)
            let (chat_id, text) = require_telegram_args(call, "text")?;

            let chunks = kittypaw_core::telegram::split_telegram_text(
                &text,
                kittypaw_core::telegram::TELEGRAM_MAX_CHARS,
            );

            if chunks.is_empty() {
                return Ok(serde_json::json!({"ok": true}));
            }
            if chunks.len() > kittypaw_core::telegram::TELEGRAM_MAX_CHUNKS {
                return Err(KittypawError::Skill(format!(
                    "메시지가 너무 깁니다 ({} 청크, 최대 {})",
                    chunks.len(),
                    kittypaw_core::telegram::TELEGRAM_MAX_CHUNKS
                )));
            }

            let url = format!("https://api.telegram.org/bot{bot_token}/sendMessage");
            let mut body = serde_json::json!({"ok": true});
            for chunk in &chunks {
                let resp = client
                    .post(&url)
                    .json(&serde_json::json!({
                        "chat_id": chat_id,
                        "text": chunk,
                    }))
                    .send()
                    .await
                    .map_err(|e| KittypawError::Skill(format!("Telegram API error: {e}")))?;

                let status = resp.status();
                body = resp.json().await.map_err(|e| {
                    KittypawError::Skill(format!("Telegram response parse error: {e}"))
                })?;

                if !status.is_success() {
                    let err = body
                        .get("description")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown error");
                    return Err(KittypawError::Skill(format!(
                        "Telegram sendMessage error {status}: {err}"
                    )));
                }
            }
            Ok(body)
        }
        "sendPhoto" => {
            // ABI: Telegram.sendPhoto(chatId, photoUrl)
            let (chat_id, photo_url) = require_telegram_args(call, "photo_url")?;

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
            let (chat_id, file_url) = require_telegram_args(call, "file_url")?;
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
        "sendVoice" => {
            // ABI: Telegram.sendVoice(chatId, filePath, caption?)
            let (chat_id, file_path) = require_telegram_args(call, "file_path")?;
            let caption = call.args.get(2).and_then(|v| v.as_str()).unwrap_or("");

            let file_bytes = std::fs::read(&file_path)
                .map_err(|e| KittypawError::Skill(format!("Failed to read audio file: {e}")))?;
            // Clean up temp TTS file after reading
            if file_path.contains("kittypaw-tts") {
                let _ = std::fs::remove_file(&file_path);
            }
            let file_name = std::path::Path::new(&file_path)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();

            let url = format!("https://api.telegram.org/bot{bot_token}/sendVoice");
            let mut form = reqwest::multipart::Form::new()
                .text("chat_id", chat_id.to_string())
                .part(
                    "voice",
                    reqwest::multipart::Part::bytes(file_bytes)
                        .file_name(file_name)
                        .mime_str("audio/mpeg")
                        .unwrap(),
                );
            if !caption.is_empty() {
                form = form.text("caption", caption.to_string());
            }

            let resp = client
                .post(&url)
                .multipart(form)
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
                    "Telegram sendVoice error {status}: {err}"
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
