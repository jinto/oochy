# Tier 1 Structural Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix security-critical permission bypass, add consistent input validation to all skill executors, and clean up dead code — bringing KittyPaw's tool execution pipeline closer to production-grade agent runtime standards.

**Architecture:** Three independent fixes (S1, S2, S3) that can be committed separately. S1 threads the existing permission callback through the skill resolver path. S2 adds early-return validation to Telegram and Storage executors. S3 wires TransitionReason into agent_loop tracing and removes truly dead code.

**Tech Stack:** Rust, tokio, kittypaw-core, kittypaw-cli, kittypaw-sandbox

---

### Task 1: S1 — Permission Bypass Path Removal

**Problem:** `on_permission_request` is accepted by `run_agent_loop()` but captured as `_on_permission` and never forwarded. All File operations through the sandbox resolver auto-allow without permission checks.

**Files:**
- Modify: `crates/kittypaw-cli/src/skill_executor.rs:104-183` (resolve_skill_call)
- Modify: `crates/kittypaw-cli/src/agent_loop.rs:228-244` (resolver closure)
- Test: `crates/kittypaw-cli/src/skill_executor.rs` (existing test module)

- [ ] **Step 1: Write the failing test — resolve_skill_call denies File.write when permission callback denies**

Add to `crates/kittypaw-cli/src/skill_executor.rs` at the end of the `mod tests` block:

```rust
#[tokio::test]
async fn test_resolve_skill_call_file_write_denied_with_permission_callback() {
    let path = temp_db_path();
    let store = Arc::new(tokio::sync::Mutex::new(open_store(&path)));
    let config = kittypaw_core::config::Config::default();

    let call = SkillCall {
        skill_name: "File".to_string(),
        method: "write".to_string(),
        args: vec![
            serde_json::Value::String("test.txt".into()),
            serde_json::Value::String("content".into()),
        ],
    };

    // Permission callback that always denies
    let deny_cb: PermissionCallback = Arc::new(|_req| {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let _ = tx.send(PermissionDecision::Deny);
        rx
    });

    let result = resolve_skill_call(&call, &config, &store, None, Some(&deny_cb)).await;
    let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert!(parsed.get("error").is_some(), "File.write should be denied");

    let _ = std::fs::remove_file(&path);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd /Users/jinto/projects/kittypaw && uv run -- cargo test -p kittypaw-cli test_resolve_skill_call_file_write_denied 2>&1 | tail -20`

Expected: Compile error — `resolve_skill_call` doesn't accept 5th argument yet.

- [ ] **Step 3: Add on_permission parameter to resolve_skill_call**

In `crates/kittypaw-cli/src/skill_executor.rs`, change the `resolve_skill_call` signature and thread it through:

```rust
pub async fn resolve_skill_call(
    call: &SkillCall,
    config: &kittypaw_core::config::Config,
    store: &Arc<Mutex<Store>>,
    checker: Option<&Arc<std::sync::Mutex<CapabilityChecker>>>,
    on_permission: Option<&PermissionCallback>,
) -> String {
```

Then update the File branch (line 128-133) to pass `on_permission`:

```rust
    if call.skill_name == "File" {
        // Check permission before file operations
        if let Err(msg) = check_file_permission(call, on_permission).await {
            return serde_json::to_string(&serde_json::json!({"error": msg}))
                .unwrap_or_else(|_| "null".to_string());
        }
        return match execute_file(call, None) {
```

And update the execute_single_call invocation (line 166-175) to pass `on_permission`:

```rust
    let result = execute_single_call(
        call,
        &config.sandbox.allowed_hosts,
        config,
        None,
        &llm_call_count,
        None,
        on_permission,
    )
    .await;
```

- [ ] **Step 4: Thread permission callback through agent_loop resolver closure**

In `crates/kittypaw-cli/src/agent_loop.rs`, change line 234 from `_on_permission` to `on_permission` and pass it through:

