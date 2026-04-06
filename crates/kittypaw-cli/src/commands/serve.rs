use std::sync::Arc;
use tokio::sync::Mutex;

use axum::{
    extract::State,
    response::{Html, Json},
    routing::get,
};
use kittypaw_channels::websocket::ServeWebSocketChannel;
use kittypaw_core::config::ChannelType;
use kittypaw_core::types::EventType;
use kittypaw_store::Store;

use super::helpers::db_path;

// ── Dashboard types and handlers ──────────────────────────────────────────

type SharedStore = Arc<Mutex<Store>>;

async fn api_status(State(store): State<SharedStore>) -> Json<serde_json::Value> {
    let s = store.lock().await;
    match s.today_stats() {
        Ok(stats) => Json(serde_json::json!({
            "total_runs": stats.total_runs,
            "successful": stats.successful,
            "failed": stats.failed,
            "auto_retries": stats.auto_retries,
            "total_tokens": stats.total_tokens,
        })),
        Err(_) => Json(serde_json::json!({"error": "failed to load stats"})),
    }
}

async fn api_executions(State(store): State<SharedStore>) -> Json<serde_json::Value> {
    let s = store.lock().await;
    match s.recent_executions(20) {
        Ok(records) => {
            let items: Vec<serde_json::Value> = records
                .iter()
                .map(|r| {
                    serde_json::json!({
                        "skill_name": r.skill_name,
                        "started_at": r.started_at,
                        "success": r.success,
                        "duration_ms": r.duration_ms,
                        "result_summary": r.result_summary,
                        "usage_json": r.usage_json,
                    })
                })
                .collect();
            Json(serde_json::json!(items))
        }
        Err(_) => Json(serde_json::json!([])),
    }
}

async fn api_agents(State(store): State<SharedStore>) -> Json<serde_json::Value> {
    let s = store.lock().await;
    match s.list_agents() {
        Ok(agents) => {
            let items: Vec<serde_json::Value> = agents
                .iter()
                .map(|a| {
                    serde_json::json!({
                        "agent_id": a.agent_id,
                        "created_at": a.created_at,
                        "updated_at": a.updated_at,
                        "turn_count": a.turn_count,
                    })
                })
                .collect();
            Json(serde_json::json!(items))
        }
        Err(_) => Json(serde_json::json!([])),
    }
}

async fn dashboard_html() -> Html<&'static str> {
    Html(DASHBOARD_HTML)
}

