use async_trait::async_trait;
use futures_util::StreamExt;
use kittypaw_core::error::{KittypawError, LlmErrorKind, Result};
use kittypaw_core::types::LlmMessage;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::provider::LlmProvider;
use crate::util::strip_code_fences;

const DEFAULT_OPENAI_BASE_URL: &str = "https://api.openai.com/v1";

pub struct OpenAiProvider {
    base_url: String,
    api_key: String,
    model: String,
    max_tokens: u32,
    client: reqwest::Client,
}

impl OpenAiProvider {
    pub fn new(api_key: String, model: String, max_tokens: u32) -> Self {
        Self::with_base_url(
            DEFAULT_OPENAI_BASE_URL.to_string(),
            api_key,
            model,
            max_tokens,
        )
    }

    pub fn with_base_url(
        base_url: String,
        api_key: String,
        model: String,
        max_tokens: u32,
    ) -> Self {
        Self {
            base_url,
            api_key,
            model,
            max_tokens,
            client: reqwest::Client::new(),
        }
    }
}

#[derive(Serialize)]
struct OpenAiRequest {
    model: String,
    max_tokens: u32,
    messages: Vec<OpenAiMessage>,
}

#[derive(Serialize)]
struct OpenAiStreamRequest {
    model: String,
    max_tokens: u32,
    messages: Vec<OpenAiMessage>,
    stream: bool,
}

#[derive(Serialize, Clone)]
struct OpenAiMessage {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct OpenAiResponse {
    choices: Vec<OpenAiChoice>,
}

#[derive(Deserialize)]
struct OpenAiChoice {
    message: OpenAiMessageContent,
}

#[derive(Deserialize)]
struct OpenAiMessageContent {
    content: Option<String>,
}

#[derive(Deserialize)]
struct OpenAiStreamChunk {
    choices: Vec<OpenAiStreamChoice>,
}

#[derive(Deserialize)]
struct OpenAiStreamChoice {
    delta: OpenAiDelta,
}

#[derive(Deserialize)]
struct OpenAiDelta {
    content: Option<String>,
}

fn map_role(role: &kittypaw_core::types::Role) -> &'static str {
    match role {
        kittypaw_core::types::Role::User => "user",
        kittypaw_core::types::Role::Assistant => "assistant",
        kittypaw_core::types::Role::System => "system",
    }
}

fn build_messages(messages: &[LlmMessage]) -> Vec<OpenAiMessage> {
    messages
        .iter()
        .map(|m| OpenAiMessage {
            role: map_role(&m.role).into(),
            content: m.content.clone(),
        })
        .collect()
}

#[async_trait]
impl LlmProvider for OpenAiProvider {
    async fn generate(&self, messages: &[LlmMessage]) -> Result<String> {
        let api_messages = build_messages(messages);

        let request = OpenAiRequest {
            model: self.model.clone(),
            max_tokens: self.max_tokens,
            messages: api_messages,
        };

        let mut retries = 0;
        let max_retries = 3;

        let url = format!("{}/chat/completions", self.base_url);

        loop {
            let mut req = self
                .client
                .post(&url)
                .header("content-type", "application/json");
            if !self.api_key.is_empty() {
                req = req.header("authorization", format!("Bearer {}", self.api_key));
            }
            let response = req
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
                let kind = LlmErrorKind::from_http_response(status.as_u16(), &body);
                return Err(KittypawError::Llm {
                    kind,
                    message: format!("API error {status}: {body}"),
                });
            }

            let body: OpenAiResponse = response.json().await.map_err(|e| KittypawError::Llm {
                kind: LlmErrorKind::Other,
                message: format!("Response parse error: {e}"),
            })?;

            let text = body
                .choices
                .into_iter()
                .next()
                .and_then(|c| c.message.content)
                .unwrap_or_default();

            return Ok(strip_code_fences(&text));
        }
    }

    async fn generate_stream(
        &self,
        messages: &[LlmMessage],
        on_token: Arc<dyn Fn(String) + Send + Sync>,
    ) -> Result<String> {
        let api_messages = build_messages(messages);

        let request = OpenAiStreamRequest {
            model: self.model.clone(),
            max_tokens: self.max_tokens,
            messages: api_messages,
            stream: true,
        };

        let url = format!("{}/chat/completions", self.base_url);
        let mut req = self
            .client
            .post(&url)
            .header("content-type", "application/json");
        if !self.api_key.is_empty() {
            req = req.header("authorization", format!("Bearer {}", self.api_key));
        }
        let response = req
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
            let kind = LlmErrorKind::from_http_response(status.as_u16(), &body);
            return Err(KittypawError::Llm {
                kind,
                message: format!("API error {status}: {body}"),
            });
        }

        let mut accumulated = String::new();
        let mut byte_stream = response.bytes_stream();
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

            while let Some(newline_pos) = line_buf.find('\n') {
                let line = line_buf[..newline_pos].trim_end_matches('\r').to_string();
                line_buf = line_buf[newline_pos + 1..].to_string();

                if let Some(data) = line.strip_prefix("data: ") {
                    if data == "[DONE]" {
                        break;
                    }
                    if let Ok(chunk) = serde_json::from_str::<OpenAiStreamChunk>(data) {
                        if let Some(choice) = chunk.choices.into_iter().next() {
                            if let Some(content) = choice.delta.content {
                                accumulated.push_str(&content);
                                on_token(content);
                            }
                        }
                    }
                }
            }
        }

        Ok(strip_code_fences(&accumulated))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_role_mapping() {
        assert_eq!(map_role(&kittypaw_core::types::Role::User), "user");
        assert_eq!(
            map_role(&kittypaw_core::types::Role::Assistant),
            "assistant"
        );
        assert_eq!(map_role(&kittypaw_core::types::Role::System), "system");
    }

    #[test]
    fn test_build_messages_preserves_system() {
        let messages = vec![
            LlmMessage {
                role: kittypaw_core::types::Role::System,
                content: "You are helpful.".into(),
            },
            LlmMessage {
                role: kittypaw_core::types::Role::User,
                content: "Hello".into(),
            },
        ];

        let result = build_messages(&messages);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].role, "system");
        assert_eq!(result[0].content, "You are helpful.");
        assert_eq!(result[1].role, "user");
        assert_eq!(result[1].content, "Hello");
    }

    #[test]
    fn test_request_body_structure() {
        let messages = vec![OpenAiMessage {
            role: "user".into(),
            content: "Hello".into(),
        }];

        let request = OpenAiRequest {
            model: "gpt-4o".into(),
            max_tokens: 1024,
            messages,
        };

        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["model"], "gpt-4o");
        assert_eq!(json["max_tokens"], 1024);
        assert_eq!(json["messages"][0]["role"], "user");
        assert_eq!(json["messages"][0]["content"], "Hello");
    }

    #[test]
    fn test_stream_request_has_stream_field() {
        let request = OpenAiStreamRequest {
            model: "gpt-4o".into(),
            max_tokens: 1024,
            messages: vec![],
            stream: true,
        };

        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["stream"], true);
    }

    #[test]
    fn test_new_uses_default_base_url() {
        let _provider = OpenAiProvider::new("test-key".into(), "gpt-4o".into(), 4096);
        // Compiles and doesn't panic = default base URL works
    }

    #[test]
    fn test_with_base_url_accepts_empty_key() {
        let _provider = OpenAiProvider::with_base_url(
            "http://localhost:11434/v1".into(),
            String::new(),
            "qwen3.5:27b".into(),
            4096,
        );
        // Empty API key should not panic
    }
}
