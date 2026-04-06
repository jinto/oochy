use kittypaw_core::config::Config;
use kittypaw_core::error::{KittypawError, Result};
use kittypaw_core::types::SkillCall;

use super::resolve_channel_token;

pub(super) async fn execute_discord(
    call: &SkillCall,
    config: &Config,
) -> Result<serde_json::Value> {
    // Token resolution: secrets -> env -> config.channels
    let bot_token =
        resolve_channel_token(config, "discord", "discord_token", "KITTYPAW_DISCORD_TOKEN")
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
