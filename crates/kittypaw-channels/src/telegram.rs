use async_trait::async_trait;
use kittypaw_core::error::{KittypawError, Result};
use kittypaw_core::types::{Event, EventType};
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use crate::channel::Channel;

pub struct TelegramChannel {
    bot_token: String,
    client: Client,
}

impl TelegramChannel {
    pub fn new(bot_token: impl Into<String>) -> Self {
        Self {
            bot_token: bot_token.into(),
            client: Client::new(),
        }
    }

    fn api_url(&self, method: &str) -> String {
        format!("https://api.telegram.org/bot{}/{}", self.bot_token, method)
    }
}

#[derive(Debug, Deserialize)]
struct TelegramResponse<T> {
    ok: bool,
    result: Option<T>,
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Update {
    update_id: i64,
    message: Option<Message>,
}

#[derive(Debug, Deserialize)]
struct Message {
    chat: Chat,
    text: Option<String>,
    from: Option<User>,
}

#[derive(Debug, Deserialize)]
struct Chat {
    id: i64,
}

#[derive(Debug, Deserialize)]
struct User {
    first_name: String,
    last_name: Option<String>,
    #[allow(dead_code)]
    username: Option<String>,
}

impl User {
    fn display_name(&self) -> String {
        if let Some(ref last) = self.last_name {
            format!("{} {}", self.first_name, last)
        } else {
            self.first_name.clone()
        }
    }
}

#[async_trait]
impl Channel for TelegramChannel {
    async fn start(&self, event_tx: mpsc::Sender<Event>) -> Result<()> {
        info!("Starting Telegram channel polling");
        let mut offset: i64 = 0;
        let mut backoff_secs: u64 = 1;

        loop {
            let url = self.api_url("getUpdates");
            let res = self
                .client
                .get(&url)
                .query(&[
                    ("offset", offset.to_string()),
                    ("timeout", "30".to_string()),
                    ("limit", "100".to_string()),
                ])
                .send()
                .await;

            match res {
                Err(e) => {
                    warn!(
                        "Telegram getUpdates network error: {}. Retrying in {}s",
                        e, backoff_secs
                    );
                    tokio::time::sleep(tokio::time::Duration::from_secs(backoff_secs)).await;
                    backoff_secs = (backoff_secs * 2).min(60);
                    continue;
                }
                Ok(resp) => {
                    backoff_secs = 1;
                    match resp.json::<TelegramResponse<Vec<Update>>>().await {
                        Err(e) => {
                            error!("Failed to parse Telegram response: {}", e);
                        }
                        Ok(tg_resp) => {
                            if !tg_resp.ok {
                                error!(
                                    "Telegram API error: {}",
                                    tg_resp.description.unwrap_or_default()
                                );
                                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                                continue;
                            }

                            if let Some(updates) = tg_resp.result {
                                for update in updates {
                                    offset = update.update_id + 1;

                                    if let Some(msg) = update.message {
                                        let text = match msg.text {
                                            Some(t) => t,
                                            None => continue,
                                        };
                                        let chat_id = msg.chat.id;
                                        let from_name = msg
                                            .from
                                            .as_ref()
                                            .map(|u| u.display_name())
                                            .unwrap_or_else(|| "unknown".to_string());

                                        let event = Event {
                                            event_type: EventType::Telegram,
                                            payload: json!({
                                                "chat_id": chat_id,
                                                "text": text,
                                                "from_name": from_name,
                                            }),
                                        };

                                        if event_tx.send(event).await.is_err() {
                                            info!(
                                                "Event receiver dropped, stopping Telegram polling"
                                            );
                                            return Ok(());
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    async fn send_response(&self, agent_id: &str, response: &str) -> Result<()> {
        // agent_id is used as the chat_id for Telegram
        let chat_id: i64 = agent_id.parse().map_err(|_| {
            KittypawError::Config(format!("Invalid Telegram chat_id: {}", agent_id))
        })?;

        let url = self.api_url("sendMessage");
        let body = json!({
            "chat_id": chat_id,
            "text": response,
        });

        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| KittypawError::Llm {
                kind: kittypaw_core::error::LlmErrorKind::Other,
                message: format!("Telegram sendMessage failed: {}", e),
            })?;

        let tg_resp: TelegramResponse<serde_json::Value> =
            resp.json().await.map_err(|e| KittypawError::Llm {
                kind: kittypaw_core::error::LlmErrorKind::Other,
                message: format!("Failed to parse sendMessage response: {}", e),
            })?;

        if !tg_resp.ok {
            return Err(KittypawError::Llm {
                kind: kittypaw_core::error::LlmErrorKind::Other,
                message: format!(
                    "Telegram sendMessage error: {}",
                    tg_resp.description.unwrap_or_default()
                ),
            });
        }

        Ok(())
    }

    fn name(&self) -> &str {
        "telegram"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_parse_update_with_message() {
        let raw = json!({
            "update_id": 123456789,
            "message": {
                "chat": { "id": 987654321_i64 },
                "text": "Hello, bot!",
                "from": {
                    "first_name": "Alice",
                    "last_name": "Smith",
                    "username": "alice"
                }
            }
        });

        let update: Update = serde_json::from_value(raw).unwrap();
        assert_eq!(update.update_id, 123456789);
        let msg = update.message.unwrap();
        assert_eq!(msg.chat.id, 987654321);
        assert_eq!(msg.text.unwrap(), "Hello, bot!");
        let from = msg.from.unwrap();
        assert_eq!(from.display_name(), "Alice Smith");
    }

    #[test]
    fn test_parse_update_no_last_name() {
        let raw = json!({
            "update_id": 1,
            "message": {
                "chat": { "id": 42_i64 },
                "text": "Hi",
                "from": { "first_name": "Bob" }
            }
        });

        let update: Update = serde_json::from_value(raw).unwrap();
        let msg = update.message.unwrap();
        let from = msg.from.unwrap();
        assert_eq!(from.display_name(), "Bob");
    }

    #[test]
    fn test_parse_update_no_message() {
        let raw = json!({
            "update_id": 999
        });
        let update: Update = serde_json::from_value(raw).unwrap();
        assert!(update.message.is_none());
    }

    #[test]
    fn test_parse_update_no_text() {
        let raw = json!({
            "update_id": 2,
            "message": {
                "chat": { "id": 10_i64 },
                "from": { "first_name": "Carol" }
            }
        });
        let update: Update = serde_json::from_value(raw).unwrap();
        let msg = update.message.unwrap();
        assert!(msg.text.is_none());
    }

    #[test]
    fn test_event_payload_structure() {
        let event = Event {
            event_type: EventType::Telegram,
            payload: json!({
                "chat_id": 12345_i64,
                "text": "test message",
                "from_name": "Test User",
            }),
        };
        assert_eq!(event.event_type, EventType::Telegram);
        assert_eq!(event.payload["chat_id"], 12345);
        assert_eq!(event.payload["text"], "test message");
        assert_eq!(event.payload["from_name"], "Test User");
    }

    #[test]
    fn test_channel_name() {
        let ch = TelegramChannel::new("dummy_token");
        assert_eq!(ch.name(), "telegram");
    }

    #[test]
    fn test_invalid_chat_id_returns_error() {
        // send_response with non-numeric agent_id should fail at parse
        let ch = TelegramChannel::new("dummy_token");
        let result = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(ch.send_response("not_a_number", "hi"));
        assert!(result.is_err());
    }
}