```rust
        let on_permission_for_resolver = on_permission_request.clone();
        let skill_resolver: Option<kittypaw_sandbox::SkillResolver> =
            Some(Arc::new(move |call: kittypaw_core::types::SkillCall| {
                let store = Arc::clone(&store_for_resolver);
                let config = Arc::clone(&config_for_resolver);
                let checker = checker_for_resolver.clone();
                let on_perm = on_permission_for_resolver.clone();
                Box::pin(async move {
                    // Convert Arc<dyn Fn(...)> to &PermissionCallback for resolve_skill_call
                    let perm_ref = on_perm.as_ref().map(|p| p as &PermissionCallback);
                    crate::skill_executor::resolve_skill_call(
                        &call,
                        &config,
                        &store,
                        checker.as_ref(),
                        perm_ref,
                    )
                    .await
                })
            }));
```

- [ ] **Step 5: Update GUI skill_config.rs Test Run resolver**

In `crates/kittypaw-gui/src/components/skill_config.rs` line 239, update the `resolve_skill_call` call to pass `None` for on_permission (GUI Test Run is explicitly user-initiated):

```rust
    kittypaw_cli::skill_executor::resolve_skill_call(&call, &config, &store, None, None).await
```

- [ ] **Step 6: Run test to verify it passes**

Run: `cd /Users/jinto/projects/kittypaw && cargo test -p kittypaw-cli test_resolve_skill_call_file_write_denied -- --nocapture 2>&1 | tail -20`

Expected: PASS

- [ ] **Step 7: Run full test suite to check for regressions**

Run: `cd /Users/jinto/projects/kittypaw && cargo test --workspace 2>&1 | tail -30`

Expected: All tests pass.

- [ ] **Step 8: Commit**

```bash
git add crates/kittypaw-cli/src/skill_executor.rs crates/kittypaw-cli/src/agent_loop.rs crates/kittypaw-gui/src/components/skill_config.rs
git commit -m "fix(security): thread permission callback through skill resolver — close auto-allow bypass"
```

---

### Task 2: S2 — Skill Input Validation Consistency

**Problem:** Telegram methods accept empty chat_id/text and make unnecessary API calls that fail. Slack and Discord already validate — Telegram doesn't. Storage.set accepts empty keys.

**Files:**
- Modify: `crates/kittypaw-cli/src/skill_executor.rs:345-475` (execute_telegram)
- Modify: `crates/kittypaw-cli/src/skill_executor.rs:903-937` (execute_storage)
- Test: `crates/kittypaw-cli/src/skill_executor.rs` (test module)

- [ ] **Step 1: Write failing tests for Telegram empty args**

Add to the test module in `crates/kittypaw-cli/src/skill_executor.rs`:

```rust
#[tokio::test]
async fn test_telegram_send_message_empty_chat_id_returns_error() {
    let config = kittypaw_core::config::Config::default();
    let call = SkillCall {
        skill_name: "Telegram".to_string(),
        method: "sendMessage".to_string(),
        args: vec![
            serde_json::Value::String("".into()),   // empty chat_id
            serde_json::Value::String("hello".into()),
        ],
    };
    let result = execute_telegram(&call, &config).await;
    assert!(result.is_err(), "Empty chat_id should fail early");
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("missing chat_id"),
        "Error should mention chat_id, got: {err_msg}"
    );
}

#[tokio::test]
async fn test_telegram_send_message_empty_text_returns_error() {
    let config = kittypaw_core::config::Config::default();
    let call = SkillCall {
        skill_name: "Telegram".to_string(),
        method: "sendMessage".to_string(),
        args: vec![
            serde_json::Value::String("12345".into()),
            serde_json::Value::String("".into()), // empty text
        ],
    };
    let result = execute_telegram(&call, &config).await;
    assert!(result.is_err(), "Empty text should fail early");
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("missing text"),
        "Error should mention text, got: {err_msg}"
    );
}

#[test]
fn test_storage_set_empty_key_returns_error() {
    let path = temp_db_path();
    let store = open_store(&path);

    let call = SkillCall {
        skill_name: "Storage".to_string(),
        method: "set".to_string(),
        args: vec![json_str(""), json_str("value")],
    };
    let result = execute_storage(&call, &store, None);
    assert!(result.is_err(), "Empty key should fail");
    let _ = std::fs::remove_file(&path);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd /Users/jinto/projects/kittypaw && cargo test -p kittypaw-cli test_telegram_send_message_empty test_storage_set_empty 2>&1 | tail -20`

