use thiserror::Error;

/// Classifies the failure mode of an LLM API call so callers can apply
/// appropriate retry or fallback strategies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LlmErrorKind {
    /// HTTP 429 or provider-level rate limit response.
    RateLimit,
    /// HTTP 400 with a context/token length error in the response body.
    TokenLimit,
    /// Any other LLM error.
    Other,
}

#[derive(Error, Debug)]
pub enum KittypawError {
    #[error("LLM error ({kind:?}): {message}")]
    Llm { kind: LlmErrorKind, message: String },

    #[error("Sandbox error: {0}")]
    Sandbox(String),

    #[error("Store error: {0}")]
    Store(String),

    #[error("Skill error: {0}")]
    Skill(String),

    #[error("Config error: {0}")]
    Config(String),

    #[error("Capability denied: {0}")]
    CapabilityDenied(String),

    #[error("Rate limit exceeded: {0}")]
    RateLimitExceeded(String),

    #[error("Timeout after {0}s")]
    Timeout(u64),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Json(#[from] serde_json::Error),

    #[cfg(feature = "registry")]
    #[error("Network error: {0}")]
    Network(String),
}

pub type Result<T> = std::result::Result<T, KittypawError>;

impl KittypawError {
    /// Returns `true` if this is an LLM rate-limit error (HTTP 429).
    pub fn is_rate_limit(&self) -> bool {
        matches!(
            self,
            KittypawError::Llm {
                kind: LlmErrorKind::RateLimit,
                ..
            }
        )
    }

    /// Returns `true` if this is an LLM token/context-length limit error.
    pub fn is_token_limit(&self) -> bool {
        matches!(
            self,
            KittypawError::Llm {
                kind: LlmErrorKind::TokenLimit,
                ..
            }
        )
    }
}

#[cfg(feature = "registry")]
impl From<reqwest::Error> for KittypawError {
    fn from(e: reqwest::Error) -> Self {
        KittypawError::Network(e.to_string())
    }
}

#[cfg(feature = "rusqlite")]
impl From<rusqlite::Error> for KittypawError {
    fn from(e: rusqlite::Error) -> Self {
        KittypawError::Store(e.to_string())
    }
}
