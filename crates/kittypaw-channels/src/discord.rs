use async_trait::async_trait;
use kittypaw_core::error::{KittypawError, Result};
use kittypaw_core::types::Event;
use reqwest::Client;
use serde_json::json;
use tokio::sync::mpsc;
use tracing::info;

use crate::channel::Channel;

pub struct DiscordChannel {
    bot_token: String,
    client: Client,
}

impl DiscordChannel {
    pub fn new(bot_token: impl Into<String>) -> Self {
        Self {
            bot_token: bot_token.into(),
            client: Client::new(),
        }
    }
}

#[async_trait]
impl Channel for DiscordChannel {
    async fn start(&self, _event_tx: mpsc::Sender<Event>) -> Result<()> {
        // Gateway/polling not implemented for v2 — no-op
        info!("Discord channel start() called (no-op in v2)");
        Ok(())
    }

    async fn send_response(&self, channel_id: &str, response: &str) -> Result<()> {
        let url = format!(
            "https://discord.com/api/v10/channels/{}/messages",
            channel_id
        );
        let body = json!({
            "content": response,
        });

        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bot {}", self.bot_token))
            .json(&body)
            .send()
            .await
            .map_err(|e| KittypawError::Llm {
                kind: kittypaw_core::error::LlmErrorKind::Other,
                message: format!("Discord send message failed: {}", e),
            })?;

        let status = resp.status();
        if !status.is_success() {
            let body_text = resp
                .text()
                .await
                .unwrap_or_else(|_| "unknown error".to_string());
            return Err(KittypawError::Llm {
                kind: kittypaw_core::error::LlmErrorKind::Other,
                message: format!("Discord API error {}: {}", status, body_text),
            });
        }

        Ok(())
    }

    fn name(&self) -> &str {
        "discord"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_channel_name() {
        let ch = DiscordChannel::new("dummy-bot-token");
        assert_eq!(ch.name(), "discord");
    }
}