Expected: FAIL — Telegram tests fail because empty chat_id triggers "bot token not configured" (no token in default config) rather than early validation. Storage test passes with empty key (no validation).

- [ ] **Step 3: Add validation to execute_telegram**

In `crates/kittypaw-cli/src/skill_executor.rs`, add validation at the start of each Telegram method:

For `sendMessage` (after line 370):
```rust
        "sendMessage" => {
            let chat_id = call.args.first().and_then(|v| v.as_str()).unwrap_or("");
            let text = call.args.get(1).and_then(|v| v.as_str()).unwrap_or("");

            if chat_id.is_empty() {
                return Err(KittypawError::Skill("Telegram: missing chat_id".into()));
            }
            if text.is_empty() {
                return Err(KittypawError::Skill("Telegram: missing text".into()));
            }
```

For `sendPhoto` (after line 403):
```rust
        "sendPhoto" => {
            let chat_id = call.args.first().and_then(|v| v.as_str()).unwrap_or("");
            let photo_url = call.args.get(1).and_then(|v| v.as_str()).unwrap_or("");

            if chat_id.is_empty() {
                return Err(KittypawError::Skill("Telegram: missing chat_id".into()));
            }
            if photo_url.is_empty() {
                return Err(KittypawError::Skill("Telegram: missing photo_url".into()));
            }
```

For `sendDocument` (after line 435):
```rust
        "sendDocument" => {
            let chat_id = call.args.first().and_then(|v| v.as_str()).unwrap_or("");
            let file_url = call.args.get(1).and_then(|v| v.as_str()).unwrap_or("");

            if chat_id.is_empty() {
                return Err(KittypawError::Skill("Telegram: missing chat_id".into()));
            }
            if file_url.is_empty() {
                return Err(KittypawError::Skill("Telegram: missing file_url".into()));
            }
```

- [ ] **Step 4: Add validation to execute_storage for set/delete**

In `crates/kittypaw-cli/src/skill_executor.rs`, add key validation:

For `set` (after line 919):
```rust
        "set" => {
            let key = call.args.first().and_then(|v| v.as_str()).unwrap_or("");
            let value = call.args.get(1).and_then(|v| v.as_str()).unwrap_or("");
            if key.is_empty() {
                return Err(KittypawError::Skill("Storage.set: key is required".into()));
            }
```

For `delete` (after line 925):
```rust
        "delete" => {
            let key = call.args.first().and_then(|v| v.as_str()).unwrap_or("");
            if key.is_empty() {
                return Err(KittypawError::Skill("Storage.delete: key is required".into()));
            }
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cd /Users/jinto/projects/kittypaw && cargo test -p kittypaw-cli test_telegram_send_message_empty test_storage_set_empty 2>&1 | tail -20`

Expected: All 3 tests PASS.

- [ ] **Step 6: Run full test suite**

Run: `cd /Users/jinto/projects/kittypaw && cargo test --workspace 2>&1 | tail -30`

Expected: All tests pass.

- [ ] **Step 7: Commit**

```bash
git add crates/kittypaw-cli/src/skill_executor.rs
git commit -m "fix: add input validation to Telegram and Storage executors — fail fast on empty args"
```

---

### Task 3: S3 — Dead Code Cleanup: Wire TransitionReason + Remove Unused Types

**Problem:** `TransitionReason` is defined (types.rs:113-138) but never used. `_skill_context` parameter has underscore prefix indicating it's unused. `NetworkPermissionRule` is defined but never checked at runtime.

