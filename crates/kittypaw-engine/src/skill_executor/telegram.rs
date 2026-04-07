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
    let config = kittypaw_core::config::Config::default();
    kittypaw_core::credential::resolve_credential("telegram", "chat_id", "", &config).ok_or_else(
        || {
            KittypawError::Config(
                "Telegram: chat_id가 설정되지 않았습니다. 설정 위자드에서 텔레그램을 연결해주세요."
                    .into(),
            )
        },
    )
}

/// For media methods (sendVoice, sendDocument, sendPhoto) where LLM-generated code
/// typically omits chatId: `Method(content)`, `Method(content, caption)`,
/// or `Method(chatId, content, caption?)`.
/// Returns `(chat_id, content, extra)`. Auto-resolves chat_id when arg0 is not numeric.
fn require_telegram_media_args(
    call: &SkillCall,
    content_name: &str,
) -> Result<(String, String, String)> {
    let arg = |i: usize| {
        call.args
            .get(i)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };
    let is_chat_id = |s: &str| !s.is_empty() && s.parse::<i64>().is_ok();

    match call.args.len() {
        0 | 1 => {
            let content = arg(0);
            if content.is_empty() {
                return Err(KittypawError::Skill(format!(
                    "Telegram: missing {content_name}"
                )));
            }
            Ok((resolve_default_chat_id()?, content, String::new()))
        }
        2 => {
            let (a0, a1) = (arg(0), arg(1));
            if is_chat_id(&a0) {
                // Method(chatId, content)
                Ok((a0, a1, String::new()))
            } else {
                // Method(content, caption) — auto chat_id
                Ok((resolve_default_chat_id()?, a0, a1))
            }
        }
        _ => {
            // Method(chatId, content, caption)
            let (a0, a1, a2) = (arg(0), arg(1), arg(2));
            let chat_id = if is_chat_id(&a0) {
                a0
            } else {
                resolve_default_chat_id()?
            };
            Ok((chat_id, a1, a2))
        }
    }
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
            kittypaw_core::telegram::send_text_chunked(&client, &bot_token, &chat_id, &text)
                .await?;
            // Return null — the message is already delivered, so returning a value
            // would cause the sandbox output to be re-sent to the user as a second message.
            Ok(serde_json::Value::Null)
        }
        "sendPhoto" => {
            // ABI: sendPhoto(photoUrl) / sendPhoto(photoUrl, caption) / sendPhoto(chatId, photoUrl, caption?)
            let (chat_id, photo_url, caption) = require_telegram_media_args(call, "photo_url")?;

            let url = format!("https://api.telegram.org/bot{bot_token}/sendPhoto");
            let mut payload = serde_json::json!({
                "chat_id": chat_id,
                "photo": photo_url,
            });
            if !caption.is_empty() {
                payload["caption"] = serde_json::Value::String(caption);
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
                    "Telegram sendPhoto error {status}: {err}"
                )));
            }
            Ok(body)
        }
        "sendDocument" => {
            // ABI: sendDocument(fileUrl) / sendDocument(fileUrl, caption) / sendDocument(chatId, fileUrl, caption?)
            let (chat_id, file_url, caption) = require_telegram_media_args(call, "file_url")?;

            let url = format!("https://api.telegram.org/bot{bot_token}/sendDocument");
            let mut payload = serde_json::json!({
                "chat_id": chat_id,
                "document": file_url,
            });
            if !caption.is_empty() {
                payload["caption"] = serde_json::Value::String(caption);
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
            // ABI: sendVoice(filePath) / sendVoice(filePath, caption) / sendVoice(chatId, filePath, caption?)
            let (chat_id, file_path, caption) = require_telegram_media_args(call, "file_path")?;

            // Restrict to TTS temp directory to prevent arbitrary file exfiltration
            let tts_dir = std::fs::canonicalize(std::env::temp_dir().join("kittypaw-tts"))
                .unwrap_or_else(|_| std::env::temp_dir().join("kittypaw-tts"));
            let canonical = std::fs::canonicalize(&file_path)
                .map_err(|e| KittypawError::Skill(format!("Invalid audio path: {e}")))?;
            if !canonical.starts_with(&tts_dir) {
                return Err(KittypawError::Skill(
                    "sendVoice: only TTS-generated audio files are allowed".into(),
                ));
            }
            let file_bytes = std::fs::read(&canonical)
                .map_err(|e| KittypawError::Skill(format!("Failed to read audio file: {e}")))?;
            let _ = std::fs::remove_file(&canonical);
            let file_name = std::path::Path::new(&file_path)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();

            let url = format!("https://api.telegram.org/bot{bot_token}/sendVoice");
            let mut form = reqwest::multipart::Form::new()
                .text("chat_id", chat_id)
                .part(
                    "voice",
                    reqwest::multipart::Part::bytes(file_bytes)
                        .file_name(file_name)
                        .mime_str("audio/mpeg")
                        .unwrap(),
                );
            if !caption.is_empty() {
                form = form.text("caption", caption);
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
