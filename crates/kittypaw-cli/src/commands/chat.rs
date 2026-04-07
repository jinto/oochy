use std::io::Read;
use std::sync::Arc;
use tokio::sync::Mutex;

use kittypaw_core::config::Config;
use kittypaw_store::Store;

use super::helpers::{db_path, require_provider, require_provider_with_fallback};

pub(crate) async fn run_chat() {
    use kittypaw_core::types::{Event, EventType};
    use std::io::Write;

    let config = Config::load().unwrap_or_else(|e| {
        eprintln!("Config error: {e}");
        std::process::exit(1);
    });

    let provider = require_provider(&config);

    let db_path = db_path();
    let store = Arc::new(Mutex::new(Store::open(&db_path).unwrap_or_else(|e| {
        eprintln!("Database error: {e}");
        std::process::exit(1);
    })));

    // Fetch registry entries; fall back to empty vec on error
    let cache_dir = std::env::temp_dir().join("kittypaw-registry-cache");
    let registry_client = kittypaw_core::registry::RegistryClient::new(&cache_dir);
    let registry_entries = match registry_client.fetch_index().await {
        Ok(index) => index.packages,
        Err(e) => {
            tracing::warn!("Failed to fetch registry: {e}");
            vec![]
        }
    };

    let sandbox = kittypaw_sandbox::sandbox::Sandbox::new(config.sandbox.clone());

    println!("KittyPaw chat — type 'exit' or 'quit' to stop.\n");

    loop {
        print!("You: ");
        std::io::stdout().flush().unwrap_or_default();

        let mut line = String::new();
        match std::io::stdin().read_line(&mut line) {
            Ok(0) => break, // EOF / Ctrl+D
            Ok(_) => {}
            Err(e) => {
                eprintln!("Read error: {e}");
                break;
            }
        }

        let text = line.trim().to_string();
        if text.is_empty() {
            continue;
        }
        if text == "exit" || text == "quit" {
            break;
        }

        let event = Event {
            event_type: EventType::Desktop,
            payload: serde_json::json!({ "text": text, "workspace_id": "cli" }),
        };

        let assistant_ctx = kittypaw_cli::assistant::AssistantContext {
            event: &event,
            provider: &*provider,
            store: Arc::clone(&store),
            registry_entries: &registry_entries,
            sandbox: &sandbox,
            config: &config,
            on_token: None,
        };
        match kittypaw_cli::assistant::run_assistant_turn(&assistant_ctx).await {
            Ok(turn) => println!("KittyPaw: {}\n", turn.response_text),
            Err(e) => eprintln!("Error: {e}\n"),
        }
    }
}

pub(crate) async fn run_status() {
    let config = Config::load().unwrap_or_default();
    let db_path = db_path();
    let store = match Store::open(&db_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Database error: {e}");
            return;
        }
    };

    let stats = match store.today_stats() {
        Ok(s) => {
            println!("=== KittyPaw Status ===");
            println!(
                "Today: {} runs ({} ok, {} failed), {} retries, {} tokens",
                s.total_runs, s.successful, s.failed, s.auto_retries, s.total_tokens
            );
            Some(s)
        }
        Err(e) => {
            eprintln!("Error loading stats: {e}");
            None
        }
    };

    match store.recent_executions(5) {
        Ok(records) if !records.is_empty() => {
            println!("\nRecent executions:");
            for r in &records {
                let status = if r.success { "ok" } else { "FAIL" };
                let tokens = parse_usage_tokens(&r.usage_json);
                println!(
                    "  {} | {:<20} | {:>4} | {:>6}ms | {} tokens",
                    &r.started_at[..19.min(r.started_at.len())],
                    r.skill_name,
                    status,
                    r.duration_ms,
                    tokens
                );
            }
        }
        Ok(_) => println!("\nNo recent executions."),
        Err(e) => eprintln!("Error loading executions: {e}"),
    }

    if let Some(ref s) = stats {
        if config.features.daily_token_limit > 0 {
            let pct =
                (s.total_tokens as f64 / config.features.daily_token_limit as f64 * 100.0) as u64;
            println!(
                "\nToken budget: {}/{} ({}%)",
                s.total_tokens, config.features.daily_token_limit, pct
            );
        }
    }
}

pub(crate) async fn run_log(skill: Option<String>, limit: usize) {
    let db_path = db_path();
    let store = match Store::open(&db_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Database error: {e}");
            return;
        }
    };

    let records = if let Some(ref name) = skill {
        store.search_executions(name, limit).unwrap_or_default()
    } else {
        store.recent_executions(limit).unwrap_or_default()
    };

    if records.is_empty() {
        println!("No executions found.");
        return;
    }

    for r in &records {
        let status = if r.success { "ok" } else { "FAIL" };
        let tokens = parse_usage_tokens(&r.usage_json);
        let summary = if r.result_summary.len() > 60 {
            format!("{}...", &r.result_summary[..57])
        } else {
            r.result_summary.clone()
        };
        println!(
            "{} | {:<20} | {:>4} | {:>6}ms | {:>6} tok | {}",
            &r.started_at[..19.min(r.started_at.len())],
            r.skill_name,
            status,
            r.duration_ms,
            tokens,
            summary
        );
    }
}