**Files:**
- Modify: `crates/kittypaw-cli/src/agent_loop.rs` (wire TransitionReason)
- Modify: `crates/kittypaw-core/src/types.rs:113-138` (TransitionReason)
- Modify: `crates/kittypaw-core/src/permission.rs:34-41` (NetworkPermissionRule — add #[allow] or remove)
- Test: compile check

- [ ] **Step 1: Wire TransitionReason into agent_loop.rs tracing**

In `crates/kittypaw-cli/src/agent_loop.rs`, add TransitionReason to the imports (line 7-10):

```rust
use kittypaw_core::types::{
    now_timestamp, AgentState, ConversationTurn, Event, EventType, ExecutionResult, LlmMessage,
    LoopPhase, Role, TransitionReason,
};
```

Then update each tracing call to include the transition reason as a structured field:

Line 86 (Init):
```rust
    let reason = TransitionReason::StateReady;
    tracing::info!(phase = ?LoopPhase::Init, agent_id = %agent_id, reason = ?reason, "agent state ready");
```

Line 129-134 (Prompt):
```rust
        let reason = TransitionReason::PromptBuilt { message_count: messages.len() };
        tracing::info!(
            phase = ?LoopPhase::Prompt,
            attempt,
            recent_window = compaction.recent_window,
            reason = ?reason,
            "prompt built with compaction"
        );
```

Line 195 (Generate):
```rust
        let reason = TransitionReason::CodeGenerated { code_len: code.len() };
        tracing::info!(phase = ?LoopPhase::Generate, agent_id = %agent_id, reason = ?reason, attempt, "code generated");
```

Line 267 (Finish — success):
```rust
            let reason = TransitionReason::ExecutionSuccess {
                output_len: output.len(),
                skill_calls: exec_result.skill_calls.len(),
            };
            tracing::info!(phase = ?LoopPhase::Finish, agent_id = %agent_id, reason = ?reason, "execution success");
```

Line 289 (Retry):
```rust
        let reason = TransitionReason::ExecutionFailed {
            error: err_msg.clone(),
            attempt,
        };
        tracing::info!(phase = ?LoopPhase::Retry, agent_id = %agent_id, reason = ?reason, "execution failed, retrying");
```

Line 295 (Finish — retries exhausted):
```rust
    let reason = TransitionReason::RetriesExhausted { error: err_msg.clone() };
    tracing::info!(phase = ?LoopPhase::Finish, agent_id = %agent_id, reason = ?reason, "retries exhausted");
```

- [ ] **Step 2: Add #[allow(dead_code)] to NetworkPermissionRule with TODO comment**

In `crates/kittypaw-core/src/permission.rs`, add annotation to NetworkPermissionRule (line 35):

```rust
/// A network permission rule scoped to a workspace.
/// TODO(v2): Wire into execute_http/execute_web for domain-level permission checks.
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkPermissionRule {
```

- [ ] **Step 3: Rename _skill_context to skill_context and log it**

In `crates/kittypaw-cli/src/skill_executor.rs`, at `execute_single_call` (line 294), rename and use:

```rust
    skill_context: Option<&str>,
```

Add a tracing line at the start of the function:

```rust
async fn execute_single_call(
    call: &SkillCall,
    allowed_hosts: &[String],
    config: &kittypaw_core::config::Config,
    skill_context: Option<&str>,
    llm_call_count: &AtomicU32,
    model_override: Option<&str>,
    on_permission: Option<&PermissionCallback>,
) -> SkillResult {
    tracing::debug!(
        skill = %call.skill_name,
        method = %call.method,
        context = ?skill_context,
        "executing skill call"
    );
```

- [ ] **Step 4: Verify compilation**

Run: `cd /Users/jinto/projects/kittypaw && cargo check --workspace 2>&1 | tail -20`

Expected: No errors, no warnings about unused imports/variables.

- [ ] **Step 5: Run full test suite**

Run: `cd /Users/jinto/projects/kittypaw && cargo test --workspace 2>&1 | tail -30`

Expected: All tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/kittypaw-cli/src/agent_loop.rs crates/kittypaw-core/src/types.rs crates/kittypaw-core/src/permission.rs crates/kittypaw-cli/src/skill_executor.rs
git commit -m "refactor: wire TransitionReason into agent loop + clean up dead code"
```

---

### Task 4: Verification — Full Build + Clippy

- [ ] **Step 1: Run clippy**

Run: `cd /Users/jinto/projects/kittypaw && cargo clippy --workspace 2>&1 | tail -30`

Expected: No new warnings.

- [ ] **Step 2: Run full test suite one final time**

Run: `cd /Users/jinto/projects/kittypaw && cargo test --workspace 2>&1 | tail -30`

Expected: All tests pass.
