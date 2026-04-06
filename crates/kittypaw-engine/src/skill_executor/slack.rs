use kittypaw_core::config::Config;
use kittypaw_core::error::{KittypawError, Result};
use kittypaw_core::types::SkillCall;

use super::resolve_channel_token;

pub(super) async fn execute_slack(call: &SkillCall, config: &Config) -> Result<serde_json::Value> {
    // Token resolution: secrets -> env -> config.channels
    let bot_token =
        resolve_channel_token(config, "slack", "slack_token", "KITTYPAW_SLACK_TOKEN")
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
