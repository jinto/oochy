use kittypaw_core::error::{KittypawError, Result};
use serde::Deserialize;

const WHISPER_URL: &str = "https://api.openai.com/v1/audio/transcriptions";

pub struct WhisperClient {
    api_key: String,
    language: Option<String>,
    client: reqwest::Client,
}

impl WhisperClient {
    pub fn new(api_key: &str) -> Self {
        Self {
            api_key: api_key.to_string(),
            language: None,
            client: reqwest::Client::new(),
        }
    }

    pub fn with_language(api_key: &str, language: &str) -> Self {
        Self {
            api_key: api_key.to_string(),
            language: Some(language.to_string()),
            client: reqwest::Client::new(),
        }
    }

    pub async fn transcribe(&self, audio_data: &[u8], format: &str) -> Result<String> {
        let filename = format!("audio.{format}");
        let mime = format!("audio/{format}");

        let file_part = reqwest::multipart::Part::bytes(audio_data.to_vec())
            .file_name(filename)
            .mime_str(&mime)
            .map_err(|e| KittypawError::Llm {
                kind: kittypaw_core::error::LlmErrorKind::Other,
                message: format!("MIME type error: {e}"),
            })?;

        let mut form = reqwest::multipart::Form::new()
            .part("file", file_part)
            .text("model", "whisper-1");

        if let Some(ref lang) = self.language {
            form = form.text("language", lang.clone());
        }

        let response = self
            .client
            .post(WHISPER_URL)
            .header("authorization", format!("Bearer {}", self.api_key))
            .multipart(form)
            .send()
            .await
            .map_err(|e| KittypawError::Llm {
                kind: kittypaw_core::error::LlmErrorKind::Other,
                message: format!("HTTP error: {e}"),
            })?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(KittypawError::Llm {
                kind: kittypaw_core::error::LlmErrorKind::Other,
                message: format!("Whisper API error {status}: {body}"),
            });
        }

        let body: WhisperResponse = response.json().await.map_err(|e| KittypawError::Llm {
            kind: kittypaw_core::error::LlmErrorKind::Other,
            message: format!("Response parse error: {e}"),
        })?;

        Ok(body.text)
    }
}

#[derive(Deserialize)]
struct WhisperResponse {
    text: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_stores_api_key() {
        let client = WhisperClient::new("test-key");
        assert_eq!(client.api_key, "test-key");
        assert!(client.language.is_none());
    }

    #[test]
    fn test_with_language_stores_language() {
        let client = WhisperClient::with_language("test-key", "ko");
        assert_eq!(client.api_key, "test-key");
        assert_eq!(client.language.as_deref(), Some("ko"));
    }

    #[test]
    fn test_response_parsing() {
        let json = r#"{"text": "안녕하세요"}"#;
        let parsed: WhisperResponse = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.text, "안녕하세요");
    }
}