fn parse_usage_tokens(usage_json: &Option<String>) -> u64 {
    usage_json
        .as_deref()
        .map(kittypaw_store::sum_usage_tokens)
        .unwrap_or(0)
}

pub(crate) async fn run_stdin() {
    // Read event from stdin
    let mut input = String::new();
    if atty::is(atty::Stream::Stdin) {
        eprintln!("kittypaw v{}", env!("CARGO_PKG_VERSION"));
        eprintln!(
            "Usage: echo '{{\"type\":\"web_chat\",\"payload\":{{\"text\":\"hello\"}}}}' | kittypaw"
        );
        eprintln!("       kittypaw serve [--bind 0.0.0.0:3000]");
        std::process::exit(0);
    }

    std::io::stdin()
        .read_to_string(&mut input)
        .expect("failed to read stdin");
    let input = input.trim();
    if input.is_empty() {
        eprintln!("Error: empty input");
        std::process::exit(1);
    }

    // Parse event
    let event: kittypaw_core::types::Event = match serde_json::from_str(input) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("Error parsing event JSON: {e}");
            eprintln!("Expected: {{\"type\":\"web_chat\",\"payload\":{{\"text\":\"hello\"}}}}");
            std::process::exit(1);
        }
    };

    // Load config
    let config = Config::load().unwrap_or_else(|e| {
        eprintln!("Config error: {e}");
        std::process::exit(1);
    });

    let (provider, fallback) = require_provider_with_fallback(&config);

    let sandbox = kittypaw_sandbox::sandbox::Sandbox::new(config.sandbox.clone());

    let db_path = db_path();
    let store = Arc::new(Mutex::new(Store::open(&db_path).unwrap_or_else(|e| {
        eprintln!("Database error: {e}");
        std::process::exit(1);
    })));

    // Run agent loop with overall timeout to prevent indefinite blocking
    let timeout_secs = config.sandbox.timeout_secs as u64 * 4; // e.g. 30 * 4 = 120s
    let session = kittypaw_cli::agent_loop::AgentSession {
        provider: &*provider,
        fallback_provider: fallback.as_deref(),
        sandbox: &sandbox,
        store,
        config: &config,
        on_token: None,
        on_permission_request: None,
    };
    let loop_future = session.run(event);
    match tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), loop_future).await {
        Ok(Ok(output)) => {
            println!("{output}");
        }
        Ok(Err(e)) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
        Err(_elapsed) => {
            eprintln!("Error: agent loop timed out after {timeout_secs}s");
            std::process::exit(1);
        }
    }
}

/// Simulate a channel event through the full agent pipeline.
/// Same path as `serve` — useful for debugging without a real Telegram bot.
pub(crate) async fn run_test_event(message: &str, channel: &str, chat_id: Option<&str>) {
    use kittypaw_core::types::{Event, EventType};

    let config = Config::load().unwrap_or_else(|e| {
        eprintln!("Config error: {e}");
        std::process::exit(1);
    });

    let (provider, fallback) = require_provider_with_fallback(&config);
    let sandbox = kittypaw_sandbox::Sandbox::new_threaded(config.sandbox.clone());

    let db_path = db_path();
    let store = Arc::new(Mutex::new(Store::open(&db_path).unwrap_or_else(|e| {
        eprintln!("Database error: {e}");
        std::process::exit(1);
    })));

    // Resolve chat_id: arg > secrets > "test-cli"
    let chat_id = chat_id
        .map(|s| s.to_string())
        .or_else(|| {
            kittypaw_core::secrets::get_secret("telegram", "chat_id")
                .ok()
                .flatten()
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| "test-cli".to_string());

    let event_type = match channel {
        "telegram" => EventType::Telegram,
        "web_chat" => EventType::WebChat,
        _ => EventType::Desktop,
    };

    let event = Event {
        event_type,
        payload: serde_json::json!({
            "text": message,
            "chat_id": chat_id,
            "from_name": "test-event",
        }),
    };

    eprintln!(">>> test-event: channel={channel}, chat_id={chat_id}");
    eprintln!(">>> message: {message}");
    eprintln!();

    let session = kittypaw_engine::agent_loop::AgentSession {
        provider: &*provider,
        fallback_provider: fallback.as_deref(),
        sandbox: &sandbox,
        store,
        config: &config,
        on_token: None,
        on_permission_request: None,
    };

    match session.run(event).await {
        Ok(text) => {
            if text.is_empty() || text == "null" || text == "(no output)" {
                eprintln!(">>> (no output — skill calls were executed inline)");
            } else {
                println!("{text}");
            }
        }
        Err(e) => {
            eprintln!(">>> ERROR: {e}");
            std::process::exit(1);
        }
    }
}
