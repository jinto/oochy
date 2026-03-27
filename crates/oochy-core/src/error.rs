use thiserror::Error;

#[derive(Error, Debug)]
pub enum OochyError {
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
}

pub type Result<T> = std::result::Result<T, OochyError>;
