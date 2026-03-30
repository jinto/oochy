use thiserror::Error;

#[derive(Error, Debug)]
pub enum KittypawError {
    #[error("LLM error: {0}")]
    Llm(String),

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
