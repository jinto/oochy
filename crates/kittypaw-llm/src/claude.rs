use async_trait::async_trait;
use futures_util::StreamExt;
use kittypaw_core::error::{KittypawError, LlmErrorKind, Result};
use kittypaw_core::types::LlmMessage;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::provider::LlmProvider;
use crate::util::strip_code_fences;

pub struct ClaudeProvider {
    api_key: String,
    model: String,
    max_tokens: u32,
    client: reqwest::Client,
}

impl ClaudeProvider {
    pub fn new(api_key: String, model: String, max_tokens: u32) -> Self {
        Self {
            api_key,
            model,
            max_tokens,
            client: reqwest::Client::new(),
        }
    }
}

#[derive(Serialize)]
struct ClaudeRequest {
    model: String,
    max_tokens: u32,
    system: String,
    messages: Vec<ClaudeMessage>,
}

#[derive(Serialize)]
struct ClaudeStreamRequest {
    model: String,
    max_tokens: u32,
    system: String,
    messages: Vec<ClaudeMessage>,
    stream: bool,
}

#[derive(Deserialize, Debug)]
struct SseDelta {
    #[serde(rename = "type")]
    delta_type: String,
    text: Option<String>,
}

#[derive(Deserialize, Debug)]
struct SseData {
    #[serde(rename = "type")]
    event_type: String,
    delta: Option<SseDelta>,
}

#[derive(Serialize)]
struct ClaudeMessage {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct ClaudeResponse {
    content: Vec<ContentBlock>,
}

#[derive(Deserialize)]
struct ContentBlock {
    text: Option<String>,
}

#[async_trait]
impl LlmProvider for ClaudeProvider {
    async fn generate(&self, messages: &[LlmMessage]) -> Result<String> {
        let system = messages
            .iter()
            .find(|m| m.role == kittypaw_core::types::Role::System)
            .map(|m| m.content.clone())
            .unwrap_or_default();

        let api_messages: Vec<ClaudeMessage> = messages
            .iter()
            .filter(|m| m.role != kittypaw_core::types::Role::System)
            .map(|m| ClaudeMessage {
                role: match m.role {
                    kittypaw_core::types::Role::User => "user".into(),
                    kittypaw_core::types::Role::Assistant => "assistant".into(),
                    kittypaw_core::types::Role::System => unreachable!(),
                },
                content: m.content.clone(),
            })
            .collect();

        let request = ClaudeRequest {
            model: self.model.clone(),
            max_tokens: self.max_tokens,
            system,
            messages: api_messages,
        };

        let mut retries = 0;
        let max_retries = 3;

        loop {
            let response = self
                .client
                .post("https://api.anthropic.com/v1/messages")
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", "2023-06-01")
                .header("content-type", "application/json")
                .json(&request)
                .send()
                .await
                .map_err(|e| KittypawError::Llm {
                    kind: LlmErrorKind::Other,
                    message: format!("HTTP error: {e}"),
                })?;

            let status = response.status();

            if status == 429 {
                retries += 1;
                if retries > max_retries {
                    return Err(KittypawError::Llm {
                        kind: LlmErrorKind::RateLimit,
                        message: "Rate limited after max retries".into(),
                    });
                }
                let delay = std::time::Duration::from_millis(1000 * 2u64.pow(retries));
                tracing::warn!("Rate limited, retrying in {:?}", delay);
                tokio::time::sleep(delay).await;
                continue;
            }

            if status.is_server_error() {
                retries += 1;
                if retries > max_retries {
                    return Err(KittypawError::Llm {
                        kind: LlmErrorKind::Other,
                        message: format!("Server error {status} after max retries"),
                    });
                }
                let delay = std::time::Duration::from_millis(1000 * 2u64.pow(retries));
                tracing::warn!("Server error {status}, retrying in {:?}", delay);
                tokio::time::sleep(delay).await;
                continue;
            }

            if !status.is_success() {
                let body = response.text().await.unwrap_or_default();
                let kind = if status == 400
                    && (body.contains("context_length_exceeded")
                        || body.contains("context_window")
                        || body.contains("too many tokens")
                        || body.contains("max_tokens"))
                {
                    LlmErrorKind::TokenLimit
                } else {
                    LlmErrorKind::Other
                };
                return Err(KittypawError::Llm {
                    kind,
                    message: format!("API error {status}: {body}"),
                });
            }

            let body: ClaudeResponse = response.json().await.map_err(|e| KittypawError::Llm {
                kind: LlmErrorKind::Other,
                message: format!("Response parse error: {e}"),
            })?;

            let text = body
                .content
                .into_iter()
                .filter_map(|b| b.text)
                .collect::<Vec<_>>()
                .join("");

            return Ok(strip_code_fences(&text));
        }
    }

