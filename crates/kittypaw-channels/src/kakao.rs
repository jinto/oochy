use async_trait::async_trait;
use dashmap::DashMap;
use kittypaw_core::error::{KittypawError, Result};
use kittypaw_core::types::{Event, EventType};
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::channel::Channel;

/// A message received from KakaoTalk via the CF Worker relay.
#[derive(Debug, Clone)]
pub struct KakaoMessage {
    pub text: String,
    pub user_id: String,
    pub callback_url: String,
    pub action_id: String,
}

/// Abstraction over the relay HTTP calls — swappable for tests.
#[async_trait]
pub trait RelayClient: Send + Sync {
    /// Poll for a pending message for `user_token`. Returns `None` if the relay has nothing (204).
    async fn poll(&self, user_token: &str) -> Result<Option<KakaoMessage>>;

    /// POST the Kakao-formatted response to `callback_url`.
    async fn send_callback(&self, callback_url: &str, response: &str) -> Result<()>;
}

/// Production relay client that calls the actual CF Worker.
pub struct HttpRelayClient {
    relay_url: String,
    client: Client,
}

impl HttpRelayClient {
    pub fn new(relay_url: impl Into<String>) -> Self {
        Self {
            relay_url: relay_url.into(),
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .unwrap_or_default(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct PollResponse {
    text: String,
    user_id: String,
    callback_url: String,
    action_id: String,
}

#[async_trait]
impl RelayClient for HttpRelayClient {
    async fn poll(&self, user_token: &str) -> Result<Option<KakaoMessage>> {
        let url = format!(
            "{}/poll/{}",
            self.relay_url.trim_end_matches('/'),
            user_token
        );
        let resp = self
            .client
            .post(&url)
            .send()
            .await
            .map_err(|e| KittypawError::Skill(format!("Relay poll error: {e}")))?;

        if resp.status() == reqwest::StatusCode::NO_CONTENT {
            return Ok(None);
        }

        if !resp.status().is_success() {
            return Err(KittypawError::Skill(format!(
                "Relay poll returned {}",
                resp.status()
            )));
        }

        let msg: PollResponse = resp
            .json()
            .await
            .map_err(|e| KittypawError::Skill(format!("Relay poll parse error: {e}")))?;

        Ok(Some(KakaoMessage {
            text: msg.text,
            user_id: msg.user_id,
            callback_url: msg.callback_url,
            action_id: msg.action_id,
        }))
    }

    async fn send_callback(&self, callback_url: &str, response: &str) -> Result<()> {
        self.client
            .post(callback_url)
            .json(&json!({
                "version": "2.0",
                "template": {
                    "outputs": [{"simpleText": {"text": response}}]
                }
            }))
            .send()
            .await
            .map_err(|e| KittypawError::Skill(format!("Kakao callback error: {e}")))?;
        Ok(())
    }
}

/// KakaoTalk channel via CF Worker relay.
///
/// Polls the relay every 3 seconds (with exponential backoff on errors).
/// Stores pending `callback_url`s by `user_id` so `send_response` can reach back.
pub struct KakaoChannel {
    user_token: String,
    client: Arc<dyn RelayClient>,
    /// user_id → callback_url for the most recent pending request
    pending_callbacks: Arc<DashMap<String, String>>,
}

impl KakaoChannel {
    pub fn new(relay_url: impl Into<String>, user_token: impl Into<String>) -> Self {
        Self::new_with_client(Arc::new(HttpRelayClient::new(relay_url)), user_token.into())
    }

    pub fn new_with_client(client: Arc<dyn RelayClient>, user_token: impl Into<String>) -> Self {
        Self {
            user_token: user_token.into(),
            client,
            pending_callbacks: Arc::new(DashMap::new()),
        }
    }

    /// Parse a raw Open Builder JSON payload into a `KakaoMessage`.
    pub fn parse_payload(payload: &serde_json::Value) -> Result<KakaoMessage> {
        let text = payload
            .get("userRequest")
            .and_then(|r| r.get("utterance"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| KittypawError::Skill("Missing userRequest.utterance".into()))?
            .to_string();

        let user_id = payload
            .get("userRequest")
            .and_then(|r| r.get("user"))
            .and_then(|u| u.get("id"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| KittypawError::Skill("Missing userRequest.user.id".into()))?
            .to_string();

        let callback_url = payload
            .get("callbackUrl")
            .and_then(|v| v.as_str())
            .ok_or_else(|| KittypawError::Skill("Missing callbackUrl".into()))?
            .to_string();

        let action_id = payload
            .get("action")
            .and_then(|a| a.get("id"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| KittypawError::Skill("Missing action.id".into()))?
            .to_string();

        Ok(KakaoMessage {
            text,
            user_id,
            callback_url,
            action_id,
        })
    }
}

#[async_trait]
impl Channel for KakaoChannel {
    async fn start(&self, event_tx: mpsc::Sender<Event>) -> Result<()> {
        info!("Starting KakaoTalk channel polling (relay)");
        let mut backoff_secs: u64 = 1;

        loop {
            match self.client.poll(&self.user_token).await {
                Err(e) => {
                    warn!(
                        "KakaoTalk relay poll error: {}. Retrying in {}s",
                        e, backoff_secs
                    );
                    tokio::time::sleep(tokio::time::Duration::from_secs(backoff_secs)).await;
                    backoff_secs = (backoff_secs * 2).min(60);
                }
                Ok(None) => {
                    // No message yet — reset backoff and wait 3 seconds
                    backoff_secs = 1;
                    tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
                }
                Ok(Some(msg)) => {
                    backoff_secs = 1;
                    info!(
                        user_id = %msg.user_id,
                        action_id = %msg.action_id,
                        "KakaoTalk: received message"
                    );

                    // Store callback_url so send_response can reach back
                    self.pending_callbacks
                        .insert(msg.user_id.clone(), msg.callback_url.clone());

                    let event = Event {
                        event_type: EventType::KakaoTalk,
                        payload: json!({
                            "user_id": msg.user_id,
                            "text": msg.text,
                            "callback_url": msg.callback_url,
                            "action_id": msg.action_id,
                        }),
                    };

                    if event_tx.send(event).await.is_err() {
                        info!("Event receiver dropped, stopping KakaoTalk polling");
                        return Ok(());
                    }

                    // Brief pause after receiving a message before next poll
                    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                }
            }
        }
    }

    async fn send_response(&self, user_id: &str, response: &str) -> Result<()> {
        if let Some((_, callback_url)) = self.pending_callbacks.remove(user_id) {
            self.client.send_callback(&callback_url, response).await
        } else {
            warn!("KakaoTalk: no pending callback_url for user_id={user_id}");
            Ok(())
        }
    }

    fn name(&self) -> &str {
        "kakao"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct MockRelayClient {
        messages: Mutex<Vec<KakaoMessage>>,
        sent_callbacks: Mutex<Vec<(String, String)>>,
    }

    impl MockRelayClient {
        fn new(messages: Vec<KakaoMessage>) -> Self {
            Self {
                messages: Mutex::new(messages),
                sent_callbacks: Mutex::new(vec![]),
            }
        }
    }

    #[async_trait]
    impl RelayClient for MockRelayClient {
        async fn poll(&self, _user_token: &str) -> Result<Option<KakaoMessage>> {
            let mut msgs = self.messages.lock().unwrap();
            if msgs.is_empty() {
                Ok(None)
            } else {
                Ok(Some(msgs.remove(0)))
            }
        }

        async fn send_callback(&self, callback_url: &str, response: &str) -> Result<()> {
            self.sent_callbacks
                .lock()
                .unwrap()
                .push((callback_url.to_string(), response.to_string()));
            Ok(())
        }
    }

    #[test]
    fn kakao_channel_name_is_kakao() {
        let ch =
            KakaoChannel::new_with_client(Arc::new(MockRelayClient::new(vec![])), "test_token");
        assert_eq!(ch.name(), "kakao");
    }

    #[test]
    fn kakao_parses_openbuilder_payload() {
        let payload = json!({
            "action": {"id": "act1"},
            "userRequest": {
                "utterance": "hello",
                "user": {"id": "u1"}
            },
            "callbackUrl": "https://cb.kakao.com/123"
        });
        let parsed = KakaoChannel::parse_payload(&payload).unwrap();
        assert_eq!(parsed.text, "hello");
        assert_eq!(parsed.user_id, "u1");
        assert_eq!(parsed.callback_url, "https://cb.kakao.com/123");
        assert_eq!(parsed.action_id, "act1");
    }

    #[test]
    fn kakao_parse_fails_on_missing_utterance() {
        let payload = json!({
            "action": {"id": "act1"},
            "userRequest": {"user": {"id": "u1"}},
            "callbackUrl": "https://cb.kakao.com/123"
        });
        assert!(KakaoChannel::parse_payload(&payload).is_err());
    }

    #[test]
    fn kakao_parse_fails_on_missing_callback_url() {
        let payload = json!({
            "action": {"id": "act1"},
            "userRequest": {
                "utterance": "hi",
                "user": {"id": "u1"}
            }
        });
        assert!(KakaoChannel::parse_payload(&payload).is_err());
    }
}
