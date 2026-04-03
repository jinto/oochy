use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use kittypaw_core::types::{ExecutionResult, SkillCall};
use rquickjs::function::Rest;
use rquickjs::prelude::Async;
use rquickjs::{async_with, AsyncContext, AsyncRuntime, Function, Object, Value};

use crate::backend::SkillResolver;

const MAX_SKILL_CALLS: usize = 100;

/// Serialize a JS `Value` to `serde_json::Value` using the context embedded in the value.
/// Using `v.ctx()` avoids a separate `Ctx<'js>` parameter, which would cause lifetime
/// variance conflicts when used in a `Rest<Value<'_>>` closure.
fn js_value_to_json(v: &Value<'_>) -> serde_json::Value {
    v.ctx()
        .json_stringify(v.clone())
        .ok()
        .flatten()
        .and_then(|s| s.to_string().ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(serde_json::Value::Null)
}

pub(crate) const KNOWN_SKILLS: &[(&str, &[&str])] = &[
    (
        "Telegram",
        &["sendMessage", "sendPhoto", "editMessage", "sendDocument"],
    ),
    ("Slack", &["sendMessage"]),
    ("Discord", &["sendMessage"]),
    ("Http", &["get", "post", "put", "delete"]),
    ("Storage", &["get", "set", "delete", "list"]),
    ("Llm", &["generate"]),
    ("File", &["read", "write"]),
    ("Env", &["get"]),
    ("Web", &["search", "fetch"]),
];

/// Run QuickJS using AsyncRuntime + AsyncContext (avoids RefCell borrow conflicts).
/// Must run on a thread with its own tokio runtime.
///
/// `timeout`: when `Some`, an interrupt handler is installed that terminates JS
/// execution after the deadline. Used by `ThreadSandbox` since there is no
/// SIGALRM / fork backstop.
pub(crate) fn run_child_async(
    code: &str,
    context: serde_json::Value,
    timeout: Option<Duration>,
    skill_resolver: Option<SkillResolver>,
) -> ExecutionResult {
    let code = code.to_string();
    let calls: Arc<Mutex<Vec<SkillCall>>> = Arc::new(Mutex::new(Vec::new()));
    let interrupted = Arc::new(AtomicBool::new(false));

    // Spawn a fresh thread to get a clean tokio runtime (no parent runtime inheritance after fork)
    let calls_clone = Arc::clone(&calls);
    let interrupted_clone = Arc::clone(&interrupted);
    let handle = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime");

        rt.block_on(async move {
            let qrt = AsyncRuntime::new().expect("create AsyncRuntime");

            // Install interrupt handler when a timeout is requested
            if let Some(dur) = timeout {
                let deadline = Instant::now() + dur;
                let flag = Arc::clone(&interrupted_clone);
                qrt.set_interrupt_handler(Some(Box::new(move || {
                    if Instant::now() >= deadline {
                        flag.store(true, Ordering::Relaxed);
                        true // interrupt JS execution
                    } else {
                        false
                    }
                })))
                .await;
            }

            let qctx = AsyncContext::full(&qrt).await.expect("create AsyncContext");

            // Clone resolver so it can be moved into the async_with! block
            let resolver = skill_resolver;

            // Everything inside one async_with! block — proven pattern from spike 0.1
            let result = async_with!(qctx => |ctx| {
                let globals = ctx.globals();

                // Inject skill stubs as async host functions
                for (skill_name, methods) in KNOWN_SKILLS {
                    let obj = Object::new(ctx.clone()).unwrap();
                    for method in *methods {
                        let skill = skill_name.to_string();
                        let meth = method.to_string();
                        let cc = Arc::clone(&calls_clone);
                        let resolver = resolver.clone();
                        let func = Function::new(
                            ctx.clone(),
                            Async(move |args: Rest<Value<'_>>| {
                                let skill = skill.clone();
                                let meth = meth.clone();
                                let cc = Arc::clone(&cc);
                                let resolver = resolver.clone();
                                // Serialize JS values to serde_json::Value before async move,
                                // while we still hold the JS context reference.
                                // js_value_to_json uses v.ctx() internally, avoiding a
                                // separate Ctx<'_> parameter that would cause lifetime conflicts.
                                let json_args: Vec<serde_json::Value> = args.0.iter()
                                    .map(js_value_to_json)
                                    .collect();
                                async move {
                                    {
                                        let mut guard = cc.lock().unwrap();
                                        if guard.len() >= MAX_SKILL_CALLS {
                                            return Err(rquickjs::Error::Exception);
                                        }
                                        let call = SkillCall { skill_name: skill.clone(), method: meth.clone(), args: json_args.clone() };
                                        guard.push(call);
                                    }
                                    let call = SkillCall { skill_name: skill, method: meth, args: json_args };
                                    if let Some(ref resolve) = resolver {
                                        Ok(resolve(call).await)
                                    } else {
                                        Ok("null".to_string())
                                    }
                                }
                            }),
                        ).unwrap();
                        let _ = obj.set(*method, func);
                    }
                    let _ = globals.set(*skill_name, obj);
                }

                // Console shim
                let console = Object::new(ctx.clone()).unwrap();
                let log_fn = Function::new(ctx.clone(), |_: Rest<String>| {}).unwrap();
                let _ = console.set("log", log_fn);
                let noop = Function::new(ctx.clone(), |_: Rest<Value>| {}).unwrap();
                let _ = console.set("error", noop.clone());
                let _ = console.set("warn", noop);
                let _ = globals.set("console", console);
                let _ = globals.set("__context__", context.to_string());

                // Wrap code in async IIFE and await the promise
                let wrapped = format!("(async function() {{\n{code}\n}})()");
                let eval_result: std::result::Result<rquickjs::Promise, _> = ctx.eval(wrapped.as_bytes());

                match eval_result {
                    Err(e) => ExecutionResult {
                        success: false,
                        output: String::new(),
                        skill_calls: vec![],
                        error: Some(format!("JS error: {e}")),
                    },
                    Ok(promise) => {
                        // Await promise — drives QuickJS + Tokio cooperatively
                        let resolved: std::result::Result<String, _> = promise.into_future().await;
                        let skill_calls = calls_clone.lock().unwrap().clone();
                        match resolved {
                            Ok(output) => ExecutionResult {
                                success: true,
                                output,
                                skill_calls,
                                error: None,
                            },
                            Err(e) => ExecutionResult {
                                success: false,
                                output: String::new(),
                                skill_calls,
                                error: Some(format!("JS runtime error: {e}")),
                            },
                        }
                    }
                }
            }).await;

            result
        })
    });

    let exec_result = handle.join().unwrap_or_else(|_| ExecutionResult {
        success: false,
        output: String::new(),
        skill_calls: vec![],
        error: Some("child thread panicked".into()),
    });

    // If the interrupt handler fired, surface it as a timeout error
    if interrupted.load(Ordering::Relaxed) {
        return ExecutionResult {
            success: false,
            output: String::new(),
            skill_calls: vec![],
            error: Some("execution timed out".into()),
        };
    }

    exec_result
}
