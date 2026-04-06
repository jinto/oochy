use kittypaw_core::error::{KittypawError, Result};
use kittypaw_core::types::SkillCall;

pub(super) async fn execute_git(call: &SkillCall) -> Result<serde_json::Value> {
    match call.method.as_str() {
        "status" => run_git(&["status", "--porcelain"]).await,
        "diff" => {
            let pathspec = call.args.first().and_then(|v| v.as_str()).unwrap_or("");
            if pathspec.is_empty() {
                run_git(&["diff"]).await
            } else {
                run_git(&["diff", "--", pathspec]).await
            }
        }
        "log" => {
            let n = call
                .args
                .first()
                .and_then(|v| v.as_u64())
                .unwrap_or(10)
                .min(100);
            let n_str = n.to_string();
            run_git(&["log", "--oneline", "-n", &n_str]).await
        }
        "commit" => {
            let message = call.args.first().and_then(|v| v.as_str()).unwrap_or("");
            if message.is_empty() {
                return Err(KittypawError::Sandbox(
                    "Git.commit: message is required".into(),
                ));
            }
            // Commit only already-staged files (no implicit git add -A)
            run_git(&["commit", "-m", message]).await
        }
        _ => Err(KittypawError::Sandbox(format!(
            "Unknown Git method: {}",
            call.method
        ))),
    }
}

async fn run_git(args: &[&str]) -> Result<serde_json::Value> {
    let (stdout, stderr, exit_code) = super::process::run_command("git", args, "Git").await?;

    Ok(serde_json::json!({
        "stdout": stdout,
        "stderr": stderr,
        "exit_code": exit_code,
    }))
}
