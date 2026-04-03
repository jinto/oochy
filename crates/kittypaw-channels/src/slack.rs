use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use kittypaw_core::error::{KittypawError, Result};
use kittypaw_core::types::{Event, EventType};
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;
use tokio::sync::mpsc;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tracing::{error, info, warn};

use crate::channel::Channel;

pub struct SlackChannel {
    bot_token: String,
    app_token: String,
    client: Client,
}

impl SlackChannel {
    pub fn new(bot_token: impl Into<String>, app_token: impl Into<String>) -> Self {
        Self {
            bot_token: bot_token.into(),
            app_token: app_token.into(),
            client: Client::new(),
        }
    }

    async fn open_socket_url(&self) -> Result<String> {
        let resp = self
            .client
            .post("https://slack.com/api/apps.connections.open")
            .header("Authorization", format!("Bearer {}", self.app_token))
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body("")
            .send()
            .await
            .map_err(|e| KittypawError::Llm {
                kind: kittypaw_core::error::LlmErrorKind::Other,
                message: format!("apps.connections.open request failed: {}", e),
            })?;

        let body: serde_json::Value = resp.json().await.map_err(|e| KittypawError::Llm {
            kind: kittypaw_core::error::LlmErrorKind::Other,
            message: format!("Failed to parse apps.connections.open response: {}", e),
        })?;

        if !body.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
            let err = body
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error");
            return Err(KittypawError::Llm {
                kind: kittypaw_core::error::LlmErrorKind::Other,
                message: format!("apps.connections.open error: {}", err),
            });
        }

        body.get("url")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| KittypawError::Llm {
                kind: kittypaw_core::error::LlmErrorKind::Other,
                message: "apps.connections.open: missing url".to_string(),
            })
    }
}

// ── Deserialization structs ────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct SlackSocketFrame {
    #[serde(rename = "type")]
    frame_type: String,
    envelope_id: Option<String>,
    payload: Option<SlackEventPayload>,
}

#[derive(Debug, Deserialize)]
struct SlackEventPayload {
    event: Option<SlackMessageEvent>,
}

#[derive(Debug, Deserialize)]
struct SlackMessageEvent {
    #[serde(rename = "type")]
    event_type: Option<String>,
    channel: Option<String>,
    text: Option<String>,
    user: Option<String>,
}

// ── Channel impl ──────────────────────────────────────────────────────────

