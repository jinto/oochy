use kittypaw_core::error::{KittypawError, Result};
use std::time::Duration;
use tokio::io::AsyncReadExt;

pub const TIMEOUT_SECS: u64 = 30;
pub const MAX_OUTPUT_BYTES: usize = 100 * 1024; // 100KB

/// Run a subprocess with timeout, kill-on-timeout, and output size limits.
/// Returns (stdout, stderr, exit_code).
pub async fn run_command(
    program: &str,
    args: &[&str],
    label: &str,
) -> Result<(String, String, i32)> {
    let mut child = tokio::process::Command::new(program)
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| KittypawError::Skill(format!("{label}: failed to spawn: {e}")))?;

    // Take stdout/stderr handles before waiting, so we retain child ownership for kill
    let mut stdout_handle = child.stdout.take().unwrap();
    let mut stderr_handle = child.stderr.take().unwrap();

    let work = async {
        let mut stdout_buf = Vec::new();
        let mut stderr_buf = Vec::new();
        let (_, _, status) = tokio::try_join!(
            async { stdout_handle.read_to_end(&mut stdout_buf).await },
            async { stderr_handle.read_to_end(&mut stderr_buf).await },
            child.wait(),
        )
        .map_err(|e| KittypawError::Skill(format!("{label}: {e}")))?;
        Ok::<_, KittypawError>((stdout_buf, stderr_buf, status))
    };

    match tokio::time::timeout(Duration::from_secs(TIMEOUT_SECS), work).await {
        Ok(Ok((stdout_buf, stderr_buf, status))) => {
            let stdout = truncate_utf8(&stdout_buf, MAX_OUTPUT_BYTES);
            let stderr = truncate_utf8(&stderr_buf, MAX_OUTPUT_BYTES);
            let exit_code = status.code().unwrap_or(-1);
            Ok((stdout, stderr, exit_code))
        }
        Ok(Err(e)) => Err(e),
        Err(_) => {
            // Timeout: kill the child process to prevent orphans
            let _ = child.kill().await;
            Err(KittypawError::Skill(format!(
                "{label}: timed out after {TIMEOUT_SECS}s"
            )))
        }
    }
}

/// Truncate raw bytes to a valid UTF-8 string within `max` bytes.
/// Slices the byte buffer BEFORE converting to avoid full allocation.
pub fn truncate_utf8(bytes: &[u8], max: usize) -> String {
    if bytes.len() <= max {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    let truncated = &bytes[..max];
    let s = String::from_utf8_lossy(truncated);
    format!("{}... (truncated)", s.trim_end_matches('\u{FFFD}'))
}
