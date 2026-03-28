use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use kittypaw_core::error::Result;
use kittypaw_core::types::{ExecutionResult, SkillCall};

/// Configuration for sandbox execution
pub struct SandboxExecConfig {
    pub code: String,
    pub context_json: String,
    pub timeout_ms: u64,
    pub max_memory_bytes: usize,
}

/// Callback that executes a skill call and returns the JSON-serialized result.
/// When provided, sandbox skill stubs call this to get real data (Http responses,
/// Storage values, etc.) instead of returning "null".
pub type SkillResolver =
    Arc<dyn Fn(SkillCall) -> Pin<Box<dyn Future<Output = String> + Send>> + Send + Sync>;

#[async_trait]
pub trait SandboxBackend: Send + Sync {
    async fn execute(
        &self,
        config: SandboxExecConfig,
        skill_resolver: Option<SkillResolver>,
    ) -> Result<ExecutionResult>;
}
