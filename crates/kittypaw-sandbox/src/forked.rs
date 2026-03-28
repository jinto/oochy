#[cfg(unix)]
use std::time::Duration;

#[cfg(unix)]
use async_trait::async_trait;
#[cfg(unix)]
use kittypaw_core::error::Result;
#[cfg(unix)]
use kittypaw_core::types::ExecutionResult;

#[cfg(unix)]
use crate::backend::SandboxBackend;
#[cfg(unix)]
use crate::backend::SandboxExecConfig;
#[cfg(unix)]
use crate::backend::SkillResolver;
#[cfg(unix)]
use crate::quickjs::run_child_async;

#[cfg(unix)]
#[cfg(target_os = "macos")]
use std::ffi::CString;

#[cfg(unix)]
#[cfg(target_os = "macos")]
const SEATBELT_PROFILE: &str = r#"
(version 1)
(deny default)
(allow process*)
(allow sysctl-read)
(allow signal)
(allow mach*)
(allow ipc*)
(deny network*)
"#;

#[cfg(unix)]
#[cfg(target_os = "macos")]
extern "C" {
    fn sandbox_init(
        profile: *const libc::c_char,
        flags: u64,
        errorbuf: *mut *mut libc::c_char,
    ) -> libc::c_int;
    fn sandbox_free_error(errorbuf: *mut libc::c_char);
}

#[cfg(unix)]
fn apply_seatbelt() -> std::result::Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let profile_cstr = CString::new(SEATBELT_PROFILE).unwrap();
        let mut errbuf: *mut libc::c_char = std::ptr::null_mut();
        let ret = unsafe { sandbox_init(profile_cstr.as_ptr(), 0, &mut errbuf) };
        if ret != 0 {
            let msg = if !errbuf.is_null() {
                let s = unsafe { std::ffi::CStr::from_ptr(errbuf) }
                    .to_string_lossy()
                    .to_string();
                unsafe { sandbox_free_error(errbuf) };
                s
            } else {
                format!("sandbox_init returned {ret}")
            };
            return Err(msg);
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        // On Linux, Seatbelt is not available. Landlock support is deferred to v2.
        // The QuickJS VM + fork isolation still provides a security layer.
    }

    Ok(())
}

#[cfg(unix)]
fn write_to_fd(fd: libc::c_int, data: &[u8]) {
    let mut offset = 0;
    while offset < data.len() {
        let n = unsafe {
            libc::write(
                fd,
                data[offset..].as_ptr() as *const libc::c_void,
                data.len() - offset,
            )
        };
        if n <= 0 {
            break;
        }
        offset += n as usize;
    }
}

#[cfg(unix)]
fn read_all_from_fd(fd: libc::c_int) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let n = unsafe { libc::read(fd, chunk.as_mut_ptr() as *mut libc::c_void, chunk.len()) };
        if n <= 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n as usize]);
    }
    buf
}

#[cfg(unix)]
fn run_child(write_fd: libc::c_int, code: &str, context: serde_json::Value, timeout_secs: u64) {
    if let Err(e) = apply_seatbelt() {
        let result = ExecutionResult {
            success: false,
            output: String::new(),
            skill_calls: vec![],
            error: Some(format!(
                "Sandbox initialization failed (will not execute unsandboxed): {e}"
            )),
        };
        if let Ok(json) = serde_json::to_string(&result) {
            write_to_fd(write_fd, json.as_bytes());
        }
        unsafe { libc::close(write_fd) };
        return;
    }
    // SIGALRM backstop for infinite JS loops
    unsafe { libc::alarm(timeout_secs as u32) };

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_child_async(code, context, None, None)
    }))
    .unwrap_or_else(|_| ExecutionResult {
        success: false,
        output: String::new(),
        skill_calls: vec![],
        error: Some("child panicked".into()),
    });

    let json = serde_json::to_string(&result).unwrap_or_else(|e| {
        format!(r#"{{"success":false,"output":"","skill_calls":[],"error":"serialize: {e}"}}"#)
    });
    write_to_fd(write_fd, json.as_bytes());
    unsafe { libc::close(write_fd) };
}

/// Fork-based sandbox backend. Provides OS-level isolation via fork + Seatbelt (macOS).
///
/// # Platform
/// Only available on unix platforms.
#[cfg(unix)]
pub struct ForkedSandbox {
    pub timeout_secs: u64,
    pub memory_limit_mb: u64,
}

#[cfg(unix)]
impl ForkedSandbox {
    pub fn new(timeout_secs: u64, memory_limit_mb: u64) -> Self {
        Self {
            timeout_secs,
            memory_limit_mb,
        }
    }
}

#[cfg(unix)]
#[async_trait]
impl SandboxBackend for ForkedSandbox {
    async fn execute(
        &self,
        config: SandboxExecConfig,
        _skill_resolver: Option<SkillResolver>,
    ) -> Result<ExecutionResult> {
        let mut fds = [0i32; 2];
        if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
            return Err(kittypaw_core::error::KittypawError::Sandbox(format!(
                "pipe() failed: {}",
                std::io::Error::last_os_error()
            )));
        }
        let (read_fd, write_fd) = (fds[0], fds[1]);
        let code_owned = config.code.clone();
        let context: serde_json::Value = serde_json::from_str(&config.context_json)
            .unwrap_or(serde_json::Value::Object(Default::default()));
        let timeout_secs = config.timeout_ms / 1000;

        let pid = unsafe { libc::fork() };
        match pid {
            -1 => {
                unsafe {
                    libc::close(read_fd);
                    libc::close(write_fd);
                }
                Err(kittypaw_core::error::KittypawError::Sandbox(format!(
                    "fork() failed: {}",
                    std::io::Error::last_os_error()
                )))
            }
            0 => {
                // CHILD
                unsafe { libc::close(read_fd) };
                run_child(write_fd, &code_owned, context, timeout_secs);
                unsafe { libc::_exit(0) };
            }
            child_pid => {
                // PARENT
                unsafe { libc::close(write_fd) };
                let parent_timeout = Duration::from_secs(timeout_secs + 5);
                let read_result = tokio::time::timeout(parent_timeout, async {
                    tokio::task::spawn_blocking(move || {
                        let data = read_all_from_fd(read_fd);
                        unsafe { libc::close(read_fd) };
                        String::from_utf8_lossy(&data).to_string()
                    })
                    .await
                    .unwrap_or_default()
                })
                .await;

                let mut status = 0i32;
                unsafe { libc::kill(child_pid, libc::SIGKILL) };
                unsafe { libc::waitpid(child_pid, &mut status, 0) };

                match read_result {
                    Ok(output) if !output.is_empty() => {
                        serde_json::from_str::<ExecutionResult>(&output).map_err(|e| {
                            kittypaw_core::error::KittypawError::Sandbox(format!(
                                "parse child output: {e} (raw: {output:?})"
                            ))
                        })
                    }
                    _ => Ok(ExecutionResult {
                        success: false,
                        output: String::new(),
                        skill_calls: vec![],
                        error: Some("execution timed out".into()),
                    }),
                }
            }
        }
    }
}