    async fn generate_stream(
        &self,
        messages: &[LlmMessage],
        on_token: Arc<dyn Fn(String) + Send + Sync>,
    ) -> Result<String> {
        let system = messages
            .iter()
            .find(|m| m.role == kittypaw_core::types::Role::System)
            .map(|m| m.content.clone())
            .unwrap_or_default();

        let api_messages: Vec<ClaudeMessage> = messages
            .iter()
            .filter(|m| m.role != kittypaw_core::types::Role::System)
            .map(|m| ClaudeMessage {
                role: match m.role {
                    kittypaw_core::types::Role::User => "user".into(),
                    kittypaw_core::types::Role::Assistant => "assistant".into(),
                    kittypaw_core::types::Role::System => unreachable!(),
                },
                content: m.content.clone(),
            })
            .collect();

        let request = ClaudeStreamRequest {
            model: self.model.clone(),
            max_tokens: self.max_tokens,
            system,
            messages: api_messages,
            stream: true,
        };

        let response = self
            .client
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&request)
            .send()
            .await
            .map_err(|e| KittypawError::Llm {
                kind: LlmErrorKind::Other,
                message: format!("HTTP error: {e}"),
            })?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            let kind = if status == 429 {
                LlmErrorKind::RateLimit
            } else if status == 400
                && (body.contains("context_length_exceeded")
                    || body.contains("context_window")
                    || body.contains("too many tokens")
                    || body.contains("max_tokens"))
            {
                LlmErrorKind::TokenLimit
            } else {
                LlmErrorKind::Other
            };
            return Err(KittypawError::Llm {
                kind,
                message: format!("API error {status}: {body}"),
            });
        }

        let mut accumulated = String::new();
        let mut byte_stream = response.bytes_stream();

        // SSE line buffer
        let mut line_buf = String::new();

        while let Some(chunk) = byte_stream.next().await {
            let chunk = chunk.map_err(|e| KittypawError::Llm {
                kind: LlmErrorKind::Other,
                message: format!("Stream error: {e}"),
            })?;
            let text = std::str::from_utf8(&chunk).map_err(|e| KittypawError::Llm {
                kind: LlmErrorKind::Other,
                message: format!("UTF-8 decode error: {e}"),
            })?;

            line_buf.push_str(text);

            // Process complete lines
            while let Some(newline_pos) = line_buf.find('\n') {
                let line = line_buf[..newline_pos].trim_end_matches('\r').to_string();
                line_buf = line_buf[newline_pos + 1..].to_string();

                if let Some(data) = line.strip_prefix("data: ") {
                    if data == "[DONE]" {
                        break;
                    }
                    if let Ok(sse) = serde_json::from_str::<SseData>(data) {
                        if sse.event_type == "content_block_delta" {
                            if let Some(delta) = sse.delta {
                                if delta.delta_type == "text_delta" {
                                    if let Some(text) = delta.text {
                                        accumulated.push_str(&text);
                                        on_token(text);
                                    }
                                }
                            }
                        } else if sse.event_type == "message_stop" {
                            break;
                        }
                    }
                }
            }
        }

        Ok(strip_code_fences(&accumulated))
    }
}
