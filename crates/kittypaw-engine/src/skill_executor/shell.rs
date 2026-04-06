use kittypaw_core::error::{KittypawError, Result};
use kittypaw_core::types::SkillCall;

pub(super) async fn execute_shell(call: &SkillCall) -> Result<serde_json::Value> {
    match call.method.as_str() {
        "exec" => {
            let command = call.args.first().and_then(|v| v.as_str()).unwrap_or("");
            if command.is_empty() {
                return Err(KittypawError::Sandbox(
                    "Shell.exec: command is required".into(),
                ));
            }

            let (stdout, stderr, exit_code) =
                super::process::run_command("sh", &["-c", command], "Shell.exec").await?;

            Ok(serde_json::json!({
                "stdout": stdout,
                "stderr": stderr,
                "exit_code": exit_code,
            }))
        }
        _ => Err(KittypawError::Sandbox(format!(
            "Unknown Shell method: {}",
            call.method
        ))),
    }
}