#[async_trait]
impl Channel for SlackChannel {
    async fn start(&self, event_tx: mpsc::Sender<Event>) -> Result<()> {
        info!("Starting Slack Socket Mode channel");
        let mut backoff_secs: u64 = 1;

        loop {
            // 1. Obtain a fresh WSS URL
            let wss_url = match self.open_socket_url().await {
                Ok(url) => url,
                Err(e) => {
                    warn!(
                        "Failed to open Slack socket URL: {}. Retrying in {}s",
                        e, backoff_secs
                    );
                    tokio::time::sleep(tokio::time::Duration::from_secs(backoff_secs)).await;
                    backoff_secs = (backoff_secs * 2).min(60);
                    continue;
                }
            };

            // 2. Connect WebSocket
            let ws_result = connect_async(&wss_url).await;
            let (mut ws_stream, _) = match ws_result {
                Ok(pair) => pair,
                Err(e) => {
                    warn!(
                        "Slack WebSocket connect error: {}. Retrying in {}s",
                        e, backoff_secs
                    );
                    tokio::time::sleep(tokio::time::Duration::from_secs(backoff_secs)).await;
                    backoff_secs = (backoff_secs * 2).min(60);
                    continue;
                }
            };

            // 3. Read loop — reset backoff on successful connection
            backoff_secs = 1;
            loop {
                match ws_stream.next().await {
                    None => {
                        warn!(
                            "Slack WebSocket stream ended. Reconnecting in {}s",
                            backoff_secs
                        );
                        break;
                    }
                    Some(Err(e)) => {
                        warn!(
                            "Slack WebSocket read error: {}. Reconnecting in {}s",
                            e, backoff_secs
                        );
                        break;
                    }
                    Some(Ok(msg)) => {
                        let text = match msg {
                            Message::Text(t) => t,
                            Message::Ping(data) => {
                                let _ = ws_stream.send(Message::Pong(data)).await;
                                continue;
                            }
                            Message::Close(_) => {
                                info!("Slack WebSocket closed by server. Reconnecting.");
                                break;
                            }
                            _ => continue,
                        };

                        let frame: SlackSocketFrame = match serde_json::from_str(&text) {
                            Ok(f) => f,
                            Err(e) => {
                                error!("Failed to parse Slack socket frame: {}", e);
                                continue;
                            }
                        };

                        match frame.frame_type.as_str() {
                            "hello" => {
                                info!("Slack Socket Mode connected (hello received)");
                            }
                            "disconnect" => {
                                info!("Slack requested disconnect. Reconnecting.");
                                break;
                            }
                            "events_api" => {
                                // ACK first to prevent Slack retries (3s window)
                                if let Some(ref envelope_id) = frame.envelope_id {
                                    let ack = json!({
                                        "envelope_id": envelope_id,
                                        "payload": ""
                                    });
                                    if let Err(e) =
                                        ws_stream.send(Message::Text(ack.to_string().into())).await
                                    {
                                        warn!(
                                            "Failed to ACK Slack envelope {}: {}",
                                            envelope_id, e
                                        );
                                    }
                                }

                                // Extract and emit the message event
                                if let Some(payload) = frame.payload {
                                    if let Some(msg_event) = payload.event {
                                        let event_type =
                                            msg_event.event_type.as_deref().unwrap_or("");
                                        if event_type == "message" {
                                            let channel_id =
                                                msg_event.channel.clone().unwrap_or_default();
                                            let text_val =
                                                msg_event.text.clone().unwrap_or_default();
                                            let user = msg_event
                                                .user
                                                .clone()
                                                .unwrap_or_else(|| "unknown".to_string());

                                            if text_val.is_empty() {
                                                continue;
                                            }

                                            let event = Event {
                                                event_type: EventType::WebChat,
                                                payload: json!({
                                                    "session_id": channel_id,
                                                    "text": text_val,
                                                    "from_name": user,
                                                }),
                                            };

                                            if event_tx.send(event).await.is_err() {
                                                info!(
                                                    "Event receiver dropped, stopping Slack channel"
                                                );
                                                return Ok(());
                                            }
                                        }
                                    }
                                }
                            }
                            other => {
                                info!("Slack socket: unhandled frame type '{}'", other);
                            }
                        }
                    }
                }
            }

            tokio::time::sleep(tokio::time::Duration::from_secs(backoff_secs)).await;
            backoff_secs = (backoff_secs * 2).min(60);
        }
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
            .map_err(|e| KittypawError::Llm {
                kind: kittypaw_core::error::LlmErrorKind::Other,
                message: format!("Slack postMessage failed: {}", e),
            })?;

        let slack_resp: serde_json::Value = resp.json().await.map_err(|e| KittypawError::Llm {
            kind: kittypaw_core::error::LlmErrorKind::Other,
            message: format!("Failed to parse Slack response: {}", e),
        })?;

        if !slack_resp
            .get("ok")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            let err = slack_resp
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error");
            return Err(KittypawError::Llm {
                kind: kittypaw_core::error::LlmErrorKind::Other,
                message: format!("Slack postMessage error: {}", err),
            });
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
    use serde_json::json;

    #[test]
    fn test_channel_name() {
        let ch = SlackChannel::new("xoxb-dummy-token", "xapp-dummy-token");
        assert_eq!(ch.name(), "slack");
    }

    #[test]
    fn test_parse_hello_frame() {
        let raw = json!({ "type": "hello", "num_connections": 1 });
        let frame: SlackSocketFrame = serde_json::from_value(raw).unwrap();
        assert_eq!(frame.frame_type, "hello");
        assert!(frame.envelope_id.is_none());
        assert!(frame.payload.is_none());
    }

    #[test]
    fn test_parse_events_api_message_frame() {
        let raw = json!({
            "type": "events_api",
            "envelope_id": "abc-123",
            "payload": {
                "event": {
                    "type": "message",
                    "channel": "C123456",
                    "text": "Hello from Slack",
                    "user": "U999"
                }
            }
        });

        let frame: SlackSocketFrame = serde_json::from_value(raw).unwrap();
        assert_eq!(frame.frame_type, "events_api");
        assert_eq!(frame.envelope_id.as_deref(), Some("abc-123"));

        let payload = frame.payload.unwrap();
        let event = payload.event.unwrap();
        assert_eq!(event.event_type.as_deref(), Some("message"));
        assert_eq!(event.channel.as_deref(), Some("C123456"));
        assert_eq!(event.text.as_deref(), Some("Hello from Slack"));
        assert_eq!(event.user.as_deref(), Some("U999"));
    }

    #[test]
    fn test_parse_events_api_missing_fields() {
        let raw = json!({
            "type": "events_api",
            "envelope_id": "xyz-456",
            "payload": {}
        });
        let frame: SlackSocketFrame = serde_json::from_value(raw).unwrap();
        assert_eq!(frame.envelope_id.as_deref(), Some("xyz-456"));
        let payload = frame.payload.unwrap();
        assert!(payload.event.is_none());
    }

    #[test]
    fn test_parse_disconnect_frame() {
        let raw = json!({ "type": "disconnect", "reason": "refresh_requested" });
        let frame: SlackSocketFrame = serde_json::from_value(raw).unwrap();
        assert_eq!(frame.frame_type, "disconnect");
    }

    #[test]
    fn test_event_payload_structure() {
        let event = Event {
            event_type: EventType::WebChat,
            payload: json!({
                "session_id": "C123456",
                "text": "Hello from Slack",
                "from_name": "U999",
            }),
        };
        assert_eq!(event.event_type, EventType::WebChat);
        assert_eq!(event.payload["session_id"], "C123456");
        assert_eq!(event.payload["text"], "Hello from Slack");
        assert_eq!(event.payload["from_name"], "U999");
    }
}
