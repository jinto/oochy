use async_trait::async_trait;
use kittypaw_core::error::Result;
use kittypaw_core::types::ExecutionResult;

use crate::backend::{SandboxBackend, SandboxExecConfig, SkillResolver};
use crate::quickjs::run_child_async;

/// Thread-based sandbox for all platforms (no fork, no OS isolation).
///
/// # Security Note
/// ThreadSandbox provides only QuickJS VM-level isolation.
/// There is no OS-level sandboxing. Malicious JS could potentially
/// access process memory via QuickJS bugs. Use ForkedSandbox on
/// unix platforms for production workloads with untrusted code.
pub struct ThreadSandbox {
    pub timeout_secs: u64,
    pub memory_limit_mb: u64,
}

impl ThreadSandbox {
    pub fn new(timeout_secs: u64, memory_limit_mb: u64) -> Self {
        Self {
            timeout_secs,
            memory_limit_mb,
        }
    }
}

#[async_trait]
impl SandboxBackend for ThreadSandbox {
    async fn execute(
        &self,
        config: SandboxExecConfig,
        skill_resolver: Option<SkillResolver>,
    ) -> Result<ExecutionResult> {
        let code = config.code.clone();
        let context: serde_json::Value = serde_json::from_str(&config.context_json)
            .unwrap_or(serde_json::Value::Object(Default::default()));
        let timeout_secs = config.timeout_ms / 1000;
        let js_timeout = std::time::Duration::from_secs(timeout_secs);
        // Outer tokio timeout is a backstop; the interrupt handler fires first
        let outer_timeout = std::time::Duration::from_secs(timeout_secs + 5);

        let result = tokio::time::timeout(
            outer_timeout,
            tokio::task::spawn_blocking(move || {
                run_child_async(&code, context, Some(js_timeout), skill_resolver)
            }),
        )
        .await;

        match result {
            Ok(Ok(exec_result)) => Ok(exec_result),
            Ok(Err(_)) => Ok(ExecutionResult {
                success: false,
                output: String::new(),
                skill_calls: vec![],
                error: Some("thread panicked".into()),
            }),
            Err(_) => Ok(ExecutionResult {
                success: false,
                output: String::new(),
                skill_calls: vec![],
                error: Some("execution timed out".into()),
            }),
        }
    }
}