const DASHBOARD_HTML: &str = r#"<!DOCTYPE html>
<html><head>
<meta charset="UTF-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>KittyPaw Dashboard</title>
<style>
  body { font-family: -apple-system, sans-serif; max-width: 900px; margin: 0 auto; padding: 20px; background: #fafaf8; color: #333; }
  h1 { font-size: 1.5em; }
  .cards { display: grid; grid-template-columns: repeat(4, 1fr); gap: 12px; margin: 20px 0; }
  .card { background: white; border: 1px solid #e7e5e4; border-radius: 8px; padding: 16px; text-align: center; }
  .card .value { font-size: 2em; font-weight: bold; }
  .card .label { font-size: 0.85em; color: #888; margin-top: 4px; }
  table { width: 100%; border-collapse: collapse; background: white; border: 1px solid #e7e5e4; border-radius: 8px; }
  th, td { padding: 8px 12px; text-align: left; border-bottom: 1px solid #eee; font-size: 0.9em; }
  th { background: #f5f5f4; font-weight: 600; }
  .ok { color: #16a34a; } .fail { color: #dc2626; }
  .refresh { color: #888; font-size: 0.8em; }
</style>
</head><body>
<h1>&#128062; KittyPaw Dashboard</h1>
<p class="refresh">Auto-refreshes every 30s</p>
<div class="cards" id="stats"></div>
<h2>Agents</h2>
<table><thead><tr><th>Agent ID</th><th>Turns</th><th>Created</th><th>Last Active</th></tr></thead>
<tbody id="agents"></tbody></table>
<h2>Recent Executions</h2>
<table><thead><tr><th>Time</th><th>Skill</th><th>Status</th><th>Duration</th><th>Summary</th></tr></thead>
<tbody id="exec"></tbody></table>
<script>
async function refresh() {
  try {
    const s = await (await fetch('/api/status')).json();
    document.getElementById('stats').innerHTML =
      '<div class="card"><div class="value">'+( s.total_runs||0)+'</div><div class="label">Today\'s Runs</div></div>'+
      '<div class="card"><div class="value ok">'+(s.successful||0)+'</div><div class="label">Successful</div></div>'+
      '<div class="card"><div class="value fail">'+(s.failed||0)+'</div><div class="label">Failed</div></div>'+
      '<div class="card"><div class="value">'+(s.total_tokens||0)+'</div><div class="label">Tokens</div></div>';
    const a = await (await fetch('/api/agents')).json();
    document.getElementById('agents').innerHTML = a.map(function(ag) {
      return '<tr><td>'+ag.agent_id+'</td><td>'+ag.turn_count+
        '</td><td>'+(ag.created_at||'').slice(0,19)+'</td><td>'+(ag.updated_at||'').slice(0,19)+'</td></tr>';
    }).join('') || '<tr><td colspan="4">No agents yet</td></tr>';
    const e = await (await fetch('/api/executions')).json();
    document.getElementById('exec').innerHTML = e.map(function(r) {
      return '<tr><td>'+(r.started_at||'').slice(0,19)+'</td><td>'+r.skill_name+
        '</td><td class="'+(r.success?'ok':'fail')+'">'+(r.success?'OK':'FAIL')+
        '</td><td>'+r.duration_ms+'ms</td><td>'+(r.result_summary||'').slice(0,60)+'</td></tr>';
    }).join('');
  } catch(err) { console.error(err); }
}
refresh(); setInterval(refresh, 30000);
</script></body></html>"#;

/// Routes a response message to the correct channel based on EventType.
struct ResponseRouter<'a> {
    ws_channel: &'a ServeWebSocketChannel,
    config: &'a kittypaw_core::config::Config,
}

impl<'a> ResponseRouter<'a> {
    async fn send_response(&self, event_type: &EventType, session_id: &str, text: &str) {
        match event_type {
            EventType::WebChat => {
                if let Err(e) = self.ws_channel.send_to_session(session_id, text).await {
                    tracing::warn!("Failed to send WebSocket response: {e}");
                }
            }
            EventType::Telegram => {
                send_telegram_message(self.config, session_id, text).await;
            }
            EventType::Desktop => {
                tracing::info!("Desktop response for {session_id}: {text}");
            }
        }
    }

    async fn send_error(&self, event_type: &EventType, session_id: &str, text: &str) {
        match event_type {
            EventType::WebChat => {
                let _ = self.ws_channel.send_to_session(session_id, text).await;
            }
            EventType::Telegram => {
                send_telegram_message(self.config, session_id, text).await;
            }
            EventType::Desktop => {
                tracing::error!("Desktop error for {session_id}: {text}");
            }
        }
    }
}

fn pid_file_path() -> std::path::PathBuf {
    kittypaw_core::secrets::data_dir()
        .unwrap_or_else(|_| std::path::PathBuf::from(".kittypaw"))
        .join("kittypaw.pid")
}

fn write_pid_file() {
    let path = pid_file_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let pid = std::process::id();
    if let Err(e) = std::fs::write(&path, pid.to_string()) {
        tracing::warn!("Failed to write PID file: {e}");
    } else {
        tracing::info!("PID file written: {} (pid={})", path.display(), pid);
    }
}

fn remove_pid_file() {
    let path = pid_file_path();
    if path.exists() {
        std::fs::remove_file(&path).ok();
    }
}

pub(crate) async fn run_serve(bind_addr: &str) {
    let config = kittypaw_core::config::Config::load().unwrap_or_else(|e| {
        eprintln!("Config error: {e}");
        std::process::exit(1);
    });

    let sandbox = Arc::new(kittypaw_sandbox::sandbox::Sandbox::new(
        config.sandbox.clone(),
    ));

    let db_path = db_path();
    let store = Arc::new(Mutex::new(Store::open(&db_path).unwrap_or_else(|e| {
        eprintln!("Database error: {e}");
        std::process::exit(1);
    })));

    // Bounded mpsc channel for all incoming events
    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel::<kittypaw_core::types::Event>(256);

    // Build LLM providers (needed for API endpoints)
    let (default_provider, fallback_provider) =
        super::helpers::require_provider_with_fallback(&config);

    // Track server start time for health endpoint
    let start_time = std::time::Instant::now();

    // Build dashboard routes (unauthenticated — backward compat)
    let dashboard_store = store.clone();
    let mut extra = axum::Router::new()
        .route("/", get(dashboard_html))
        .route("/api/status", get(api_status))
        .route("/api/executions", get(api_executions))
        .route("/api/agents", get(api_agents))
        .with_state(dashboard_store)
        .route(
            "/api/v1/health",
            get(move || async move {
                let uptime_secs = start_time.elapsed().as_secs();
                axum::Json(serde_json::json!({
                    "status": "ok",
                    "version": env!("CARGO_PKG_VERSION"),
                    "uptime_secs": uptime_secs,
                }))
            }),
        );

    // Conditionally mount authenticated REST API at /api/v1/*
    let api_state = super::api::ApiState {
        store: store.clone(),
        config: Arc::new(config.clone()),
        provider: default_provider.clone(),
        fallback_provider: fallback_provider.clone(),
        sandbox: Arc::clone(&sandbox),
    };
    if let Some(api_router) = super::api::build_api_router(&config.server.api_key, api_state) {
        extra = extra.merge(api_router);
    }

    // Start WebSocket channel (with dashboard + API routes merged)
    let ws_channel = ServeWebSocketChannel::new(bind_addr);
    let _ws_handle = ws_channel
        .spawn(event_tx.clone(), Some(extra))
        .await
        .unwrap_or_else(|e| {
            eprintln!("Failed to start WebSocket channel: {e}");
            std::process::exit(1);
        });

    // Start polling channels from config (Telegram, Slack, Discord)
    let channels = kittypaw_channels::registry::ChannelRegistry::create_all(&config.channels);
    for channel in channels {
        let name = channel.name().to_string();
        eprintln!("{name} channel started.");
        let tx = event_tx.clone();
        tokio::spawn(async move {
            if let Err(e) = channel.start(tx).await {
                tracing::error!("Channel error: {e}");
            }
        });
    }

    // All services started — write PID file now (not before, to avoid stale PIDs on startup failure)
    write_pid_file();

    eprintln!(
        "kittypaw serve started. WebSocket at ws://{}/ws/chat",
        bind_addr
    );
    eprintln!("Press Ctrl+C to stop.");

    // Spawn schedule evaluator
    let schedule_config = config.clone();
    let schedule_sandbox = kittypaw_sandbox::sandbox::Sandbox::new(config.sandbox.clone());
    let db_path_sched = db_path.clone();
    tokio::spawn(async move {
        kittypaw_engine::schedule::run_schedule_loop(
            &schedule_config,
            &schedule_sandbox,
            &db_path_sched,
        )
        .await;
    });

    // Graceful shutdown signal
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        tracing::info!("Shutting down...");
        let _ = shutdown_tx.send(true);
    });

    // Event processing loop
    tracing::info!("Event processing loop started, waiting for events...");
    loop {
        tokio::select! {
            _ = shutdown_rx.changed() => {
                tracing::info!("Shutdown signal received, exiting event loop.");
                break;
            }
            maybe_event = event_rx.recv() => {
                let event = match maybe_event {
                    Some(e) => {
                        tracing::info!(event_type = ?e.event_type, "Event received in main loop");
                        e
                    },
                    None => break,
                };

                let session_id = event.session_id();
                let event_type = event.event_type.clone();
                let router = ResponseRouter { ws_channel: &ws_channel, config: &config };

                // ── Security gate: reject unpaired Telegram chat IDs ────
                if event.event_type == EventType::Telegram
                    && !config.paired_chat_ids.is_empty()
                    && !config.paired_chat_ids.iter().any(|id| id == &session_id)
                {
                    let text = event.payload.get("text").and_then(|v| v.as_str()).unwrap_or("").trim();
                    if text.starts_with("/pair ") {
                        let code = text.strip_prefix("/pair ").unwrap().trim();
                        let expected = std::env::var("KITTYPAW_PAIR_CODE").unwrap_or_default();
                        let msg = if !expected.is_empty() && code == expected {
                            tracing::info!("Pairing accepted for chat_id={session_id}");
                            "✅ 페어링 성공! paired_chat_ids에 이 ID를 추가하세요."
                        } else {
                            "❌ 페어링 코드가 올바르지 않습니다."
                        };
                        send_telegram_message(&config, &session_id, msg).await;
                    } else {
                        tracing::warn!("Rejected message from unpaired chat_id={session_id}");
                    }
                    continue;
                }

                // ── Unified event processing via AgentSession ───────────
                // AgentSession.run() handles:
                // - Slash commands (/help, /status, /run, /teach) → fast path
                // - Natural language → LLM agent loop with full primitives
                let session = kittypaw_engine::agent_loop::AgentSession {
                    provider: &*default_provider,
                    fallback_provider: fallback_provider.as_deref(),
                    sandbox: &sandbox,
                    store: Arc::clone(&store),
                    config: &config,
                    on_token: None,
                    on_permission_request: None,
                };
                match session.run(event).await {
                    Ok(text) => {
                        router.send_response(&event_type, &session_id, &text).await;
                    }
                    Err(e) => {
                        tracing::error!("Agent error for session {session_id}: {e}");
                        router.send_error(&event_type, &session_id, &format!("Error: {e}")).await;
                    }
                }
            }
        }
    }

    remove_pid_file();
    tracing::info!("Server stopped.");
}

pub(crate) async fn send_telegram_message(
    config: &kittypaw_core::config::Config,
    chat_id: &str,
    text: &str,
) {
    let bot_token = match std::env::var("KITTYPAW_TELEGRAM_TOKEN") {
        Ok(t) => t,
        Err(_) => {
            // Try channel config
            config
                .channels
                .iter()
                .find(|c| c.channel_type == ChannelType::Telegram)
                .map(|c| c.token.clone())
                .unwrap_or_default()
        }
    };
    if bot_token.is_empty() {
        tracing::warn!("Cannot send Telegram message: no bot token configured");
        return;
    }
    let url = format!("https://api.telegram.org/bot{bot_token}/sendMessage");
    let client = reqwest::Client::new();
    let res = client
        .post(&url)
        .json(&serde_json::json!({
            "chat_id": chat_id,
            "text": text,
        }))
        .send()
        .await;
    if let Err(e) = res {
        tracing::warn!("Failed to send Telegram message: {e}");
    }
}
