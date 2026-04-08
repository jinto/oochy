use kittypaw_store::Store;

use super::helpers::db_path;

pub(crate) fn run_reflection_list() {
    let db_path = db_path();
    let store = match Store::open(&db_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Database error: {e}");
            return;
        }
    };

    // Pending suggestions (filter empty values from old-style "clear")
    let candidates: Vec<_> = store
        .list_user_context_prefix("suggest_candidate:")
        .unwrap_or_default()
        .into_iter()
        .filter(|(_, v)| !v.is_empty())
        .collect();
    if !candidates.is_empty() {
        println!("=== Pending Suggestions ===\n");
        for (key, value) in &candidates {
            let hash = key.strip_prefix("suggest_candidate:").unwrap_or(key);
            let parts: Vec<&str> = value.splitn(3, '|').collect();
            let label = parts.first().unwrap_or(&"?");
            let count = parts.get(1).unwrap_or(&"?");
            println!("  {} ({}x) — hash: {}", label, count, hash);
        }
        println!(
            "\n  승인: kittypaw reflection approve <hash>\n  거절: kittypaw reflection reject <hash>"
        );
    } else {
        println!("No pending suggestions.");
    }

    // Learned patterns
    let intents = store.list_reflection_intents(20).unwrap_or_default();
    if !intents.is_empty() {
        println!("\n=== Learned Patterns ===\n");
        for (key, value) in &intents {
            let hash = key.strip_prefix("reflection:intent:").unwrap_or(key);
            println!("  {} — {}", value, hash);
        }
    }

    // Rejected intents
    let rejected = store
        .list_user_context_prefix("rejected_intent:")
        .unwrap_or_default();
    if !rejected.is_empty() {
        println!("\n=== Rejected Intents ===\n");
        for (key, value) in &rejected {
            let hash = key.strip_prefix("rejected_intent:").unwrap_or(key);
            println!("  {} — {}", value, hash);
        }
    }
}

pub(crate) async fn run_reflection_approve(hash: &str) {
    let db_path = db_path();
    let store = match Store::open(&db_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Database error: {e}");
            return;
        }
    };

    let key = format!("suggest_candidate:{hash}");
    let value = match store.get_user_context(&key) {
        Ok(Some(v)) if !v.is_empty() => v,
        Ok(_) => {
            eprintln!("No pending suggestion with hash '{hash}'.");
            eprintln!("Run 'kittypaw reflection list' to see available suggestions.");
            return;
        }
        Err(e) => {
            eprintln!("Error: {e}");
            return;
        }
    };

    let parts: Vec<&str> = value.splitn(3, '|').collect();
    let label = parts.first().copied().unwrap_or("unknown");
    let schedule = parts.get(2).copied().unwrap_or("0 0 9 * * *");

    println!("Approved: {}", label);
    println!("Schedule: {}", schedule);
    println!("Creating skill via teach_loop...");

    let config = kittypaw_core::config::Config::load().unwrap_or_default();
    let provider = super::helpers::require_provider(&config);
    let sandbox = kittypaw_sandbox::Sandbox::new_threaded(config.sandbox.clone());

    match kittypaw_engine::teach_loop::handle_teach(
        label,
        "local",
        &*provider,
        &sandbox,
        &config,
        Some(schedule),
    )
    .await
    {
        Ok(result @ kittypaw_engine::teach_loop::TeachResult::Generated { .. }) => {
            match kittypaw_engine::teach_loop::approve_skill(&result) {
                Ok(()) => {
                    println!("Skill created and scheduled!");
                    let _ = store.delete_user_context(&key);
                }
                Err(e) => eprintln!("Failed to save skill: {e}"),
            }
        }
        Ok(kittypaw_engine::teach_loop::TeachResult::Error(e)) => {
            eprintln!("teach_loop failed: {e}");
        }
        Err(e) => eprintln!("Error: {e}"),
    }
}

pub(crate) fn run_reflection_reject(hash: &str) {
    let db_path = db_path();
    let store = match Store::open(&db_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Database error: {e}");
            return;
        }
    };

    let candidate_key = format!("suggest_candidate:{hash}");
    let label = store
        .get_user_context(&candidate_key)
        .ok()
        .flatten()
        .and_then(|v| v.split('|').next().map(String::from))
        .unwrap_or_else(|| "unknown".into());

    // Store rejection
    let reject_key = format!("rejected_intent:{hash}");
    match store.set_user_context(&reject_key, &label, "user") {
        Ok(()) => {
            let _ = store.delete_user_context(&candidate_key);
            println!("Rejected: {} — will not suggest again.", label);
        }
        Err(e) => eprintln!("Error: {e}"),
    }
}

/// Manually trigger one reflection cycle (for debugging / E2E testing).
pub(crate) async fn run_reflection_now() {
    let db_path = db_path();
    let store = match kittypaw_store::Store::open(&db_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Database error: {e}");
            return;
        }
    };

    let config = kittypaw_core::config::Config::load().unwrap_or_default();
    let provider = super::helpers::require_provider(&config);

    println!("Running reflection analysis...");
    match kittypaw_engine::reflection::run_reflection(&store, &*provider, &config.reflection).await
    {
        Ok(result) => {
            if result.suggestions.is_empty() {
                println!("No new patterns detected.");
            } else {
                for sg in &result.suggestions {
                    println!(
                        "  💡 {} ({}x) — hash: {}",
                        sg.intent_label, sg.count, sg.intent_hash
                    );
                }
                println!(
                    "\n  승인: kittypaw reflection approve <hash>\n  거절: kittypaw reflection reject <hash>"
                );
            }
            if result.swept > 0 {
                println!("  Swept {} expired entries.", result.swept);
            }
        }
        Err(e) => eprintln!("Reflection failed: {e}"),
    }
}

pub(crate) fn run_reflection_clear() {
    let db_path = db_path();
    let store = match Store::open(&db_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Database error: {e}");
            return;
        }
    };

    let mut deleted = 0;
    for prefix in &["reflection:", "rejected_intent:", "suggest_candidate:"] {
        deleted += store.delete_user_context_prefix(prefix).unwrap_or(0);
    }
    println!("Cleared {} reflection entries.", deleted);
}
