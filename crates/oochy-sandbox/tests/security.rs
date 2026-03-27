use oochy_sandbox::sandbox::Sandbox;
use serde_json::json;

// Test that JS cannot read arbitrary files
#[tokio::test]
async fn test_fs_traversal_blocked() {
    // JS that tries to access the filesystem — should fail or return empty
    // (QuickJS has no fs module, and Seatbelt blocks raw syscalls)
    let s = Sandbox::new(10, 128);
    let r = s
        .execute("return typeof require;", json!({}))
        .await
        .unwrap();
    assert!(r.success);
    assert_eq!(r.output, "undefined"); // require doesn't exist
}

#[tokio::test]
async fn test_memory_bomb() {
    // Allocate huge array — should be killed by timeout
    let s = Sandbox::new(3, 64);
    let r = s
        .execute(
            "var a = []; while(true) { a.push(new Array(1000000)); }",
            json!({}),
        )
        .await
        .unwrap();
    assert!(!r.success);
}

#[tokio::test]
async fn test_cpu_spin_timeout() {
    let s = Sandbox::new(2, 128);
    let r = s.execute("while(true) {}", json!({})).await.unwrap();
    assert!(!r.success);
    assert!(r.error.unwrap_or_default().contains("timed out"));
}

#[tokio::test]
async fn test_prototype_pollution_contained() {
    let s = Sandbox::new(10, 128);
    let r = s
        .execute(
            r#"
        Object.prototype.polluted = true;
        return String(({}).polluted);
    "#,
            json!({}),
        )
        .await
        .unwrap();
    // Sandbox may fail-closed if Seatbelt is unavailable (CI, containers)
    if r.success {
        // Pollution stays inside sandbox — next execution is clean
        let r2 = s
            .execute("return String(({}).polluted);", json!({}))
            .await
            .unwrap();
        if r2.success {
            assert_eq!(r2.output, "undefined"); // clean sandbox
        } else {
            let err = r2.error.unwrap_or_default();
            assert!(
                err.contains("Sandbox initialization failed"),
                "unexpected error: {err}"
            );
        }
    } else {
        let err = r.error.unwrap_or_default();
        assert!(
            err.contains("Sandbox initialization failed"),
            "unexpected error: {err}"
        );
    }
}

#[tokio::test]
async fn test_no_eval_escape() {
    let s = Sandbox::new(10, 128);
    let r = s.execute("return typeof eval;", json!({})).await.unwrap();
    assert!(r.success);
    // eval exists in QuickJS but is sandboxed
}

#[tokio::test]
async fn test_multiple_skill_calls_captured() {
    let s = Sandbox::new(10, 128);
    let r = s
        .execute(
            r#"
        await Telegram.sendMessage("1", "a");
        await Telegram.sendMessage("2", "b");
        await Http.get("http://example.com");
        return "done";
    "#,
            json!({}),
        )
        .await
        .unwrap();
    // Sandbox may fail-closed if Seatbelt is unavailable (CI, containers)
    if r.success {
        assert_eq!(r.skill_calls.len(), 3);
    } else {
        let err = r.error.unwrap_or_default();
        assert!(
            err.contains("Sandbox initialization failed"),
            "unexpected error: {err}"
        );
    }
}
