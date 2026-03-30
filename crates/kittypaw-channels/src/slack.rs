use async_trait::async_trait;
use kittypaw_core::error::{KittypawError, Result};
use kittypaw_core::types::Event;
use reqwest::Client;
use serde_json::json;
use tokio::sync::mpsc;
use tracing::info;

use crate::channel::Channel;

pub struct SlackChannel {
    bot_token: String,
    client: Client,
}

impl SlackChannel {
    pub fn new(bot_token: impl Into<String>) -> Self {
        Self {
            bot_token: bot_token.into(),
            client: Client::new(),
        }
    }
}

#[async_trait]
impl Channel for SlackChannel {
    async fn start(&self, _event_tx: mpsc::Sender<Event>) -> Result<()> {
        // Polling/Events API not implemented for v2 — no-op
        info!("Slack channel start() called (no-op in v2)");
        Ok(())
    }

    async fn send_response(&self, channel_id: &str, response: &str) -> Result<()> {
        let url = "https://slack.com/api/chat.postMessage";
        let body = json!({
            "channel": channel_id,
            "text": response,
        });

        let resp = self
            .client
            .post(url)
            .header("Authorization", format!("Bearer {}", self.bot_token))
            .json(&body)
            .send()
            .await
            .map_err(|e| KittypawError::Llm(format!("Slack postMessage failed: {}", e)))?;

        let slack_resp: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| KittypawError::Llm(format!("Failed to parse Slack response: {}", e)))?;

        if !slack_resp
            .get("ok")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            let err = slack_resp
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error");
            return Err(KittypawError::Llm(format!(
                "Slack postMessage error: {}",
                err
            )));
        }

        Ok(())
    }

    fn name(&self) -> &str {
        "slack"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_channel_name() {
        let ch = SlackChannel::new("xoxb-dummy-token");
        assert_eq!(ch.name(), "slack");
    }
}
